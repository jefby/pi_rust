//! Session persistence: save transcripts as JSON under
//! `$XDG_CONFIG_HOME/pi/sessions/<id>.json`, list them, and load by id.

use std::path::{Path, PathBuf};

use anyhow::Context;
use pi_ai::{AssistantMessage, Content, Cost, Message, StopReason, ToolResultMessage, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub model: String,
    pub provider: String,
    pub messages: Vec<Message>,
}

impl Session {
    pub fn new(model: &pi_ai::Model) -> Self {
        let now = pi_ai::now_ms();
        Self {
            id: new_id(),
            created_ms: now,
            updated_ms: now,
            model: model.id.clone(),
            provider: model.provider.clone(),
            messages: Vec::new(),
        }
    }

    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.updated_ms = pi_ai::now_ms();
    }
}

pub fn sessions_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("sessions")
}

pub fn save(config_dir: &Path, session: &Session) -> anyhow::Result<PathBuf> {
    let dir = sessions_dir(config_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    let path = dir.join(format!("{}.json", session.id));
    let json = serde_json::to_string_pretty(session)?;
    std::fs::write(&path, json).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

pub fn load(config_dir: &Path, id: &str) -> anyhow::Result<Session> {
    let path = sessions_dir(config_dir).join(format!("{id}.json"));
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let s: Session = serde_json::from_str(&text)?;
    Ok(s)
}

/// Load the most recently updated session, if any.
pub fn latest(config_dir: &Path) -> anyhow::Result<Option<Session>> {
    match list(config_dir)?.first() {
        Some(summary) => Ok(Some(load(config_dir, &summary.id)?)),
        None => Ok(None),
    }
}

pub fn list(config_dir: &Path) -> anyhow::Result<Vec<SessionSummary>> {
    let dir = sessions_dir(config_dir);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut out: Vec<SessionSummary> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let text = std::fs::read_to_string(&path)?;
        let s: Session = match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let first_user = s
            .messages
            .iter()
            .find_map(|m| match m {
                Message::User { content, .. } => content
                    .iter()
                    .find_map(|c| c.as_text().map(|s| s.to_string())),
                _ => None,
            })
            .unwrap_or_default();
        out.push(SessionSummary {
            id: s.id,
            updated_ms: s.updated_ms,
            model: s.model,
            provider: s.provider,
            first_message: first_user,
            turns: s.messages.len(),
        });
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.updated_ms));
    Ok(out)
}

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub updated_ms: i64,
    pub model: String,
    #[allow(dead_code)] // exposed for callers, not yet rendered.
    pub provider: String,
    pub first_message: String,
    pub turns: usize,
}

// ---------------------------------------------------------------------------
// Interop with the upstream (`@earendil-works/pi`) JSONL session format.
//
// Upstream stores sessions as JSONL under
// `~/.pi/agent/sessions/--<cwd>--/<timestamp>_<uuid>.jsonl`. Each line is a
// JSON object; message entries carry `id` / `parentId` and form a tree. We read
// the active branch (last entry back to the root) and flatten it into our
// linear transcript. Export writes the reverse shape to a new file.
// ---------------------------------------------------------------------------

/// Load either a native session JSON file or an upstream `.jsonl` session.
pub fn import(path: &Path) -> anyhow::Result<Session> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("read {}", path.display()))?;
            Ok(serde_json::from_str(&text)?)
        }
        _ => import_jsonl(path),
    }
}

