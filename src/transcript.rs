use crate::activity::Doing;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub const TAIL_CAP: u64 = 2 * 1024 * 1024;
const FIRST_WINDOW: u64 = 256 * 1024;
const TEXT_MAX: usize = 80;

/// `<projects>/*/<session_id>.jsonl`
pub fn find(projects: &Path, session_id: &str) -> Option<PathBuf> {
    let name = format!("{session_id}.jsonl");
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&name))
        .find(|p| p.is_file())
}

/// Parsed records from the end of the file. The window starts at 256 KiB and
/// doubles until it holds an assistant record, reaches `cap` or the file start.
pub fn tail(path: &Path, cap: u64) -> Vec<Value> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(len) = file.metadata().map(|m| m.len()) else {
        return Vec::new();
    };
    let mut window = FIRST_WINDOW.min(cap);
    loop {
        let n = window.min(len);
        let mut buf = Vec::with_capacity(n as usize);
        let read = file
            .seek(SeekFrom::Start(len - n))
            .and_then(|_| file.by_ref().take(n).read_to_end(&mut buf));
        if read.is_err() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&buf);
        let mut lines = text.lines();
        if n < len {
            lines.next();
        }
        let records: Vec<Value> = lines.filter_map(|l| serde_json::from_str(l).ok()).collect();
        let has_assistant = records.iter().any(|r| kind(r) == Some("assistant"));
        if has_assistant || n >= len || window >= cap {
            return records;
        }
        window = (window * 2).min(cap);
    }
}

fn kind(record: &Value) -> Option<&str> {
    record.get("type")?.as_str()
}

fn items(record: &Value) -> &[Value] {
    record
        .pointer("/message/content")
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn item_kind(item: &Value) -> Option<&str> {
    item.get("type")?.as_str()
}

fn at(record: &Value) -> Option<i64> {
    let ts = record.get("timestamp")?.as_str()?;
    Some(
        chrono::DateTime::parse_from_rfc3339(ts)
            .ok()?
            .timestamp_millis(),
    )
}

fn cut(text: &str) -> String {
    if text.chars().count() <= TEXT_MAX {
        return text.to_string();
    }
    let mut out: String = text.chars().take(TEXT_MAX - 1).collect();
    out.push('…');
    out
}

fn tool_detail(name: &str, input: &Value) -> String {
    let field = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    };
    let detail = field("description")
        .or_else(|| field("file_path"))
        .or_else(|| field("command").and_then(|c| c.lines().next()))
        .or_else(|| field("prompt"))
        .or_else(|| field("pattern"));
    match detail {
        Some(d) => format!("{name}: {d}"),
        None => name.to_string(),
    }
}

/// What the last assistant record is doing, if there is one.
pub fn doing(records: &[Value]) -> Option<Doing> {
    let record = records
        .iter()
        .rev()
        .find(|r| kind(r) == Some("assistant"))?;
    let items = items(record);
    let text = items
        .iter()
        .rev()
        .find(|i| item_kind(i) == Some("tool_use"))
        .map(|i| {
            let name = i.get("name").and_then(Value::as_str).unwrap_or("tool");
            tool_detail(name, i.get("input").unwrap_or(&Value::Null))
        })
        .or_else(|| {
            items
                .iter()
                .filter(|i| item_kind(i) == Some("text"))
                .filter_map(|i| i.get("text")?.as_str())
                .flat_map(str::lines)
                .map(str::trim)
                .find(|l| !l.is_empty())
                .map(str::to_string)
        })?;
    Some(Doing {
        text: cut(&text),
        at: at(record),
    })
}

/// Newest timestamp in the window.
pub fn last_at(records: &[Value]) -> Option<i64> {
    records.iter().rev().find_map(at)
}

/// When the last prompt (not a tool result) was sent.
pub fn turn_start(records: &[Value]) -> Option<i64> {
    records
        .iter()
        .rev()
        .filter(|r| kind(r) == Some("user"))
        .find(|r| {
            r.pointer("/message/content").is_some_and(Value::is_string)
                || items(r).iter().any(|i| item_kind(i) == Some("text"))
        })
        .and_then(at)
}

