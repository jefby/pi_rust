//! `pi` — interactive coding agent CLI.

mod config;
mod file_config;
mod interactive;
mod permission;
mod pi_agent_config;
mod print_mode;
mod project;
mod session;
mod slash;
mod system_prompt;
#[cfg(feature = "tui")]
mod tui;

#[cfg(feature = "tui")]
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};

use crate::config::{
    list_models, parse_thinking_level, resolve_model, resolve_provider, AppConfig,
};
use crate::permission::{CliPermission, Mode};

#[derive(Parser, Debug)]
#[command(name = "pi", version, about = "Pi coding agent (Rust port)")]
struct Cli {
    /// One-shot prompt — run agent to completion and exit.
    #[arg(short, long)]
    prompt: Option<String>,

    /// Model identifier. Overrides PI_MODEL.
    #[arg(short = 'm', long, env = "PI_MODEL")]
    model: Option<String>,

    /// Maximum agent turns before stopping.
    #[arg(long)]
    max_turns: Option<u32>,

    /// Skip permission prompts (DANGEROUS — bash/write/edit run without confirm).
    #[arg(long)]
    yolo: bool,

    /// In print mode (`-p`), emit JSON-lines on stdout instead of human text.
    #[arg(long)]
    json: bool,

    /// Resume a session: `-r <id|path>` for a specific one, or bare `-r` for
    /// the most recent. Accepts a native JSON or upstream `.jsonl` file.
    #[arg(
        short = 'r',
        long,
        value_name = "ID|PATH",
        num_args = 0..=1,
        default_missing_value = ""
    )]
    resume: Option<String>,

    /// Load a session by id or by path (native JSON or upstream `.jsonl`).
    /// Alias of `--resume` that is explicit about file paths.
    #[arg(long)]
    session: Option<String>,

    /// Use the full-screen TUI (requires a build with `--features tui`).
    #[arg(long)]
    tui: bool,

    /// Disable the full-screen TUI and use the line REPL.
    #[arg(long)]
    no_tui: bool,

    /// Extended-thinking budget: off, minimal, low, medium, high, xhigh, or max.
    #[arg(long)]
    thinking: Option<String>,

    /// Continue the most recent saved session.
    #[arg(short = 'c', long = "continue")]
    continue_latest: bool,

    /// Force a provider (built-in or from the upstream ~/.pi/agent config).
    #[arg(long)]
    provider: Option<String>,

    /// Override the API key for the selected provider.
    #[arg(long)]
    api_key: Option<String>,

    /// Replace the default system prompt.
    #[arg(long)]
    system_prompt: Option<String>,

    /// Append text to the system prompt (repeatable).
    #[arg(long = "append-system-prompt")]
    append_system_prompt: Vec<String>,

    /// Disable AGENTS.md / CLAUDE.md context-file discovery.
    #[arg(long = "no-context-files")]
    no_context_files: bool,

    /// List available models (optional case-insensitive search) and exit.
    #[arg(
        long = "list-models",
        value_name = "SEARCH",
        num_args = 0..=1,
        default_missing_value = ""
    )]
    list_models: Option<String>,

    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Manage saved sessions.
    Sessions {
        #[command(subcommand)]
        action: SessionAction,
    },
}

#[derive(Subcommand, Debug)]
enum SessionAction {
    /// List saved sessions.
    List,
    /// Show a single session as pretty JSON.
    Show { id: String },
    /// Delete a session by id.
    Delete { id: String },
    /// Import an upstream (`@earendil-works/pi`) `.jsonl` session.
    Import { path: String },
    /// Export a local session as upstream v3 JSONL.
    Export {
        id: String,
        /// Destination file (defaults to `./<id>.jsonl`).
        #[arg(long)]
        to: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    // Load `$XDG_CONFIG_HOME/pi/config.toml` (best-effort).
    let file_cfg = file_config::load();
    // Bridge to the upstream `~/.pi/agent` config (settings/auth/models).
    let agent_home = pi_agent_config::AgentHome::load();

    let cli = Cli::parse();

    // Model precedence: `-m` / `PI_MODEL` → `config.toml` → upstream settings.json.
    let explicit_model = cli.model.clone().or_else(|| file_cfg.model.clone());
    if let Some(m) = &explicit_model {
        // Keep `PI_MODEL` coherent for anything that reads it later.
        std::env::set_var("PI_MODEL", m);
    }
    let mut resolved = match cli.provider.as_deref() {
        Some(provider) => {
            match resolve_provider(provider, explicit_model.as_deref(), agent_home.as_ref()) {
                Some(resolved) => resolved,
                None => anyhow::bail!(
                    "cannot resolve provider `{provider}`; pass -m <model> or run --list-models"
                ),
            }
        }
        None => resolve_model(explicit_model.as_deref(), agent_home.as_ref()),
    };
    if let Some(key) = cli.api_key.clone() {
        resolved.api_key = Some(key);
    }
    tracing::debug!(
        provider = %resolved.model.provider,
        model = %resolved.model.id,
        api = %resolved.model.api,
        base_url = %resolved.model.base_url,
        has_api_key = resolved.api_key.is_some(),
        "resolved model"
    );

    if let Some(search) = cli.list_models.as_deref() {
        for entry in list_models(search, agent_home.as_ref()) {
            println!("{entry}");
        }
        return Ok(());
    }

    // CLI flags / env win; the files fill holes.
    let max_turns = cli.max_turns.or(file_cfg.max_turns).unwrap_or(32);
    let thinking_level = if let Some(level) = cli.thinking.as_deref() {
        parse_thinking_level(level)
            .ok_or_else(|| anyhow::anyhow!("invalid --thinking value `{level}`"))?
    } else {
        let env_thinking = std::env::var("PI_REASONING_LEVEL").ok();
        file_cfg
            .thinking_level
            .as_deref()
            .or(env_thinking.as_deref())
            .or_else(|| {
                agent_home
                    .as_ref()
                    .and_then(|h| h.settings.default_thinking_level.as_deref())
            })
            .and_then(parse_thinking_level)
            .unwrap_or_default()
    };
    let yolo = cli.yolo || file_cfg.yolo;
    let json = cli.json || file_cfg.json;

    let app = AppConfig {
        model: resolved.model,
        api_key: resolved.api_key,
        max_turns,
        thinking_level,
        system_prompt: cli.system_prompt.clone(),
        system_prompt_append: cli.append_system_prompt.clone(),
        no_context_files: cli.no_context_files,
        ..AppConfig::default()
    };

    if let Some(Cmd::Sessions { action }) = cli.cmd {
        return run_sessions_cmd(&app, action);
    }

    let permission: Arc<dyn pi_agent::PermissionPolicy> = if yolo {
        Arc::new(CliPermission::new(Mode::Yolo))
    } else {
        Arc::new(CliPermission::new(Mode::Interactive))
    };

    // `-r` / `--resume` may carry a target or be bare (=> most recent).
    let resume_target = cli
        .resume
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| cli.session.clone());
    let bare_resume = matches!(cli.resume.as_deref(), Some(""));

