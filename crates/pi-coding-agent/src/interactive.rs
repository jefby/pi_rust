//! Interactive (REPL) mode: read user lines from stdin, run agent turns,
//! print streaming output, and dispatch slash commands.

use std::io::{BufRead, Write};
use std::sync::Arc;

use pi_agent::{
    run_agent_with_history, tools::default_tools, AgentConfig, AgentEvent, PermissionPolicy,
};
use pi_ai::Message;
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::session::Session;
use crate::slash;
use crate::system_prompt::build_system_prompt;

pub async fn run_interactive(
    app: &AppConfig,
    permission: Arc<dyn PermissionPolicy>,
    initial: Option<Session>,
) -> anyhow::Result<()> {
    eprintln!(
        "pi — model: {} ({})  •  slash commands: /help",
        app.model.name, app.model.provider
    );

    let mut session = initial.unwrap_or_else(|| Session::new(&app.model));
    if !session.is_empty() {
        eprintln!(
            "(resumed session {}, {} prior messages)",
            session.id,
            session.branch().len()
        );
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let system_prompt = build_system_prompt(app);

    loop {
        write!(stdout, "\n> ")?;
        stdout.flush()?;
        let mut line = String::new();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let prompt = line.trim().to_string();
        if prompt.is_empty() {
            continue;
        }
        if prompt.starts_with('/') {
            if is_compact(&prompt) {
                match slash::compact(app, &mut session).await {
                    Ok(msg) => eprintln!("{msg}"),
                    Err(e) => eprintln!("compact failed: {e}"),
                }
                continue;
            }
            let outcome = slash::handle(&prompt, app, &mut session)?;
            for l in &outcome.output {
                eprintln!("{l}");
            }
            if outcome.action != slash::SlashAction::None {
                eprintln!("(/tree and /fork need the full-screen TUI; use --features tui)");
            }
            if !outcome.keep_going {
                break;
            }
            // Persist commands that changed the session (/clone, /resume, ...).
            if let Err(e) = crate::session::save(&app.config_dir, &mut session) {
                eprintln!("(warning: session save failed: {e})");
            }
            continue;
        }

        let cfg = AgentConfig::new(app.model.clone(), system_prompt.clone())
            .with_tools(default_tools())
            .with_max_turns(app.max_turns)
            .with_thinking(app.thinking_level)
            .with_api_key(app.api_key.clone())
            .with_permission(permission.clone());
        let (tx, mut rx) = mpsc::unbounded_channel();
        // Persist the user message before the turn so an interrupt still leaves
        // a resumable session on disk.
        session.push_message(Message::user_text(prompt));
        if let Err(e) = crate::session::save(&app.config_dir, &mut session) {
            eprintln!("(warning: session save failed: {e})");
        }
        let history = session.messages();
        let branch_len = history.len();

        let cfg_cloned = cfg.clone();
        let handle =
            tokio::spawn(
                async move { run_agent_with_history(&cfg_cloned, history, Some(tx)).await },
            );

        while let Some(ev) = rx.recv().await {
            match ev {
                AgentEvent::TextDelta { delta } => {
                    let _ = write!(stdout, "{delta}");
                    let _ = stdout.flush();
                }
                AgentEvent::AssistantMessage { .. } => {
                    let _ = writeln!(stdout);
                }
                AgentEvent::ToolExecutionStart {
                    tool_name, args, ..
                } => {
                    eprintln!("  → {}({})", tool_name, args);
                }
                AgentEvent::ToolExecutionEnd {
                    tool_name,
                    is_error,
                    ..
                } => {
                    eprintln!(
                        "  ← {} {}",
                        tool_name,
                        if is_error { "error" } else { "ok" }
                    );
                }
                AgentEvent::PermissionDenied { tool_name, reason } => {
                    eprintln!("  ✗ {tool_name} denied: {reason}");
                }
                _ => {}
            }
        }
        let res = handle.await??;
        if let Some(new_messages) = res.messages.get(branch_len..) {
            session.append_messages(new_messages);
        }
        if let Err(e) = crate::session::save(&app.config_dir, &mut session) {
            eprintln!("(warning: session save failed: {e})");
        }
    }
    Ok(())
}

fn is_compact(prompt: &str) -> bool {
    prompt == "/compact" || prompt.starts_with("/compact ")
}
