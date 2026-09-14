//! `pi` — interactive coding agent CLI.

mod config;
mod file_config;
mod interactive;
mod permission;
mod pi_agent_config;
mod print_mode;
mod project;
mod session;
mod system_prompt;

use std::sync::Arc;

use clap::{Parser, Subcommand};

use crate::config::{parse_thinking_level, resolve_model, AppConfig};
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

    /// Resume a saved session by id.
    #[arg(long)]
    resume: Option<String>,

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
    let resolved = resolve_model(explicit_model.as_deref(), agent_home.as_ref());
    tracing::debug!(
        provider = %resolved.model.provider,
        model = %resolved.model.id,
        api = %resolved.model.api,
        base_url = %resolved.model.base_url,
        has_api_key = resolved.api_key.is_some(),
        "resolved model"
    );

    // CLI flags / env win; the files fill holes.
    let max_turns = cli.max_turns.or(file_cfg.max_turns).unwrap_or(32);
    let env_thinking = std::env::var("PI_REASONING_LEVEL").ok();
    let thinking_level = file_cfg
        .thinking_level
        .as_deref()
        .or(env_thinking.as_deref())
        .or_else(|| {
            agent_home
                .as_ref()
                .and_then(|h| h.settings.default_thinking_level.as_deref())
        })
        .and_then(parse_thinking_level)
        .unwrap_or_default();
    let yolo = cli.yolo || file_cfg.yolo;
    let json = cli.json || file_cfg.json;

    let app = AppConfig {
        model: resolved.model,
        api_key: resolved.api_key,
        max_turns,
        thinking_level,
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

    match (cli.prompt, cli.resume) {
        (Some(p), _) => print_mode::run_print(&app, p, permission, json).await,
        (None, resume_id) => {
            let initial = match resume_id {
                Some(id) => match session::load(&app.config_dir, &id) {
                    Ok(s) => Some(s),
                    Err(e) => {
                        eprintln!("warning: failed to load session {id}: {e}");
                        None
                    }
                },
                None => None,
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
    }
}
