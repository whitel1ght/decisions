//! `on_answer`: a command run on every answer, own-words answer and ask-back, with the event
//! as one JSON line on stdin. A failure or a timeout never undoes the change; the caller warns.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::store::Event;

pub const LIMIT: Duration = Duration::from_secs(10);

pub fn run(cmd: Option<&str>, event: &Event) -> Result<(), String> {
    run_within(cmd, event, LIMIT)
}

pub fn run_within(cmd: Option<&str>, event: &Event, limit: Duration) -> Result<(), String> {
    let Some(cmd) = cmd.filter(|c| !c.trim().is_empty()) else {
        return Ok(());
    };
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("on_answer did not start: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(
            stdin,
            "{}",
            serde_json::to_string(event).unwrap_or_default()
        );
    }
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut err);
                }
                let line = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                return Err(format!("on_answer failed ({status}): {line}"));
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "on_answer took longer than {}s and was stopped",
                    limit.as_secs_f32()
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event() -> Event {
        Event {
            event: "answered",
            id: "D-7".into(),
            key: "ACME-1".into(),
            source: "s".into(),
            session: String::new(),
            option: Some(2),
            note: None,
            text: "D-7 answered: 2 — Push only.".into(),
            at: "2026-10-20T11:20:00+03:00".into(),
        }
    }

    #[test]
    fn no_hook_is_fine() {
        assert_eq!(run(None, &event()), Ok(()));
        assert_eq!(run(Some("  "), &event()), Ok(()));
    }

    #[test]
    fn the_hook_gets_the_event_as_one_json_line() {
        let out = std::env::temp_dir().join(format!("decisions-hook-{}", std::process::id()));
        run(Some(&format!("cat > '{}'", out.display())), &event()).unwrap();
        let line = std::fs::read_to_string(&out).unwrap();
        let v: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(
            (
                v["event"].as_str(),
                v["id"].as_str(),
                v["option"].as_u64(),
                v["text"].as_str()
            ),
            (
                Some("answered"),
                Some("D-7"),
                Some(2),
                Some("D-7 answered: 2 — Push only.")
            )
        );
        assert!(v["note"].is_null());
        assert_eq!(line.lines().count(), 1);
    }

    #[test]
    fn a_failing_hook_warns() {
        let err = run(Some("echo nope >&2; exit 3"), &event()).unwrap_err();
        assert!(err.contains("on_answer") && err.contains("nope"), "{err}");
    }

    #[test]
    fn a_slow_hook_is_stopped_and_reported() {
        let t = std::time::Instant::now();
        let err = run_within(
            Some("sleep 5"),
            &event(),
            std::time::Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(err.contains("stopped"), "{err}");
        assert!(t.elapsed() < std::time::Duration::from_secs(2));
    }
}
