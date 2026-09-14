//! Interactive (REPL) mode: read user lines from stdin, run agent turns,
//! print streaming output, and dispatch slash commands.

use std::io::{BufRead, Write};
use std::sync::Arc;

use futures::StreamExt;
use pi_agent::{
    run_agent_with_history, tools::default_tools, AgentConfig, AgentEvent, PermissionPolicy,
};
use pi_ai::{AssistantMessageEvent, Context, Message, StreamOptions};
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::session::Session;
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
    if !session.messages.is_empty() {
        eprintln!(
            "(resumed session {}, {} prior messages)",
            session.id,
            session.messages.len()
        );
    }

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let system_prompt = build_system_prompt(&app.config_dir);

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
            if prompt == "/compact" || prompt.starts_with("/compact ") {
                if let Err(e) = handle_compact(app, &mut session).await {
                    eprintln!("compact failed: {e}");
                }
                continue;
            }
            if !handle_slash(&prompt, app, &mut session)? {
                break;
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
        let user = Message::user_text(prompt);
        let mut history = session.messages.clone();
        history.push(user);

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
        session.replace_messages(res.messages);
        if let Err(e) = crate::session::save(&app.config_dir, &session) {
            eprintln!("(warning: session save failed: {e})");
        }
    }
    Ok(())
}

/// Returns `false` if the loop should exit (e.g. `/quit`).
fn handle_slash(line: &str, app: &AppConfig, session: &mut Session) -> anyhow::Result<bool> {
    let (cmd, rest) = match line.split_once(' ') {
        Some((c, r)) => (c, r.trim()),
        None => (line, ""),
    };
    match cmd {
        "/quit" | "/exit" => return Ok(false),
        "/help" => {
            eprintln!("/quit /exit          quit pi");
            eprintln!("/help                show this help");
            eprintln!(
                "/reset               clear in-memory transcript (does not delete session file)"
            );
            eprintln!("/model               print current model");
            eprintln!("/tools               list builtin tools");
            eprintln!("/cost                print accumulated cost/usage so far");
            eprintln!("/sessions            list saved sessions");
            eprintln!("/resume <id>         load a saved session by id");
            eprintln!("/session             print current session id");
            eprintln!("/compact             summarize older messages into a recap");
        }
        "/reset" => {
            *session = Session::new(&app.model);
            eprintln!("(reset; new session id {})", session.id);
        }
        "/model" => {
            eprintln!("model: {} ({})", app.model.name, app.model.provider);
        }
        "/tools" => {
            for t in default_tools() {
                eprintln!("- {}: {}", t.name(), t.description());
            }
        }
        "/cost" => {
            let mut total_in = 0u64;
            let mut total_out = 0u64;
            let mut total_cost = 0.0f64;
            for m in &session.messages {
                if let Message::Assistant(a) = m {
                    total_in += a.usage.input;
                    total_out += a.usage.output;
                    total_cost += a.usage.cost.total;
                }
            }
            eprintln!("tokens: in={total_in} out={total_out}  cost: ${total_cost:.4}");
        }
        "/sessions" => {
            let summaries = crate::session::list(&app.config_dir)?;
            if summaries.is_empty() {
                eprintln!("(no saved sessions)");
            }
            for s in summaries.iter().take(20) {
                let first = truncate(&s.first_message, 60);
                eprintln!("{}  ({} msgs, {})  {}", s.id, s.turns, s.model, first);
            }
        }
        "/session" => {
            eprintln!("{}", session.id);
        }
        "/resume" => {
            if rest.is_empty() {
                eprintln!("usage: /resume <id>");
            } else {
                match crate::session::load(&app.config_dir, rest) {
                    Ok(s) => {
                        eprintln!("loaded session {} ({} messages)", s.id, s.messages.len());
                        *session = s;
                    }
                    Err(e) => eprintln!("load failed: {e}"),
                }
            }
        }
        other => {
            eprintln!("unknown command: {other} — try /help");
        }
    }
    Ok(true)
}

/// Summarize all but the last 4 messages into a single synthetic user
/// message, replacing the older slice in-place. Prints `(nothing to compact)`
/// if fewer than 4 messages exist.
async fn handle_compact(app: &AppConfig, session: &mut Session) -> anyhow::Result<()> {
    let total = session.messages.len();
    if total < 4 {
        eprintln!("(nothing to compact)");
        return Ok(());
    }
    let keep_from = total - 4;
    let older: Vec<Message> = session.messages[..keep_from].to_vec();
    let older_count = older.len();

    let ctx = Context {
        system_prompt: Some(
            "Summarize this conversation into a compact context-preserving recap. \
             Include files mentioned, decisions, and open todos."
                .into(),
        ),
        messages: older,
        tools: Vec::new(),
    };

    let options = StreamOptions {
        api_key: app.api_key.clone(),
        ..Default::default()
    };
    let mut stream = pi_ai::stream_simple(&app.model, &ctx, &options).await?;
    let mut summary = String::new();
    while let Some(event) = stream.next().await {
        if let AssistantMessageEvent::TextDelta { delta, .. } = event? {
            summary.push_str(&delta);
        }
    }

    let recap = Message::user_text(format!("[compacted summary]\n{summary}"));
    let mut new_messages = Vec::with_capacity(5);
    new_messages.push(recap);
    new_messages.extend(session.messages.drain(keep_from..));
    session.replace_messages(new_messages);
    crate::session::save(&app.config_dir, session)?;
    eprintln!("compacted {older_count} messages");
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        let head: String = s.chars().take(n).collect();
        format!("{head}…")
    }
}
