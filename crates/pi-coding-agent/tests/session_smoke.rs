//! Session round-trip smoke test against a temp config dir.

use std::path::PathBuf;

#[path = "../src/session.rs"]
#[allow(dead_code)]
mod session;

use pi_ai::{Message, Model};

fn temp_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pi-rs-session-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn save_and_load_roundtrip() {
    let dir = temp_dir();
    let model = Model::anthropic_claude_sonnet_4_6();
    let mut s = session::Session::new(&model);
    s.messages.push(Message::user_text("hello"));
    let path = session::save(&dir, &mut s).unwrap();
    assert!(path.exists());

    let loaded = session::load(&dir, &s.id).unwrap();
    assert_eq!(loaded.id, s.id);
    assert_eq!(loaded.messages.len(), 1);

    let list = session::list(&dir).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, s.id);
    assert_eq!(list[0].turns, 1);
}
