//! Session persistence: tree-structured transcripts stored as JSON under
//! `$XDG_CONFIG_HOME/pi/sessions/<id>.json`, plus import/export of the
//! upstream (`@earendil-works/pi`) JSONL tree format.
//!
//! Every message is an [`Entry`] with an `id` and optional `parent_id`; the
//! active conversation is the path from the root to `active_leaf`. New messages
//! are appended as children of the active leaf, so branching only requires
//! moving the leaf.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use pi_ai::{AssistantMessage, Content, Cost, Message, StopReason, ToolResultMessage, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One node of a session tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub timestamp: i64,
    /// Present for message entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// Original JSON for non-message entries (`model_change`, `custom`, ...),
    /// kept so a round trip does not lose them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub model: String,
    pub provider: String,
    #[serde(default)]
    pub entries: Vec<Entry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_leaf: Option<String>,
}

/// On-disk shape used before sessions became trees.
#[derive(Deserialize)]
struct LegacySession {
    id: String,
    created_ms: i64,
    updated_ms: i64,
    model: String,
    provider: String,
    #[serde(default)]
    messages: Vec<Message>,
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
            entries: Vec::new(),
            active_leaf: None,
        }
    }

    /// Parse a native session file, migrating the pre-tree `messages` format.
    pub fn from_json(text: &str) -> anyhow::Result<Self> {
        let value: Value = serde_json::from_str(text)?;
        if value.get("entries").is_some() {
            Ok(serde_json::from_value(value)?)
        } else {
            let legacy: LegacySession = serde_json::from_value(value)?;
            let mut session = Session {
                id: legacy.id,
                created_ms: legacy.created_ms,
                updated_ms: legacy.updated_ms,
                model: legacy.model,
                provider: legacy.provider,
                entries: Vec::new(),
                active_leaf: None,
            };
            for message in legacy.messages {
                session.push_message(message);
            }
            Ok(session)
        }
    }

    pub fn to_json(&self) -> anyhow::Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// Entries from the root to `id` (inclusive).
    pub fn branch_to(&self, id: &str) -> Vec<&Entry> {
        let index: HashMap<&str, &Entry> =
            self.entries.iter().map(|e| (e.id.as_str(), e)).collect();
        let mut chain = Vec::new();
        let mut cursor = Some(id.to_string());
        let mut guard = 0usize;
        while let Some(current) = cursor {
            let Some(entry) = index.get(current.as_str()).copied() else {
                break;
            };
            chain.push(entry);
            cursor = entry.parent_id.clone();
            guard += 1;
            if guard > self.entries.len() {
                break; // cycle guard
            }
        }
        chain.reverse();
        chain
    }

    /// Entries on the active branch (root → active leaf).
    pub fn branch(&self) -> Vec<&Entry> {
        let leaf = self
            .active_leaf
            .clone()
            .or_else(|| self.entries.last().map(|e| e.id.clone()));
        match leaf {
            Some(id) => self.branch_to(&id),
            None => Vec::new(),
        }
    }

    /// Active branch as a flat transcript (message entries only).
    pub fn messages(&self) -> Vec<Message> {
        self.branch()
            .into_iter()
            .filter_map(|e| e.message.clone())
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append `message` as a child of the active leaf and move the leaf to it.
    pub fn push_message(&mut self, message: Message) -> String {
        let id = new_entry_id();
        self.entries.push(Entry {
            id: id.clone(),
            parent_id: self.active_leaf.clone(),
            timestamp: pi_ai::now_ms(),
            message: Some(message),
            raw: None,
        });
        self.active_leaf = Some(id.clone());
        self.updated_ms = pi_ai::now_ms();
        id
    }

    pub fn append_messages(&mut self, messages: &[Message]) {
        for message in messages {
            self.push_message(message.clone());
        }
    }

    /// Rebuild the session as a fresh linear branch (used by `/compact`, `/reset`).
    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.entries.clear();
        self.active_leaf = None;
        for message in messages {
            self.push_message(message);
        }
    }

    /// Move the active leaf (branch switch). Used by the `tui` feature.
    #[allow(dead_code)]
    pub fn set_active_leaf(&mut self, id: Option<String>) {
        self.active_leaf = id;
        self.updated_ms = pi_ai::now_ms();
    }

    /// Messages on the branch from `from_leaf` that are not shared with the
    /// branch to `to_leaf` — the segment abandoned by a branch switch. Used by
    /// the `tui` feature.
    #[allow(dead_code)]
    pub fn abandoned(&self, from_leaf: Option<&str>, to_leaf: Option<&str>) -> Vec<Message> {
        let from = from_leaf.map(|id| self.branch_to(id)).unwrap_or_default();
        let to = to_leaf.map(|id| self.branch_to(id)).unwrap_or_default();
        let mut i = 0;
        while i < from.len() && i < to.len() && from[i].id == to[i].id {
            i += 1;
        }
        from[i..].iter().filter_map(|e| e.message.clone()).collect()
    }

    /// Label items for the `/tree` overlay. Used by the `tui` feature.
    #[allow(dead_code)]
    pub fn tree_items(&self) -> Vec<TreeItem> {
        let mut depth: HashMap<&str, usize> = HashMap::new();
        let mut items = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            let d = entry
                .parent_id
                .as_deref()
                .and_then(|parent| depth.get(parent).copied())
                .map(|d| d + 1)
                .unwrap_or(0);
            depth.insert(entry.id.as_str(), d);
            items.push(TreeItem {
                id: entry.id.clone(),
                parent_id: entry.parent_id.clone(),
                depth: d,
                label: entry_label(entry),
            });
        }
        items
    }
}