    match (cli.prompt, resume_target) {
        (Some(p), _) => print_mode::run_print(&app, p, permission, json).await,
        (None, resume_id) => {
            let initial = match resume_id {
                Some(target) => match load_session_target(&app.config_dir, &target) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        eprintln!("warning: failed to load session {target}: {e}");
                        None
                    }
                },
                None if cli.continue_latest => match session::latest(&app.config_dir) {
                    Ok(Some(s)) => Some(s),
                    Ok(None) => {
                        eprintln!("no saved sessions to continue");
                        None
                    }
                    Err(e) => {
                        eprintln!("warning: failed to load the latest session: {e}");
                        None
                    }
                },
                None => None,
            };

            if cli.tui && !cfg!(feature = "tui") {
                eprintln!("--tui: this build has no `tui` feature; using the line REPL");
            }

            #[cfg(feature = "tui")]
            {
                let interactive_tty =
                    std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
                if cli.tui && !interactive_tty {
                    eprintln!("warning: --tui needs an interactive terminal; using the line REPL");
                }
                let use_tui = !cli.no_tui && interactive_tty;
                if use_tui {
                    // Bare `-r` opens the session picker instead of auto-resuming.
                    let pick = bare_resume && !cli.continue_latest;
                    let (perm, rx) = crate::permission::tui::TuiPermission::new();
                    return tui::run_tui(&app, perm, rx, initial, pick).await;
                }
            }

            // No TUI: bare `-r` / `--continue` resume the most recent session.
            let initial = if initial.is_none() && (bare_resume || cli.continue_latest) {
                session::latest(&app.config_dir).ok().flatten()
            } else {
                initial
            };
            interactive::run_interactive(&app, permission, initial).await
        }
    }
}

fn run_sessions_cmd(app: &AppConfig, action: SessionAction) -> anyhow::Result<()> {
    match action {
        SessionAction::List => {
            let summaries = session::list(&app.config_dir)?;
            if summaries.is_empty() {
                eprintln!("(no saved sessions)");
                return Ok(());
            }
            for s in summaries {
                let first = s
                    .first_message
                    .replace('\n', " ")
                    .chars()
                    .take(70)
                    .collect::<String>();
                println!("{}\t{}\t{}\t{}", s.id, s.model, s.turns, first);
            }
            Ok(())
        }
        SessionAction::Show { id } => {
            let s = session::load(&app.config_dir, &id)?;
            println!("{}", serde_json::to_string_pretty(&s)?);
            Ok(())
        }
        SessionAction::Delete { id } => {
            let path = session::sessions_dir(&app.config_dir).join(format!("{id}.json"));
            std::fs::remove_file(&path)?;
            eprintln!("deleted {}", path.display());
            Ok(())
        }
        SessionAction::Import { path } => {
            let mut imported = session::import(Path::new(&path))?;
            let saved = session::save(&app.config_dir, &mut imported)?;
            eprintln!(
                "imported {} message(s) from {path}",
                imported.messages.len()
            );
            eprintln!("saved as {}", saved.display());
            println!("{}", imported.id);
            Ok(())
        }
        SessionAction::Export { id, to } => {
            let s = session::load(&app.config_dir, &id)?;
            let jsonl = session::export_jsonl(&s)?;
            let path = to
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(format!("{id}.jsonl")));
            std::fs::write(&path, jsonl)?;
            eprintln!(
                "exported {} message(s) to {}",
                s.messages.len(),
                path.display()
            );
            Ok(())
        }
    }
}

/// Load a session by id (local store) or by file path (native JSON or an
/// upstream `.jsonl`).
fn load_session_target(config_dir: &Path, target: &str) -> anyhow::Result<session::Session> {
    let path = Path::new(target);
    if path.is_file() {
        session::import(path)
    } else {
        session::load(config_dir, target)
    }
}
