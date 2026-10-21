//! The bot's memory: `telegram.json` in the decisions folder. Every change is a read-modify-write
//! under `.telegram.lock`, so `decisions telegram on|off` and the service never undo each other.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    pub message_id: i64,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub discussion_seen: usize,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub html: bool,
    #[serde(default)]
    pub replies: Vec<i64>,
    /// The card's `asked` and question when it was sent: an id reused for a new card (after
    /// a file was deleted) does not match, so the new card is sent.
    #[serde(default)]
    pub asked: String,
    #[serde(default)]
    pub question: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub option: u8,
    #[serde(default)]
    pub note: String,
    pub due_ms: i64,
    pub message_id: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub chat_id: Option<i64>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub offset: i64,
    #[serde(default)]
    pub messages: BTreeMap<String, Sent>,
    #[serde(default)]
    pub pending: BTreeMap<String, Pending>,
    #[serde(default)]
    pub last_error: Option<String>,
}

impl State {
    /// The card a message belongs to: the card's own message or one of the bot's replies under it.
    pub fn card_of(&self, message_id: i64) -> Option<String> {
        self.messages
            .iter()
            .find(|(_, s)| s.message_id == message_id || s.replies.contains(&message_id))
            .map(|(id, _)| id.clone())
    }

    /// Sent cards not answered yet.
    pub fn open(&self) -> Vec<String> {
        self.messages
            .iter()
            .filter(|(_, s)| !s.done)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

pub fn path(dir: &Path) -> PathBuf {
    dir.join("telegram.json")
}

pub fn load(dir: &Path) -> Result<State, String> {
    let p = path(dir);
    match fs::read_to_string(&p) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

fn lock_file(dir: &Path, name: &str) -> Result<File, String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    File::create(dir.join(name)).map_err(|e| format!("{name}: {e}"))
}

/// Read, change and write `telegram.json` under its lock; returns the state as written.
pub fn update(dir: &Path, f: impl FnOnce(&mut State)) -> Result<State, String> {
    let lock = lock_file(dir, ".telegram.lock")?;
    lock.lock().map_err(|e| format!(".telegram.lock: {e}"))?;
    let mut s = load(dir)?;
    f(&mut s);
    let text = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?;
    crate::store::write_atomic(&path(dir), &text).map_err(|e| e.0)?;
    Ok(s)
}

/// Held by `run` and `pair` while they talk to Telegram; None when another process holds it.
pub fn run_lock(dir: &Path) -> Result<Option<File>, String> {
    let f = lock_file(dir, ".telegram.run.lock")?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(fs::TryLockError::WouldBlock) => Ok(None),
        Err(fs::TryLockError::Error(e)) => Err(format!(".telegram.run.lock: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "decisions-tgstate-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn missing_file_is_the_default() {
        assert_eq!(load(&temp()).unwrap(), State::default());
    }

    #[test]
    fn round_trip_and_private_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp();
        let s = update(&dir, |s| {
            s.chat_id = Some(42);
            s.enabled = true;
            s.messages.insert(
                "D-7".into(),
                Sent {
                    message_id: 9,
                    text: "<b>Q</b>".into(),
                    html: true,
                    replies: vec![11],
                    ..Sent::default()
                },
            );
            s.pending.insert(
                "D-7".into(),
                Pending {
                    option: 2,
                    note: String::new(),
                    due_ms: 5,
                    message_id: 9,
                },
            );
        })
        .unwrap();
        assert_eq!(load(&dir).unwrap(), s);
        let mode = fs::metadata(path(&dir)).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(s.card_of(9).as_deref(), Some("D-7"));
        assert_eq!(
            s.card_of(11).as_deref(),
            Some("D-7"),
            "a reply under the card"
        );
        assert_eq!(s.card_of(12), None);
        assert_eq!(s.open(), vec!["D-7".to_string()]);
    }

    #[test]
    fn concurrent_updates_lose_nothing() {
        let dir = temp();
        let hs: Vec<_> = (0..4)
            .map(|_| {
                let d = dir.clone();
                std::thread::spawn(move || {
                    for _ in 0..25 {
                        update(&d, |s| s.offset += 1).unwrap();
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(load(&dir).unwrap().offset, 100);
    }

    #[test]
    fn one_runner_at_a_time() {
        let dir = temp();
        let first = run_lock(&dir).unwrap();
        assert!(first.is_some());
        assert!(run_lock(&dir).unwrap().is_none(), "held");
        drop(first);
        assert!(run_lock(&dir).unwrap().is_some(), "released");
    }
}