/// A row of the `/tree` overlay. Used by the `tui` feature.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct TreeItem {
    pub id: String,
    pub parent_id: Option<String>,
    pub depth: usize,
    pub label: String,
}

// Entry labels for the `/tree` overlay; only used by the `tui` feature.
#[allow(dead_code)]
fn entry_label(entry: &Entry) -> String {
    match &entry.message {
        Some(Message::User { content, .. }) => {
            format!("❯ {}", truncate(&blocks_text(content), 60))
        }
        Some(Message::Assistant(a)) => {
            let text = blocks_text(&a.content);
            let tool = a.content.iter().find_map(|c| match c {
                Content::ToolCall { name, .. } => Some(name.as_str()),
                _ => None,
            });
            match tool {
                Some(name) if text.is_empty() => format!("  [tool: {name}]"),
                Some(name) => format!("  {} [tool: {name}]", truncate(&text, 48)),
                None => format!("  {}", truncate(&text, 60)),
            }
        }
        Some(Message::ToolResult(tr)) => {
            format!(
                "    [{}: {}]",
                tr.tool_name,
                if tr.is_error { "error" } else { "ok" }
            )
        }
        None => {
            let kind = entry
                .raw
                .as_ref()
                .and_then(|v| v.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("entry");
            format!("  · {kind}")
        }
    }
}

#[allow(dead_code)]
fn truncate(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        let head: String = s.chars().take(n).collect();
        format!("{head}…")
    }
}

#[allow(dead_code)]
fn blocks_text(content: &[Content]) -> String {
    content
        .iter()
        .filter_map(|c| c.as_text())
        .collect::<Vec<_>>()
        .join("")
}

pub fn sessions_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("sessions")
}

pub fn save(config_dir: &Path, session: &mut Session) -> anyhow::Result<PathBuf> {
    // Refresh the timestamp so `--continue` / bare `-r` point at the session
    // written last.
    session.updated_ms = pi_ai::now_ms();
    let dir = sessions_dir(config_dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    let path = dir.join(format!("{}.json", session.id));
    std::fs::write(&path, session.to_json()?)
        .with_context(|| format!("write {}", path.display()))?;
    tracing::debug!(
        session = %session.id,
        entries = session.entries.len(),
        path = %path.display(),
        "saved session"
    );
    Ok(path)
}

pub fn load(config_dir: &Path, id: &str) -> anyhow::Result<Session> {
    let path = sessions_dir(config_dir).join(format!("{id}.json"));
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    Session::from_json(&text)
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
        let s: Session = match Session::from_json(&text) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let messages = s.messages();
        let first_user = messages
            .iter()
            .find_map(|m| match m {
                Message::User { content, .. } => content
                    .iter()
                    .find_map(|c| c.as_text().map(|s| s.to_string())),
                _ => None,
            })
            .unwrap_or_default();
        // Fall back to the file mtime so sessions written by older builds (with
        // a stale `updated_ms`) still sort as "most recently written".
        let file_ms = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        out.push(SessionSummary {
            id: s.id,
            updated_ms: s.updated_ms.max(file_ms),
            model: s.model,
            provider: s.provider,
            first_message: first_user,
            turns: messages.len(),
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
// JSON object; message entries carry `id` / `parentId` and form a tree. Import
// keeps the whole tree; export writes it back.
// ---------------------------------------------------------------------------

/// Load either a native session JSON file or an upstream `.jsonl` session.
pub fn import(path: &Path) -> anyhow::Result<Session> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("json") => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("read {}", path.display()))?;
            Session::from_json(&text)
        }
        _ => import_jsonl(path),
    }
}