fn word(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(s.len());
    &s[..end]
}

/// Every id that follows `marker`, after skipping `skip` if present.
fn ids_after<'a>(text: &'a str, marker: &str, skip: &str, out: &mut HashSet<&'a str>) {
    for (i, _) in text.match_indices(marker) {
        let rest = &text[i + marker.len()..];
        let id = word(rest.strip_prefix(skip).unwrap_or(rest));
        if !id.is_empty() {
            out.insert(id);
        }
    }
}

fn result_texts(record: &Value) -> Vec<&str> {
    let mut out = Vec::new();
    for item in items(record)
        .iter()
        .filter(|i| item_kind(i) == Some("tool_result"))
    {
        match item.get("content") {
            Some(Value::String(s)) => out.push(s.as_str()),
            Some(Value::Array(parts)) => out.extend(
                parts
                    .iter()
                    .filter(|p| item_kind(p) == Some("text"))
                    .filter_map(|p| p.get("text")?.as_str()),
            ),
            _ => {}
        }
    }
    out
}

fn finished_ids<'a>(text: &'a str, out: &mut HashSet<&'a str>) {
    for (i, _) in text.match_indices("<task-id>") {
        let rest = &text[i + "<task-id>".len()..];
        let id = word(rest);
        let Some(status) = rest.find("<status>") else {
            continue;
        };
        let status = word(&rest[status + "<status>".len()..]);
        if !id.is_empty() && matches!(status, "completed" | "failed" | "killed" | "stopped") {
            out.insert(id);
        }
    }
}

