//! Slash-command handling shared by the line REPL and the TUI.
//!
//! Commands mutate the [`Session`] in place and return the lines that should be
//! shown to the user, so each front-end can render them its own way.

use futures::StreamExt;
use pi_ai::{Context, Message, StreamOptions};

use crate::config::AppConfig;
use crate::session::Session;

/// Result of dispatching one slash command.
pub struct SlashOutcome {
    /// `false` when the front-end should exit (e.g. `/quit`).
    pub keep_going: bool,
    /// Lines to display. May be empty.
    pub output: Vec<String>,
}

impl SlashOutcome {
    fn keep(output: Vec<String>) -> Self {
        Self {
            keep_going: true,
            output,
        }
    }
}

/// Handle `line` (which must start with `/`). Returns the text to display and
/// whether the loop should continue.
pub fn handle(line: &str, app: &AppConfig, session: &mut Session) -> anyhow::Result<SlashOutcome> {
    let (cmd, rest) = match line.split_once(' ') {
        Some((c, r)) => (c, r.trim()),
        None => (line, ""),
    };
    Ok(match cmd {
        "/quit" | "/exit" => SlashOutcome {
            keep_going: false,
            output: Vec::new(),
        },
        "/help" => SlashOutcome::keep(vec![
            "/quit /exit          quit pi".into(),
            "/help                show this help".into(),
            "/reset               clear in-memory transcript (does not delete session file)".into(),
            "/model               print current model".into(),
            "/tools               list builtin tools".into(),
            "/cost                print accumulated cost/usage so far".into(),
            "/sessions            list saved sessions".into(),
            "/resume <id>         load a saved session by id".into(),
            "/session             print current session id".into(),
            "/compact             summarize older messages into a recap".into(),
        ]),
        "/reset" => {
            *session = Session::new(&app.model);
            SlashOutcome::keep(vec![format!("(reset; new session id {})", session.id)])
        }
        "/model" => SlashOutcome::keep(vec![format!(
            "model: {} ({})",
            app.model.name, app.model.provider
        )]),
        "/tools" => {
            let mut out = Vec::new();
            for t in pi_agent::tools::default_tools() {
                out.push(format!("- {}: {}", t.name(), t.description()));
            }
            SlashOutcome::keep(out)
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
            SlashOutcome::keep(vec![format!(
                "tokens: in={total_in} out={total_out}  cost: ${total_cost:.4}"
            )])
        }
        "/sessions" => {
            let summaries = crate::session::list(&app.config_dir)?;
            if summaries.is_empty() {
                SlashOutcome::keep(vec!["(no saved sessions)".into()])
            } else {
                let out = summaries
                    .iter()
                    .take(20)
                    .map(|s| {
                        format!(
                            "{}  ({} msgs, {})  {}",
                            s.id,
                            s.turns,
                            s.model,
                            truncate(&s.first_message, 60)
                        )
                    })
                    .collect();
                SlashOutcome::keep(out)
            }
        }
        "/session" => SlashOutcome::keep(vec![session.id.clone()]),
        "/resume" => {
            if rest.is_empty() {
                SlashOutcome::keep(vec!["usage: /resume <id>".into()])
            } else {
                match crate::session::load(&app.config_dir, rest) {
                    Ok(s) => {
                        let msg =
                            format!("loaded session {} ({} messages)", s.id, s.messages.len());
                        *session = s;
                        SlashOutcome::keep(vec![msg])
                    }
                    Err(e) => SlashOutcome::keep(vec![format!("load failed: {e}")]),
                }
            }
        }
        other => SlashOutcome::keep(vec![format!("unknown command: {other} — try /help")]),
    })
}

/// Summarize all but the last 4 messages into a single synthetic user message.
pub async fn compact(app: &AppConfig, session: &mut Session) -> anyhow::Result<String> {
    let total = session.messages.len();
    if total < 4 {
        return Ok("(nothing to compact)".into());
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
        if let pi_ai::AssistantMessageEvent::TextDelta { delta, .. } = event? {
            summary.push_str(&delta);
        }
    }

    let recap = Message::user_text(format!("[compacted summary]\n{summary}"));
    let mut new_messages = Vec::with_capacity(5);
    new_messages.push(recap);
    new_messages.extend(session.messages.drain(keep_from..));
    session.replace_messages(new_messages);
    crate::session::save(&app.config_dir, session)?;
    Ok(format!("compacted {older_count} messages"))
}

/// Truncate `s` to `n` characters, collapsing newlines.
pub fn truncate(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        let head: String = s.chars().take(n).collect();
        format!("{head}…")
    }
}