/// Parse an upstream `.jsonl` session into a [`Session`], keeping the tree.
pub fn import_jsonl(path: &Path) -> anyhow::Result<Session> {
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
        let message = value.get("message").and_then(convert_message);
        // Keep non-message entries verbatim so export can reproduce them.
        let raw = if message.is_none() {
            Some(value.clone())
        } else {
            None
        };
        entries.push(Entry {
            id,
            parent_id: value
                .get("parentId")
                .and_then(Value::as_str)
                .map(String::from),
            timestamp: value
                .get("timestamp")
                .and_then(Value::as_str)
                .map(parse_ts_str)
                .unwrap_or_else(pi_ai::now_ms),
            message,
            raw,
        });
    }

    let active_leaf = entries.last().map(|e| e.id.clone());

    let (model, provider) = entries
        .iter()
        .rev()
        .find_map(|e| match &e.message {
            Some(Message::Assistant(a)) => Some((a.model.clone(), a.provider.clone())),
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
            .map(|e| e.timestamp)
            .unwrap_or_else(pi_ai::now_ms),
        model,
        provider,
        entries,
        active_leaf,
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

/// Render a [`Session`] as upstream v3 JSONL, preserving the tree.
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

    for entry in &session.entries {
        // Non-message entries are written back verbatim.
        if let Some(raw) = &entry.raw {
            out.push_str(&serde_json::to_string(raw)?);
            out.push('\n');
            continue;
        }
        let Some(message) = &entry.message else {
            continue;
        };
        let value = serde_json::json!({
            "type": "message",
            "id": entry.id,
            "parentId": entry.parent_id,
            "timestamp": iso(entry.timestamp),
            "message": export_message(message),
        });
        out.push_str(&serde_json::to_string(&value)?);
        out.push('\n');
    }
    Ok(out)
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

fn new_id() -> String {
    format!("{:x}-{:08x}", pi_ai::now_ms(), rand_u32())
}

fn new_entry_id() -> String {
    format!("e{:x}-{:08x}", pi_ai::now_ms(), rand_u32())
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
        r#"{"type":"model_change","id":"m1","parentId":null,"timestamp":"2026-05-01T00:00:00.500Z","provider":"deepseek","modelId":"deepseek-v4-flash"}"#,
        r#"{"type":"message","id":"a","parentId":null,"timestamp":"2026-05-01T00:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":"2026-05-01T00:00:01.000Z"}}"#,
        r#"{"type":"message","id":"b","parentId":"a","timestamp":"2026-05-01T00:00:02.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"p"},{"type":"text","text":"hello"},{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"x"}}],"api":"openai-completions","provider":"deepseek","model":"deepseek-v4-flash","usage":{"input":10,"output":2,"cacheRead":1,"cacheWrite":0,"totalTokens":12,"cost":{"input":0.1,"output":0.2,"cacheRead":0.01,"cacheWrite":0,"total":0.31}},"stopReason":"toolUse","timestamp":"2026-05-01T00:00:02.000Z"}}"#,
        r#"{"type":"message","id":"c","parentId":"b","timestamp":"2026-05-01T00:00:03.000Z","message":{"role":"toolResult","toolCallId":"c1","toolName":"read","content":[{"type":"text","text":"contents"}],"isError":false,"timestamp":"2026-05-01T00:00:03.000Z"}}"#,
        // second branch off "a"
        r#"{"type":"message","id":"d","parentId":"a","timestamp":"2026-05-01T00:00:04.000Z","message":{"role":"assistant","content":[{"type":"text","text":"branch two"}],"api":"openai-completions","provider":"deepseek","model":"deepseek-v4-flash","usage":{},"stopReason":"stop","timestamp":"2026-05-01T00:00:04.000Z"}}"#,
    ];

    #[test]
    fn imports_full_tree_and_active_branch() {
        let dir = temp_dir("import");
        let path = dir.join("s.jsonl");
        std::fs::write(&path, SAMPLE.join("\n") + "\n").unwrap();

        let session = import_jsonl(&path).unwrap();
        assert_eq!(session.id, "sess-1");
        assert_eq!(session.provider, "deepseek");
        // All entries are kept, including the model_change and the second branch.
        assert_eq!(session.entries.len(), 5);
        assert!(session.entries.iter().any(|e| e.raw.is_some()));
        // Active leaf is the last written entry ("d").
        assert_eq!(session.active_leaf.as_deref(), Some("d"));
        assert_eq!(session.messages().len(), 2);
        match &session.branch_to("c")[1].message {
            Some(Message::Assistant(a)) => assert_eq!(a.stop_reason, StopReason::ToolUse),
            other => panic!("expected assistant, got {other:?}"),
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn export_preserves_tree() {
        let dir = temp_dir("export-tree");
        let path = dir.join("s.jsonl");
        std::fs::write(&path, SAMPLE.join("\n") + "\n").unwrap();
        let session = import_jsonl(&path).unwrap();

        let out = export_jsonl(&session).unwrap();
        let round = dir.join("round.jsonl");
        std::fs::write(&round, &out).unwrap();
        let back = import_jsonl(&round).unwrap();
        assert_eq!(back.entries.len(), session.entries.len());
        // The non-message entry survives the round trip.
        assert!(back.entries.iter().any(|e| e.raw.is_some()));
        // "b" and "d" share parent "a" in the round-tripped file.
        let b_parent = back
            .entries
            .iter()
            .find(|e| e.id == "b")
            .unwrap()
            .parent_id
            .clone();
        let d_parent = back
            .entries
            .iter()
            .find(|e| e.id == "d")
            .unwrap()
            .parent_id
            .clone();
        assert_eq!(b_parent.as_deref(), Some("a"));
        assert_eq!(d_parent.as_deref(), Some("a"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn branch_switch_and_abandoned() {
        let dir = temp_dir("branch");
        let path = dir.join("s.jsonl");
        std::fs::write(&path, SAMPLE.join("\n") + "\n").unwrap();
        let mut session = import_jsonl(&path).unwrap();

        let abandoned = session.abandoned(Some("d"), Some("c"));
        assert_eq!(abandoned.len(), 1);
        match &abandoned[0] {
            Message::Assistant(a) => {
                let text: String = a.content.iter().filter_map(|c| c.as_text()).collect();
                assert!(text.contains("branch two"));
            }
            other => panic!("expected assistant, got {other:?}"),
        }

        session.set_active_leaf(Some("c".into()));
        assert_eq!(session.messages().len(), 3);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn migrates_legacy_messages_format() {
        let model = pi_ai::Model::openai_gpt_4o();
        let mut session = Session::new(&model);
        session.push_message(Message::user_text("old"));
        // Emulate the pre-tree file shape.
        let legacy = serde_json::json!({
            "id": session.id,
            "created_ms": 1,
            "updated_ms": 2,
            "model": "gpt-4o",
            "provider": "openai",
            "messages": [Message::user_text("legacy")],
        });
        let loaded = Session::from_json(&legacy.to_string()).unwrap();
        assert_eq!(loaded.messages().len(), 1);
        match &loaded.messages()[0] {
            Message::User { content, .. } => assert_eq!(content[0].as_text(), Some("legacy")),
            other => panic!("expected user, got {other:?}"),
        }
    }

    #[test]
    fn save_bumps_updated_ms() {
        let dir = temp_dir("touch");
        let model = pi_ai::Model::openai_gpt_4o();
        let mut session = Session::new(&model);
        session.updated_ms = 0;
        save(&dir, &mut session).unwrap();
        assert!(session.updated_ms > 0);
        std::fs::remove_dir_all(dir).ok();
    }
}
