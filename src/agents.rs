use std::path::Path;
use std::time::UNIX_EPOCH;

use serde_json::Value;

use crate::activity::{AgentActivity, Doing, Status};
use crate::transcript;

/// A running agent silent for this long is shown as Unknown.
pub const AGENT_STALE: i64 = 10 * 60_000;
/// An unfinished agent silent for this long crashed: it is dropped.
pub const AGENT_GONE: i64 = 2 * 3_600_000;
const DETAIL_CAP: u64 = 256 * 1024;

fn mtime_ms(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).ok()?.modified().ok()?;
    let d = t.duration_since(UNIX_EPOCH).ok()?;
    Some(d.as_millis() as i64)
}

fn meta(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn str_field<'a>(meta: &'a Value, key: &str) -> Option<&'a str> {
    meta.get(key)?.as_str().filter(|s| !s.is_empty())
}

fn status(now: i64, last_write: i64) -> Option<Status> {
    match now - last_write {
        age if age > AGENT_GONE => None,
        age if age > AGENT_STALE => Some(Status::Unknown),
        _ => Some(Status::Working),
    }
}

pub fn claude(session_dir: &Path, now: i64, detail: bool) -> Vec<AgentActivity> {
    let Ok(entries) = std::fs::read_dir(session_dir.join("subagents")) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".meta.json"))
            .filter(|n| n.starts_with("agent-"))
        else {
            continue;
        };
        let Some(meta) = meta(&path) else { continue };
        if meta.get("finished").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let transcript_path = path.with_file_name(format!("{stem}.jsonl"));
        let Some(since) = mtime_ms(&path) else {
            continue;
        };
        let last_write = mtime_ms(&transcript_path).unwrap_or(since);
        let Some(status) = status(now, last_write) else {
            continue;
        };
        let label = str_field(&meta, "description")
            .or_else(|| str_field(&meta, "agentType"))
            .unwrap_or(stem)
            .to_string();
        let doing: Option<Doing> = detail
            .then(|| transcript::doing(&transcript::tail(&transcript_path, DETAIL_CAP)))
            .flatten();
        out.push(AgentActivity {
            label,
            status,
            since: Some(since),
            doing,
        });
    }
    out
}

pub fn pi_sub(root: &Path, now: i64) -> Vec<(Option<u32>, AgentActivity)> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dir in entries.flatten().map(|e| e.path()) {
        if dir.join("exit.json").exists() {
            continue;
        }
        let meta_path = dir.join("meta.json");
        let Some(meta) = meta(&meta_path) else {
            continue;
        };
        let Some(since) = mtime_ms(&meta_path) else {
            continue;
        };
        let last_write = mtime_ms(&dir.join("events.jsonl")).unwrap_or(since);
        let Some(status) = status(now, last_write) else {
            continue;
        };
        let id = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let label = str_field(&meta, "label")
            .or_else(|| str_field(&meta, "role"))
            .unwrap_or(id)
            .to_string();
        let parent = meta
            .get("parent_pid")
            .and_then(Value::as_u64)
            .and_then(|p| u32::try_from(p).ok());
        out.push((
            parent,
            AgentActivity {
                label,
                status,
                since: Some(since),
                doing: None,
            },
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    const NOW: i64 = 1_800_000_000_000;

    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "decisions-agents-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn touch(path: &Path, age_ms: i64) {
        let at = UNIX_EPOCH + Duration::from_millis((NOW - age_ms) as u64);
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(at)
            .unwrap();
    }

    fn write(path: &Path, text: &str, age_ms: i64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        touch(path, age_ms);
    }

    fn subagent(session: &Path, id: &str, meta: &str, age_ms: i64) {
        let base = session.join("subagents");
        write(&base.join(format!("agent-{id}.meta.json")), meta, age_ms);
        write(&base.join(format!("agent-{id}.jsonl")), "", age_ms);
    }

    #[test]
    fn a_finished_subagent_is_left_out() {
        let s = dir();
        subagent(&s, "a1", r#"{"description":"Done","finished":true}"#, 1_000);
        assert!(claude(&s, NOW, false).is_empty());
    }

    #[test]
    fn a_running_subagent_is_working_with_its_description() {
        let s = dir();
        subagent(
            &s,
            "a1",
            r#"{"agentType":"general-purpose","description":"Review colours"}"#,
            1_000,
        );
        let got = claude(&s, NOW, false);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].label, "Review colours");
        assert_eq!(got[0].status, Status::Working);
        assert_eq!(got[0].since, Some(NOW - 1_000));
        assert!(got[0].doing.is_none());
    }

    #[test]
    fn a_silent_one_is_unknown_after_ten_minutes() {
        let s = dir();
        subagent(&s, "a1", r#"{"agentType":"explorer"}"#, 11 * 60_000);
        let got = claude(&s, NOW, false);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].label, "explorer");
        assert_eq!(got[0].status, Status::Unknown);
    }

    #[test]
    fn a_crashed_one_is_dropped_after_two_hours() {
        let s = dir();
        subagent(&s, "a1", r#"{"description":"Crashed"}"#, 3 * 3_600_000);
        assert!(claude(&s, NOW, false).is_empty());
    }

    #[test]
    fn a_broken_meta_skips_only_that_agent() {
        let s = dir();
        subagent(&s, "a1", "{\"descr", 1_000);
        subagent(&s, "a2", r#"{"description":"Fine"}"#, 1_000);
        let got = claude(&s, NOW, false);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].label, "Fine");
    }

    #[test]
    fn the_label_falls_back_to_the_file_stem() {
        let s = dir();
        subagent(&s, "a1", "{}", 1_000);
        assert_eq!(claude(&s, NOW, false)[0].label, "agent-a1");
    }

    #[test]
    fn pi_sub_skips_exited_agents_and_reads_the_parent_pid() {
        let root = dir();
        write(&root.join("x1/meta.json"), r#"{"label":"gone"}"#, 1_000);
        write(&root.join("x1/exit.json"), "{}", 1_000);
        write(
            &root.join("x2/meta.json"),
            r#"{"label":"research","role":"researcher","parent_pid":4242}"#,
            1_000,
        );
        write(&root.join("x2/events.jsonl"), "", 1_000);
        write(
            &root.join("x3/meta.json"),
            r#"{"role":"coder"}"#,
            20 * 60_000,
        );
        let mut got = pi_sub(&root, NOW);
        got.sort_by_key(|(_, a)| a.label.clone());
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, None);
        assert_eq!(got[0].1.label, "coder");
        assert_eq!(got[0].1.status, Status::Unknown);
        assert_eq!(got[1].0, Some(4242));
        assert_eq!(got[1].1.label, "research");
        assert_eq!(got[1].1.status, Status::Working);
    }
}