/// Parse an upstream `.jsonl` session into a [`Session`].
pub fn import_jsonl(path: &Path) -> anyhow::Result<Session> {
    struct Entry {
        id: String,
        parent: Option<String>,
        timestamp: Option<String>,
        message: Option<Value>,
    }

    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;

    let mut entries: Vec<Entry> = Vec::new();
    let mut header_id: Option<String> = None;
    let mut header_ts: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("parse JSONL entry in {}", path.display()))?;
        if value.get("type").and_then(Value::as_str) == Some("session") {
            header_id = value.get("id").and_then(Value::as_str).map(String::from);
            header_ts = value
                .get("timestamp")
                .and_then(Value::as_str)
                .map(String::from);
            continue;
        }
        let Some(id) = value.get("id").and_then(Value::as_str).map(String::from) else {
            continue;
        };
        entries.push(Entry {
            id,
            parent: value
                .get("parentId")
                .and_then(Value::as_str)
                .map(String::from),
            timestamp: value
                .get("timestamp")
                .and_then(Value::as_str)
                .map(String::from),
            message: value.get("message").cloned(),
        });
    }

    // Walk the active branch: last entry back to the root via `parentId`.
    let index: std::collections::HashMap<&str, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.as_str(), i))
        .collect();
    let mut chain: Vec<usize> = Vec::new();
    let mut cursor = entries.len().checked_sub(1);
    let mut guard = 0usize;
    while let Some(i) = cursor {
        chain.push(i);
        guard += 1;
        if guard > entries.len() {
            break; // cycle guard
        }
        cursor = entries[i]
            .parent
            .as_deref()
            .and_then(|parent| index.get(parent).copied());
    }
    chain.reverse();

    let messages: Vec<Message> = chain
        .into_iter()
        .filter_map(|i| entries[i].message.as_ref().and_then(convert_message))
        .collect();

    let (model, provider) = messages
        .iter()
        .rev()
        .find_map(|m| match m {
            Message::Assistant(a) => Some((a.model.clone(), a.provider.clone())),
            _ => None,
        })
        .unwrap_or_default();

    Ok(Session {
        id: header_id.unwrap_or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "imported".into())
        }),
        created_ms: header_ts
            .as_deref()
            .map(parse_ts_str)
            .unwrap_or_else(pi_ai::now_ms),
        updated_ms: entries
            .last()
            .and_then(|e| e.timestamp.as_deref())
            .map(parse_ts_str)
            .unwrap_or_else(pi_ai::now_ms),
        model,
        provider,
        messages,
    })
}

fn convert_message(message: &Value) -> Option<Message> {
    let role = message.get("role").and_then(Value::as_str)?;
    let timestamp = parse_ts(message.get("timestamp"));
    match role {
        "user" => Some(Message::User {
            content: convert_blocks(message.get("content")),
            timestamp,
        }),
        "assistant" => Some(Message::Assistant(AssistantMessage {
            content: convert_blocks(message.get("content")),
            api: str_field(message, "api"),
            provider: str_field(message, "provider"),
            model: str_field(message, "model"),
            usage: convert_usage(message.get("usage")),
            stop_reason: convert_stop_reason(message.get("stopReason").and_then(Value::as_str)),
            error_message: message
                .get("errorMessage")
                .and_then(Value::as_str)
                .map(String::from),
            timestamp,
        })),
        "toolResult" => Some(Message::ToolResult(ToolResultMessage {
            tool_call_id: str_field(message, "toolCallId"),
            tool_name: str_field(message, "toolName"),
            content: convert_blocks(message.get("content")),
            is_error: message
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            timestamp,
        })),
        _ => None,
    }
}

fn convert_blocks(value: Option<&Value>) -> Vec<Content> {
    value
        .and_then(Value::as_array)
        .map(|blocks| blocks.iter().filter_map(convert_block).collect())
        .unwrap_or_default()
}

fn convert_block(block: &Value) -> Option<Content> {
    match block.get("type").and_then(Value::as_str)? {
        "text" => Some(Content::Text {
            text: str_field(block, "text"),
        }),
        "thinking" => Some(Content::Thinking {
            thinking: str_field(block, "thinking"),
            thinking_signature: block
                .get("thinkingSignature")
                .and_then(Value::as_str)
                .map(String::from),
        }),
        "toolCall" => Some(Content::ToolCall {
            id: str_field(block, "id"),
            name: str_field(block, "name"),
            arguments: block
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(Default::default())),
        }),
        "image" => Some(Content::Image {
            data: str_field(block, "data"),
            mime_type: str_field(block, "mimeType"),
        }),
        _ => None,
    }
}

fn convert_usage(value: Option<&Value>) -> Usage {
    let Some(value) = value else {
        return Usage::default();
    };
    let count = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0);
    let cost = value.get("cost");
    let usd = |key: &str| {
        cost.and_then(|c| c.get(key))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    };
    Usage {
        input: count("input"),
        output: count("output"),
        cache_read: count("cacheRead"),
        cache_write: count("cacheWrite"),
        total_tokens: count("totalTokens"),
        cost: Cost {
            input: usd("input"),
            output: usd("output"),
            cache_read: usd("cacheRead"),
            cache_write: usd("cacheWrite"),
            total: usd("total"),
        },
    }
}

