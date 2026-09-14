//! One-shot "print" mode: run a single prompt to completion and print the
//! result. Streams text deltas to stdout as they arrive.
//!
//! Two output formats:
//! - human (default): assistant text streams to stdout; tool activity goes to stderr.
//! - JSON-lines (`--json`): one JSON object per line on stdout for every
//!   significant agent event. Stable contract for scripting.

use std::io::Write;
use std::sync::Arc;

use pi_agent::{run_agent, tools::default_tools, AgentConfig, AgentEvent, PermissionPolicy};
use pi_ai::{Content, Message};
use serde_json::json;
use tokio::sync::mpsc;

use crate::config::AppConfig;
use crate::system_prompt::build_system_prompt;

pub async fn run_print(
    app: &AppConfig,
    prompt: String,
    permission: Arc<dyn PermissionPolicy>,
    json_mode: bool,
) -> anyhow::Result<()> {
    let cfg = AgentConfig::new(app.model.clone(), build_system_prompt(&app.config_dir))
        .with_tools(default_tools())
        .with_max_turns(app.max_turns)
        .with_thinking(app.thinking_level)
        .with_api_key(app.api_key.clone())
        .with_permission(permission);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let user = Message::user_text(prompt);

    let cfg_cloned = cfg.clone();
    let handle = tokio::spawn(async move { run_agent(&cfg_cloned, user, Some(tx)).await });

    if json_mode {
        run_json(&mut rx).await;
    } else {
        run_human(&mut rx).await;
    }

    let res = handle.await??;
    if json_mode {
        emit_json(&json!({
            "type": "agent_end",
            "stopped_at_turn_limit": res.stopped_at_turn_limit,
            "message_count": res.messages.len(),
        }));
    } else if res.stopped_at_turn_limit {
        eprintln!("(stopped at max turns)");
    }
    Ok(())
}

async fn run_human(rx: &mut mpsc::UnboundedReceiver<AgentEvent>) {
    let mut stdout = std::io::stdout();
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
                eprintln!("→ {tool_name}({args})");
            }
            AgentEvent::ToolExecutionEnd {
                tool_name,
                is_error,
                ..
            } => {
                eprintln!("← {} {}", tool_name, if is_error { "error" } else { "ok" });
            }
            AgentEvent::PermissionDenied { tool_name, reason } => {
                eprintln!("✗ permission denied for {tool_name}: {reason}");
            }
            _ => {}
        }
    }
}

async fn run_json(rx: &mut mpsc::UnboundedReceiver<AgentEvent>) {
    while let Some(ev) = rx.recv().await {
        let value = event_to_json(&ev);
        if !value.is_null() {
            emit_json(&value);
        }
    }
}

fn event_to_json(ev: &AgentEvent) -> serde_json::Value {
    match ev {
        AgentEvent::AgentStart => json!({"type": "agent_start"}),
        AgentEvent::TurnStart => json!({"type": "turn_start"}),
        AgentEvent::TurnEnd => json!({"type": "turn_end"}),
        AgentEvent::UserMessage { message } => {
            json!({"type": "user_message", "message": message})
        }
        AgentEvent::AssistantMessage { message } => {
            let mut text = String::new();
            if let Message::Assistant(a) = message {
                for c in &a.content {
                    if let Content::Text { text: t } = c {
                        text.push_str(t);
                    }
                }
            }
            json!({"type": "assistant_message", "text": text, "message": message})
        }
        AgentEvent::TextDelta { delta } => {
            json!({"type": "text_delta", "delta": delta})
        }
        AgentEvent::ThinkingDelta { delta } => {
            json!({"type": "thinking_delta", "delta": delta})
        }
        AgentEvent::ToolExecutionStart {
            tool_call_id,
            tool_name,
            args,
        } => json!({
            "type": "tool_start",
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "args": args,
        }),
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            tool_name,
            is_error,
            content,
        } => json!({
            "type": "tool_end",
            "tool_call_id": tool_call_id,
            "tool_name": tool_name,
            "is_error": is_error,
            "content": content,
        }),
        AgentEvent::PermissionDenied { tool_name, reason } => json!({
            "type": "permission_denied",
            "tool_name": tool_name,
            "reason": reason,
        }),
        // AgentEnd is emitted by run_print after the channel closes so we know
        // the final state (stopped_at_turn_limit, etc.).
        AgentEvent::AgentEnd { .. } => serde_json::Value::Null,
    }
}

fn emit_json(value: &serde_json::Value) {
    let line = match serde_json::to_string(value) {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{line}");
    let _ = stdout.flush();
}
