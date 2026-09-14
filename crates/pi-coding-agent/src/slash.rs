//! Slash-command handling shared by the line REPL and the TUI.
//!
//! Commands mutate the [`Session`] in place and return the lines that should be
//! shown to the user, so each front-end can render them its own way.

use futures::StreamExt;
use pi_ai::{Context, Message, StreamOptions};

use crate::config::AppConfig;
use crate::session::Session;

/// Result of dispatching one slash command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SlashAction {
    #[default]
    None,
    /// Open the tree overlay (branch switch).
    Tree,
    /// Open the fork overlay (branch off a previous user message).
    Fork,
    /// A new session was started; front-ends should reset their view.
    New,
}

pub struct SlashOutcome {
    /// `false` when the front-end should exit (e.g. `/quit`).
    pub keep_going: bool,
    /// Lines to display. May be empty.
    pub output: Vec<String>,
    /// A front-end action requested by the command.
    pub action: SlashAction,
}

impl SlashOutcome {
    fn keep(output: Vec<String>) -> Self {
        Self {
            keep_going: true,
            output,
            action: SlashAction::None,
        }
    }

    fn action(output: Vec<String>, action: SlashAction) -> Self {
        Self {
            keep_going: true,
            output,
            action,
        }
    }

    fn quit() -> Self {
        Self {
            keep_going: false,
            output: Vec::new(),
            action: SlashAction::None,
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
        "/quit" | "/exit" => SlashOutcome::quit(),
        "/help" => SlashOutcome::keep(vec![
            "/quit /exit          quit pi".into(),
            "/help                show this help".into(),
            "/new                 start a new session".into(),
            "/reset               start a new session (alias of /new)".into(),
            "/model               print current model".into(),
            "/tools               list builtin tools".into(),
            "/cost                print accumulated cost/usage so far".into(),
            "/sessions            list saved sessions".into(),
            "/resume <id>         load a saved session by id".into(),
            "/session             print current session id".into(),
            "/tree                switch to an earlier point in the session tree".into(),
            "/fork                start a new session from a previous user message".into(),
            "/clone               duplicate the active branch into a new session".into(),
            "/compact             summarize older messages into a recap".into(),
        ]),
        "/new" => {
            *session = Session::new(&app.model);
            SlashOutcome::action(
                vec![format!("(new session {})", session.id)],
                SlashAction::New,
            )
        }
        "/reset" => {
            *session = Session::new(&app.model);
            SlashOutcome::action(
                vec![format!("(reset; new session {})", session.id)],
                SlashAction::New,
            )
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
            for m in &session.messages() {
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
        "/tree" => SlashOutcome::action(
            vec!["(select a point to branch from)".into()],
            SlashAction::Tree,
        ),
        "/fork" => SlashOutcome::action(
            vec!["(select a user message to fork from)".into()],
            SlashAction::Fork,
        ),
        "/clone" => {
            let mut new = Session::new(&app.model);
            new.replace_messages(session.messages());
            let id = new.id.clone();
            *session = new;
            SlashOutcome::keep(vec![format!("cloned active branch into new session {id}")])
        }
        "/resume" => {
            if rest.is_empty() {
                SlashOutcome::keep(vec!["usage: /resume <id>".into()])
            } else {
                match crate::session::load(&app.config_dir, rest) {
                    Ok(s) => {
                        let msg =
                            format!("loaded session {} ({} messages)", s.id, s.branch().len());
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
    let messages = session.messages();
    let total = messages.len();
    if total < 4 {
        return Ok("(nothing to compact)".into());
    }
    let keep_from = total - 4;
    let older: Vec<Message> = messages[..keep_from].to_vec();
    let older_count = older.len();

    let summary = summarize(app, older).await?;

    let recap = Message::user_text(format!("[compacted summary]\n{summary}"));
    let mut new_messages = Vec::with_capacity(5);
    new_messages.push(recap);
    new_messages.extend(messages[keep_from..].iter().cloned());
    session.replace_messages(new_messages);
    crate::session::save(&app.config_dir, session)?;
    Ok(format!("compacted {older_count} messages"))
}

/// Summarize a set of messages into a context-preserving recap. Used by
/// `/compact` and for branch summaries when switching branches with `/tree`.
pub async fn summarize(app: &AppConfig, messages: Vec<Message>) -> anyhow::Result<String> {
    let ctx = Context {
        system_prompt: Some(
            "Summarize this conversation into a compact context-preserving recap. \
             Include files mentioned, decisions, and open todos."
                .into(),
        ),
        messages,
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
    Ok(summary)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;

    fn app() -> AppConfig {
        AppConfig::default()
    }

    #[test]
    fn clone_duplicates_active_branch() {
        let app = app();
        let mut session = Session::new(&app.model);
        session.push_message(Message::user_text("one"));
        let original_id = session.id.clone();

        let outcome = handle("/clone", &app, &mut session).unwrap();
        assert_ne!(session.id, original_id);
        assert_eq!(session.messages().len(), 1);
        assert!(outcome.output[0].contains("cloned"));
    }

    #[test]
    fn tree_and_fork_request_actions() {
        let app = app();
        let mut session = Session::new(&app.model);
        assert_eq!(
            handle("/tree", &app, &mut session).unwrap().action,
            SlashAction::Tree
        );
        assert_eq!(
            handle("/fork", &app, &mut session).unwrap().action,
            SlashAction::Fork
        );
        assert_eq!(
            handle("/help", &app, &mut session).unwrap().action,
            SlashAction::None
        );
    }

    #[test]
    fn new_and_reset_start_fresh_sessions() {
        let app = app();
        for cmd in ["/new", "/reset"] {
            let mut session = Session::new(&app.model);
            session.push_message(Message::user_text("old"));
            let old_id = session.id.clone();
            let outcome = handle(cmd, &app, &mut session).unwrap();
            assert_eq!(outcome.action, SlashAction::New);
            assert_ne!(session.id, old_id);
            assert!(session.is_empty());
        }
    }
}