fn convert_stop_reason(reason: Option<&str>) -> StopReason {
    match reason {
        Some("length") => StopReason::Length,
        Some("toolUse") | Some("tool_use") => StopReason::ToolUse,
        Some("error") => StopReason::Error,
        Some("aborted") => StopReason::Aborted,
        _ => StopReason::Stop,
    }
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn parse_ts(value: Option<&Value>) -> i64 {
    value
        .and_then(Value::as_str)
        .map(parse_ts_str)
        .unwrap_or_else(pi_ai::now_ms)
}

fn parse_ts_str(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(|_| pi_ai::now_ms())
}

fn iso(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Render a [`Session`] as upstream v3 JSONL. The caller decides where to
/// write it; nothing under the upstream agent directory is touched here.
pub fn export_jsonl(session: &Session) -> anyhow::Result<String> {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let header = serde_json::json!({
        "type": "session",
        "version": 3,
        "id": session.id,
        "timestamp": iso(session.created_ms),
        "cwd": cwd,
    });

    let mut out = String::new();
    out.push_str(&serde_json::to_string(&header)?);
    out.push('\n');

    let mut parent: Option<String> = None;
    for (i, message) in session.messages.iter().enumerate() {
        let id = format!("{:x}-{:04x}", session.created_ms.max(0), i);
        let entry = serde_json::json!({
            "type": "message",
            "id": id,
            "parentId": parent,
            "timestamp": iso(message_timestamp(message)),
            "message": export_message(message),
        });
        out.push_str(&serde_json::to_string(&entry)?);
        out.push('\n');
        parent = Some(id);
    }
    Ok(out)
}

fn message_timestamp(message: &Message) -> i64 {
    match message {
        Message::User { timestamp, .. } => *timestamp,
        Message::Assistant(a) => a.timestamp,
        Message::ToolResult(tr) => tr.timestamp,
    }
}

fn export_message(message: &Message) -> Value {
    match message {
        Message::User { content, timestamp } => serde_json::json!({
            "role": "user",
            "content": export_blocks(content),
            "timestamp": iso(*timestamp),
        }),
        Message::Assistant(a) => serde_json::json!({
            "role": "assistant",
            "content": export_blocks(&a.content),
            "api": a.api,
            "provider": a.provider,
            "model": a.model,
            "usage": export_usage(&a.usage),
            "stopReason": stop_reason_str(a.stop_reason),
            "timestamp": iso(a.timestamp),
        }),
        Message::ToolResult(tr) => serde_json::json!({
            "role": "toolResult",
            "toolCallId": tr.tool_call_id,
            "toolName": tr.tool_name,
            "content": export_blocks(&tr.content),
            "isError": tr.is_error,
            "timestamp": iso(tr.timestamp),
        }),
    }
}

fn export_blocks(content: &[Content]) -> Value {
    Value::Array(
        content
            .iter()
            .map(|c| match c {
                Content::Text { text } => serde_json::json!({"type": "text", "text": text}),
                Content::Thinking {
                    thinking,
                    thinking_signature,
                } => {
                    let mut value = serde_json::json!({"type": "thinking", "thinking": thinking});
                    if let Some(sig) = thinking_signature {
                        value["thinkingSignature"] = serde_json::json!(sig);
                    }
                    value
                }
                Content::ToolCall {
                    id,
                    name,
                    arguments,
                } => serde_json::json!({
                    "type": "toolCall", "id": id, "name": name, "arguments": arguments
                }),
                Content::Image { data, mime_type } => serde_json::json!({
                    "type": "image", "data": data, "mimeType": mime_type
                }),
            })
            .collect(),
    )
}

fn export_usage(usage: &Usage) -> Value {
    serde_json::json!({
        "input": usage.input,
        "output": usage.output,
        "cacheRead": usage.cache_read,
        "cacheWrite": usage.cache_write,
        "totalTokens": usage.total_tokens,
        "cost": {
            "input": usage.cost.input,
            "output": usage.cost.output,
            "cacheRead": usage.cost.cache_read,
            "cacheWrite": usage.cost.cache_write,
            "total": usage.cost.total,
        }
    })
}

fn stop_reason_str(reason: StopReason) -> &'static str {
    match reason {
        StopReason::Stop => "stop",
        StopReason::Length => "length",
        StopReason::ToolUse => "toolUse",
        StopReason::Error => "error",
        StopReason::Aborted => "aborted",
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pi-rs-session-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const SAMPLE: &[&str] = &[
        r#"{"type":"session","version":3,"id":"sess-1","timestamp":"2026-05-01T00:00:00.000Z","cwd":"/tmp"}"#,
        r#"{"type":"message","id":"a","parentId":null,"timestamp":"2026-05-01T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":"2026-05-01T00:00:01.000Z"}}"#,
        r#"{"type":"message","id":"b","parentId":"a","timestamp":"2026-05-01T00:00:02.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"pondering"},{"type":"text","text":"hello"},{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"x"}}],"api":"openai-completions","provider":"deepseek","model":"deepseek-v4-flash","usage":{"input":10,"output":2,"cacheRead":1,"cacheWrite":0,"totalTokens":12,"cost":{"input":0.1,"output":0.2,"cacheRead":0.01,"cacheWrite":0,"total":0.31}},"stopReason":"toolUse","timestamp":"2026-05-01T00:00:02.000Z"}}"#,
        r#"{"type":"message","id":"c","parentId":"b","timestamp":"2026-05-01T00:00:03.000Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"read","content":[{"type":"text","text":"contents"}],"isError":false,"timestamp":"2026-05-01T00:00:03.000Z"}}"#,
    ];

    #[test]
    fn imports_active_branch() {
        let dir = temp_dir("import");
        let path = dir.join("s.jsonl");
        std::fs::write(&path, SAMPLE.join("\n") + "\n").unwrap();

        let session = import_jsonl(&path).unwrap();
        assert_eq!(session.id, "sess-1");
        assert_eq!(session.provider, "deepseek");
        assert_eq!(session.model, "deepseek-v4-flash");
        assert_eq!(session.messages.len(), 3);
        match &session.messages[1] {
            Message::Assistant(a) => {
                assert_eq!(a.content.len(), 3);
                assert!(matches!(a.content[0], Content::Thinking { .. }));
                assert_eq!(a.usage.input, 10);
                assert_eq!(a.usage.cache_read, 1);
                assert_eq!(a.stop_reason, StopReason::ToolUse);
            }
            other => panic!("expected assistant, got {other:?}"),
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn export_round_trips_through_import() {
        let model = pi_ai::Model::openai_gpt_4o();
        let mut session = Session::new(&model);
        session.messages.push(Message::user_text("hi"));
        session.messages.push(Message::Assistant(AssistantMessage {
            content: vec![Content::text("yo")],
            api: "openai-completions".into(),
            provider: "openai".into(),
            model: "gpt-4o".into(),
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: pi_ai::now_ms(),
        }));

        let jsonl = export_jsonl(&session).unwrap();
        let dir = temp_dir("export");
        let path = dir.join("out.jsonl");
        std::fs::write(&path, jsonl).unwrap();

        let back = import_jsonl(&path).unwrap();
        assert_eq!(back.messages.len(), 2);
        assert_eq!(back.model, "gpt-4o");
        match &back.messages[0] {
            Message::User { content, .. } => assert_eq!(content[0].as_text(), Some("hi")),
            other => panic!("expected user, got {other:?}"),
        }
        std::fs::remove_dir_all(dir).ok();
    }
}

fn new_id() -> String {
    let now = pi_ai::now_ms();
    let suffix: u32 = rand_u32();
    format!("{now:x}-{suffix:08x}")
}

// Tiny xorshift PRNG seeded from time — we don't pull in `rand` just for this.
fn rand_u32() -> u32 {
    use std::cell::Cell;
    thread_local!(static STATE: Cell<u32> = const { Cell::new(0) });
    STATE.with(|s| {
        let mut x = s.get();
        if x == 0 {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            x = (now as u32) ^ 0x9E37_79B9;
        }
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        s.set(x);
        x
    })
}
