//! A decision as the deck sees it, and the backend the deck answers through: the store
//! (`live`) or the in-memory demo.

use serde::{Deserialize, Serialize};

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
    /// `answer` for a card reporting an answer to the user's question; empty for a question.
    #[serde(default)]
    pub kind: String,
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
    /// A card reporting an answer to the user's question, not a question.
    pub fn is_answer(&self) -> bool {
        self.kind == "answer"
    }

    /// When it was asked, in unix milliseconds; None when `asked` is not RFC 3339.
    pub fn asked_ms(&self) -> Option<i64> {
        chrono::DateTime::parse_from_rfc3339(&self.asked)
            .ok()
            .map(|t| t.timestamp_millis())
    }
}

pub fn decode(json: &str) -> Result<Vec<Decision>, String> {
    serde_json::from_str(json).map_err(|e| format!("bad JSON from decide: {e}"))
}

/// Where the queue comes from: the store, or the in-memory demo.
pub trait Backend: Send + Sync {
    fn list(&self) -> Result<Vec<Decision>, String>;
    /// Ok(Some(warning)) when the change stands but something around it failed (the hook).
    fn answer(&self, id: &str, n: u8, note: Option<&str>) -> Result<Option<String>, String>;
    fn ask(&self, id: &str, text: &str) -> Result<Option<String>, String>;
    fn tell(&self, id: &str, text: &str) -> Result<Option<String>, String>;
    fn seen(&self, id: &str) -> Result<Option<String>, String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/list.json");

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
}
