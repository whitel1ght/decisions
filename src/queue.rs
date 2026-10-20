//! The decision queue as `decide` reports it, and the calls that answer it. `decide` owns
//! the files; this module only runs it.

use std::process::Command;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Opt {
    pub n: u8,
    pub text: String,
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Entry {
    #[serde(default)]
    pub at: String,
    #[serde(default)]
    pub who: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Decision {
    pub id: String,
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub mr: String,
    #[serde(default)]
    pub mr_short: String,
    #[serde(default)]
    pub source: String,
    pub question: String,
    pub asked: String,
    #[serde(default)]
    pub blocks: String,
    #[serde(default)]
    pub question_back: bool,
    #[serde(default)]
    pub options: Vec<Opt>,
    #[serde(default)]
    pub recommended: Option<u8>,
    #[serde(default)]
    pub context: String,
    #[serde(default)]
    pub discussion: Vec<Entry>,
}

impl Decision {
    /// When it was asked, in unix milliseconds; None when `asked` is not RFC 3339.
    pub fn asked_ms(&self) -> Option<i64> {
        let at = OffsetDateTime::parse(&self.asked, &Rfc3339).ok()?;
        Some((at.unix_timestamp_nanos() / 1_000_000) as i64)
    }
}

pub fn decode(json: &str) -> Result<Vec<Decision>, String> {
    serde_json::from_str(json).map_err(|e| format!("bad JSON from decide: {e}"))
}

/// Where the queue comes from: the real `decide`, or the in-memory demo.
pub trait Backend: Send + Sync {
    fn list(&self) -> Result<Vec<Decision>, String>;
    fn answer(&self, id: &str, n: u8, note: Option<&str>) -> Result<(), String>;
    fn ask(&self, id: &str, text: &str) -> Result<(), String>;
    fn tell(&self, id: &str, text: &str) -> Result<(), String>;
}

impl Backend for Decide {
    fn list(&self) -> Result<Vec<Decision>, String> {
        Decide::list(self)
    }

    fn answer(&self, id: &str, n: u8, note: Option<&str>) -> Result<(), String> {
        Decide::answer(self, id, n, note)
    }

    fn ask(&self, id: &str, text: &str) -> Result<(), String> {
        Decide::ask(self, id, text)
    }

    fn tell(&self, id: &str, text: &str) -> Result<(), String> {
        Decide::tell(self, id, text)
    }
}

pub struct Decide {
    pub bin: String,
}

impl Decide {
    pub fn new(bin: impl Into<String>) -> Self {
        Self { bin: bin.into() }
    }

    pub fn from_env() -> Self {
        Self::new(std::env::var("DECIDE").unwrap_or_else(|_| "decide".into()))
    }

    pub fn list(&self) -> Result<Vec<Decision>, String> {
        decode(&self.run(&["list", "--json"])?)
    }

    pub fn answer(&self, id: &str, n: u8, note: Option<&str>) -> Result<(), String> {
        let n = n.to_string();
        let mut args = vec!["answer", id, n.as_str()];
        if let Some(note) = note.map(str::trim).filter(|s| !s.is_empty()) {
            args.push(note);
        }
        self.run(&args).map(|_| ())
    }

    pub fn ask(&self, id: &str, text: &str) -> Result<(), String> {
        self.run(&["ask", id, text]).map(|_| ())
    }

    pub fn tell(&self, id: &str, text: &str) -> Result<(), String> {
        self.run(&["tell", id, text]).map(|_| ())
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new(&self.bin)
            .args(args)
            .output()
            .map_err(|e| format!("{} not runnable: {e}", self.bin))?;
        if out.status.success() {
            return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
        }
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .map(String::from)
            .unwrap_or_else(|| format!("{} exited with {}", self.bin, out.status)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const FIXTURE: &str = include_str!("../tests/fixtures/list.json");

    /// A throwaway `decide` stand-in: a shell script with this body.
    fn stub(body: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "decisions-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("decide");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn decodes_what_decide_prints() {
        let list = decode(FIXTURE).unwrap();
        assert_eq!(list.len(), 2);
        let d1 = &list[0];
        assert_eq!(
            (
                d1.id.as_str(),
                d1.key.as_str(),
                d1.mr_short.as_str(),
                d1.blocks.as_str()
            ),
            ("D-1", "ACME-1001", "backend!870", "reply")
        );
        assert_eq!(d1.options.len(), 3);
        assert!(d1.options[0].recommended);
        assert_eq!(d1.recommended, Some(1));
        assert!(d1.context.contains("**two**"));
        let d2 = &list[1];
        assert_eq!(d2.recommended, None);
        assert_eq!(d2.discussion[1].who, "session");
        assert_eq!(d2.discussion[0].at, "11:04");
    }

    #[test]
    fn asked_is_unix_milliseconds() {
        let list = decode(FIXTURE).unwrap();
        assert_eq!(list[0].asked_ms(), Some(1_792_483_448_000));
        let mut bad = list[0].clone();
        bad.asked = "yesterday".into();
        assert_eq!(bad.asked_ms(), None);
    }

    #[test]
    fn missing_optional_fields_default() {
        let list = decode(r#"[{"id":"D-7","question":"Q?","asked":"2026-10-20T11:04:08+03:00"}]"#)
            .unwrap();
        assert!(list[0].options.is_empty());
        assert_eq!(list[0].recommended, None);
        assert_eq!(list[0].key, "");
    }

    #[test]
    fn bad_json_is_an_error() {
        assert!(
            decode("not json")
                .unwrap_err()
                .starts_with("bad JSON from decide")
        );
    }

    #[test]
    fn list_runs_decide_with_json() {
        let bin = stub(&format!(
            "[ \"$*\" = 'list --json' ] && cat <<'EOF'\n{FIXTURE}\nEOF"
        ));
        let list = Decide::new(bin.to_string_lossy()).list().unwrap();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn answer_passes_id_option_and_note() {
        let log = std::env::temp_dir().join(format!("decisions-args-{}", std::process::id()));
        let bin = stub(&format!("printf '%s|' \"$@\" > '{}'", log.display()));
        let decide = Decide::new(bin.to_string_lossy());
        decide.answer("D-1", 2, Some("use staging")).unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "answer|D-1|2|use staging|"
        );
        decide.answer("D-1", 3, Some("  ")).unwrap();
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "answer|D-1|3|");
        decide.ask("D-2", "which run?").unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "ask|D-2|which run?|"
        );
        decide.tell("D-2", "close it as a duplicate").unwrap();
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "tell|D-2|close it as a duplicate|"
        );
    }

    #[test]
    fn a_failing_decide_reports_its_last_stderr_line() {
        let bin = stub(
            "echo 'usage: decide answer id n' >&2; echo 'decide: no such decision D-9' >&2; exit 2",
        );
        let err = Decide::new(bin.to_string_lossy())
            .answer("D-9", 1, None)
            .unwrap_err();
        assert_eq!(err, "decide: no such decision D-9");
    }

    #[test]
    fn a_missing_decide_is_an_error_not_a_panic() {
        let err = Decide::new("/nonexistent/decide").list().unwrap_err();
        assert!(err.starts_with("/nonexistent/decide not runnable"), "{err}");
    }
}