/// Background jobs started in the window and not yet reported finished.
pub fn open_background(records: &[Value]) -> usize {
    let (mut started, mut finished) = (HashSet::new(), HashSet::new());
    for record in records.iter().filter(|r| kind(r) == Some("user")) {
        if let Some(s) = record.pointer("/message/content").and_then(Value::as_str) {
            finished_ids(s, &mut finished);
        }
        for text in result_texts(record) {
            ids_after(text, "moved to the background as ", "task ", &mut started);
            ids_after(text, "running in background with ID: ", "", &mut started);
            ids_after(text, "agentId: ", "", &mut started);
        }
    }
    started.difference(&finished).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Doing;
    use serde_json::{Value, json};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "decisions-transcript-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn assistant(ts: &str, content: Value) -> Value {
        json!({"type":"assistant","timestamp":ts,"message":{"content":content}})
    }

    fn user(ts: &str, content: Value) -> Value {
        json!({"type":"user","timestamp":ts,"message":{"content":content}})
    }

    fn tool_result(ts: &str, text: &str) -> Value {
        user(ts, json!([{"type":"tool_result","content":text}]))
    }

    fn write(path: &Path, records: &[Value]) {
        let mut out = String::new();
        for r in records {
            out.push_str(&r.to_string());
            out.push('\n');
        }
        std::fs::write(path, out).unwrap();
    }

    const T1: &str = "2026-11-05T21:09:59.727Z";
    const T1_MS: i64 = 1_793_912_999_727;
    const T2: &str = "2026-11-05T21:10:30.000Z";

    #[test]
    fn doing_names_the_last_tool_call() {
        let records = vec![assistant(
            T1,
            json!([{"type":"tool_use","name":"Bash","input":{"description":"run the tests","command":"cargo test"}}]),
        )];
        assert_eq!(
            doing(&records),
            Some(Doing {
                text: "Bash: run the tests".into(),
                at: Some(T1_MS)
            })
        );
    }

    #[test]
    fn doing_falls_back_to_text() {
        let records = vec![assistant(
            T1,
            json!([{"type":"text","text":"\nPlan saved.\nmore"}]),
        )];
        assert_eq!(doing(&records).unwrap().text, "Plan saved.");
    }

    #[test]
    fn doing_picks_detail_and_cuts_long_text() {
        let records = vec![assistant(
            T1,
            json!([{"type":"tool_use","name":"Read","input":{"file_path":"/a/b.rs"}},
                   {"type":"tool_use","name":"Bash","input":{"command":"ls\nrm"}}]),
        )];
        assert_eq!(doing(&records).unwrap().text, "Bash: ls");
        let records = vec![assistant(
            T1,
            json!([{"type":"text","text":"x".repeat(200)}]),
        )];
        let text = doing(&records).unwrap().text;
        assert_eq!(text.chars().count(), 80);
        assert!(text.ends_with('…'));
        let records = vec![assistant(
            T1,
            json!([{"type":"tool_use","name":"Task","input":{}}]),
        )];
        assert_eq!(doing(&records).unwrap().text, "Task");
    }

    #[test]
    fn open_background_counts_unfinished_jobs() {
        let records = vec![
            tool_result(T1, "Command running; moved to the background as task k1"),
            tool_result(T1, "running in background with ID: b2"),
            user(
                T2,
                json!(
                    "<task-notification><task-id>k1</task-id><status>completed</status></task-notification>"
                ),
            ),
        ];
        assert_eq!(open_background(&records), 1);
    }

    #[test]
    fn turn_start_ignores_tool_results() {
        let records = vec![
            user(T1, json!("do the thing")),
            assistant(T1, json!([{"type":"text","text":"ok"}])),
            tool_result(T2, "done"),
        ];
        assert_eq!(turn_start(&records), Some(T1_MS));
        assert_eq!(last_at(&records), Some(T1_MS + 30_273));
    }

    #[test]
    fn tail_skips_a_cut_first_line_and_bad_lines() {
        let dir = scratch();
        let path = dir.join("s.jsonl");
        let mut records: Vec<Value> = (0..3000)
            .map(|i| user(T1, json!(format!("prompt number {i} {}", "y".repeat(180)))))
            .collect();
        records.push(assistant(T1, json!([{"type":"text","text":"last"}])));
        write(&path, &records);
        let mut text = std::fs::read_to_string(&path).unwrap();
        assert!(text.len() > 600_000);
        let at = text.len() - 150;
        let at = at + text[at..].find('\n').unwrap() + 1;
        text.insert_str(at, "{oops\n");
        std::fs::write(&path, text).unwrap();
        let got = tail(&path, TAIL_CAP);
        assert!(got.len() > 10);
        assert!(got.iter().all(|r| r.get("type").is_some()));
        assert_eq!(doing(&got).unwrap().text, "last");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tail_reads_at_most_the_cap() {
        let dir = scratch();
        let path = dir.join("s.jsonl");
        let filler = user(T1, json!("z".repeat(1000)));
        let line = filler.to_string() + "\n";
        let n = 5 * 1024 * 1024 / line.len();
        let mut text = line.repeat(n);
        text.push_str(&(assistant(T1, json!([{"type":"text","text":"end"}])).to_string() + "\n"));
        std::fs::write(&path, &text).unwrap();
        let got = tail(&path, TAIL_CAP);
        assert_eq!(doing(&got).unwrap().text, "end");
        assert!(got.len() * line.len() <= 256 * 1024 + line.len());

        std::fs::write(&path, line.repeat(n)).unwrap();
        let got = tail(&path, TAIL_CAP);
        assert!(!got.is_empty());
        assert!(got.len() * line.len() <= TAIL_CAP as usize);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_looks_in_every_project_folder() {
        let dir = scratch();
        std::fs::create_dir_all(dir.join("-a")).unwrap();
        std::fs::create_dir_all(dir.join("-b")).unwrap();
        std::fs::write(dir.join("-b").join("sid.jsonl"), "").unwrap();
        assert_eq!(find(&dir, "sid"), Some(dir.join("-b").join("sid.jsonl")));
        assert_eq!(find(&dir, "nope"), None);
        assert_eq!(find(&dir.join("missing"), "sid"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
