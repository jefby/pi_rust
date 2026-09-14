//! Session persistence: save transcripts as JSON under
//! `$XDG_CONFIG_HOME/pi/sessions/<id>.json`, list them, and load by id.

use std::path::{Path, PathBuf};

use anyhow::Context;
use pi_ai::Message;
use serde::{Deserialize, Serialize};

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
