//! The queue on disk: one Markdown file per decision. Every write takes the folder's lock and
//! renames a temp file into place, so readers never see half a file and concurrent writers
//! never lose each other's changes. The format is the one the Python `decide` wrote.

use std::collections::HashMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Local, SecondsFormat};
use serde::Serialize;
use serde_json::{Value, json};

use crate::queue::{Decision, Entry, Opt};

const FIELDS: [&str; 8] = [
    "id", "day", "key", "mr", "asked", "blocks", "status", "source",
];
const SECTIONS: [&str; 5] = ["Question", "Context", "Options", "Discussion", "Answer"];
const REC: &str = " (recommended)";
const RECENT_HOURS: i64 = 12;

#[derive(Debug, Clone, PartialEq)]
pub struct Fail(pub String);

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub type Result<T> = std::result::Result<T, Fail>;

fn fail<T>(msg: impl Into<String>) -> Result<T> {
    Err(Fail(msg.into()))
}

fn io(e: std::io::Error) -> Fail {
    Fail(e.to_string())
}

pub fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn is_id(s: &str) -> bool {
    s.strip_prefix("D-")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

pub fn is_key(s: &str) -> bool {
    let Some((head, num)) = s.rsplit_once('-') else {
        return false;
    };
    let mut chars = head.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric())
        && !num.is_empty()
        && num.bytes().all(|b| b.is_ascii_digit())
}

fn now() -> DateTime<Local> {
    Local::now()
}

fn iso(t: DateTime<Local>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, false)
}

fn scalar(v: &str) -> String {
    let plain = v
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._/:@#?=%+-".contains(c));
    if plain {
        v.to_string()
    } else {
        serde_json::to_string(v).unwrap_or_default()
    }
}

fn unscalar(v: &str) -> Result<String> {
    let v = v.trim();
    if v.starts_with('"') {
        serde_json::from_str(v).map_err(|e| Fail(format!("bad value {v}: {e}")))
    } else {
        Ok(v.to_string())
    }
}

#[derive(Debug, Clone, Default)]
struct Doc {
    meta: HashMap<String, String>,
    sections: HashMap<String, String>,
}

impl Doc {
    fn get(&self, k: &str) -> &str {
        self.meta.get(k).map_or("", String::as_str)
    }

    fn set(&mut self, k: &str, v: impl Into<String>) {
        self.meta.insert(k.into(), v.into());
    }

    fn section(&self, s: &str) -> &str {
        self.sections.get(s).map_or("", String::as_str)
    }

    fn set_section(&mut self, s: &str, v: impl Into<String>) {
        self.sections.insert(s.into(), v.into());
    }

    fn discuss(&mut self, who: &str, text: &str) -> Result<()> {
        let text = one_line(text);
        if text.is_empty() {
            return fail("empty text");
        }
        let entry = format!("- {} {who}: {text}", now().format("%H:%M"));
        let all = format!("{}\n{entry}", self.section("Discussion"));
        self.set_section("Discussion", all.trim());
        Ok(())
    }

    fn require_open(&self, id: &str) -> Result<()> {
        match self.get("status") {
            "open" => Ok(()),
            other => fail(format!("{id} is {other}, not open")),
        }
    }
}

fn parse(text: &str) -> Result<Doc> {
    let rest = text
        .strip_prefix("---\n")
        .ok_or_else(|| Fail("no frontmatter".into()))?;
    let end = rest
        .find("\n---")
        .ok_or_else(|| Fail("no frontmatter".into()))?;
    let front = &rest[..end];
    let after = &rest[end + 4..];
    let body = after.strip_prefix('\n').unwrap_or(after);
    let mut doc = Doc::default();
    for line in front.lines() {
        let (k, v) = line
            .split_once(':')
            .ok_or_else(|| Fail(format!("bad frontmatter line {line:?}")))?;
        doc.meta.insert(k.trim().to_string(), unscalar(v)?);
    }
    if !is_id(doc.get("id")) {
        return fail("no id");
    }
    let mut parts: HashMap<String, Vec<&str>> = HashMap::new();
    let mut name: Option<String> = None;
    for line in body.lines() {
        if let Some(h) = line.strip_prefix("## ").map(str::trim_end)
            && SECTIONS.contains(&h)
        {
            name = Some(h.to_string());
            parts.insert(h.to_string(), Vec::new());
            continue;
        }
        if let Some(n) = &name {
            parts.get_mut(n).expect("section").push(line);
        }
    }
    doc.sections = parts
        .into_iter()
        .map(|(k, v)| (k, v.join("\n").trim().to_string()))
        .collect();
    Ok(doc)
}

fn render(doc: &Doc) -> String {
    let mut out = String::from("---\n");
    for k in FIELDS {
        if k == "source" && doc.get(k).is_empty() {
            continue;
        }
        out.push_str(&format!("{k}: {}\n", scalar(doc.get(k))));
    }
    out.push_str("---\n");
    for s in SECTIONS {
        let v = doc.section(s).trim();
        if !v.is_empty() {
            out.push_str(&format!("## {s}\n{v}\n\n"));
        }
    }
    format!("{}\n", out.trim_end_matches('\n'))
}

pub(crate) fn write_atomic(path: &Path, text: &str) -> Result<()> {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().expect("parent");
    fs::create_dir_all(dir).map_err(io)?;
    let name = path.file_name().expect("name").to_string_lossy();
    let tmp = dir.join(format!(
        ".{name}.{}.{}.tmp",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(io)?;
        f.write_all(text.as_bytes()).map_err(io)?;
    }
    fs::rename(&tmp, path).map_err(io)
}

fn options(section: &str) -> Vec<(u8, String, bool)> {
    section
        .lines()
        .filter_map(|l| {
            let (n, text) = l.trim().split_once(". ")?;
            let n: u8 = n.parse().ok()?;
            Some(match text.strip_suffix(REC) {
                Some(t) => (n, t.to_string(), true),
                None => (n, text.to_string(), false),
            })
        })
        .collect()
}

fn discussion(section: &str) -> Vec<Entry> {
    section
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("- ")?;
            let (at, rest) = rest.split_once(' ')?;
            let (who, text) = rest.split_once(": ")?;
            let ok = at.len() == 5
                && at.as_bytes()[2] == b':'
                && !who.is_empty()
                && who.chars().all(|c| c.is_alphanumeric() || c == '_');
            ok.then(|| Entry {
                at: at.into(),
                who: who.into(),
                text: text.into(),
            })
        })
        .collect()
}

fn question_back(section: &str) -> bool {
    discussion(section).last().is_some_and(|e| e.who == "you")
}

fn age(asked: &str) -> String {
    let Ok(t) = DateTime::parse_from_rfc3339(asked) else {
        return "?".into();
    };
    let mins = (now().fixed_offset() - t).num_seconds().div_euclid(60);
    if mins < 60 {
        format!("{mins}m")
    } else if mins < 1440 {
        format!("{}h", mins / 60)
    } else {
        format!("{}d", mins / 1440)
    }
}

fn mr_short(url: &str, strip: &str) -> String {
    let Some((head, tail)) = url.split_once("/-/merge_requests/") else {
        return String::new();
    };
    let iid: String = tail.chars().take_while(char::is_ascii_digit).collect();
    let repo = head.rsplit('/').next().unwrap_or("");
    if iid.is_empty() || repo.is_empty() {
        return String::new();
    }
    format!(
        "{}!{iid}",
        repo.strip_prefix(strip)
            .filter(|_| !strip.is_empty())
            .unwrap_or(repo)
    )
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct New {
    pub key: String,
    pub mr: String,
    pub blocks: String,
    pub source: String,
    pub question: String,
    pub options: Vec<String>,
    pub recommend: Option<usize>,
    pub context: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Event {
    pub event: &'static str,
    pub id: String,
    pub key: String,
    pub source: String,
    pub option: Option<u8>,
    pub note: Option<String>,
    pub text: String,
    pub at: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Pending,
    Answered {
        n: u8,
        text: String,
        note: Option<String>,
    },
    Told(String),
    AskedBack(String),
    Withdrawn(String),
}

impl Outcome {
    pub fn describe(&self, id: &str) -> String {
        match self {
            Outcome::Answered { n, text, note } => match note {
                Some(note) => format!("answered: {n} — {text}\nnote: {note}"),
                None => format!("answered: {n} — {text}"),
            },
            Outcome::Told(text) => format!("answered in your own words: {text}"),
            Outcome::AskedBack(q) => format!(
                "the user asked back: {q} — answer with reply(\"{id}\", text), then call wait_for_answer(\"{id}\")"
            ),
            Outcome::Pending => format!(
                "pending: {id} is still open — call wait_for_answer(\"{id}\") when you need the answer"
            ),
            Outcome::Withdrawn(reason) => format!("withdrawn: {id} ({reason})"),
        }
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Outcome::Answered { .. } | Outcome::Told(_) => 0,
            Outcome::Pending => 3,
            Outcome::Withdrawn(_) => 4,
            Outcome::AskedBack(_) => 5,
        }
    }
}

pub struct Store {
    pub dir: PathBuf,
    pub mr_strip: String,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>, mr_strip: impl Into<String>) -> Self {
        Self {
            dir: dir.into(),
            mr_strip: mr_strip.into(),
        }
    }

    fn lock(&self) -> Result<File> {
        fs::create_dir_all(&self.dir).map_err(io)?;
        let f = File::create(self.dir.join(".lock")).map_err(io)?;
        f.lock().map_err(io)?;
        Ok(f)
    }

    fn path_of(&self, id: &str) -> Result<PathBuf> {
        if !is_id(id) {
            return fail(format!("not a decision id: {id}"));
        }
        let p = self.dir.join(format!("{id}.md"));
        if !p.exists() {
            return fail(format!("{id} not found in {}", self.dir.display()));
        }
        Ok(p)
    }

    fn load(&self, id: &str) -> Result<(PathBuf, Doc)> {
        let p = self.path_of(id)?;
        let doc = parse(&fs::read_to_string(&p).map_err(io)?)?;
        Ok((p, doc))
    }

    fn next_id(&self) -> String {
        let mut files = Vec::new();
        walk(&self.dir, &mut files);
        let max = files
            .iter()
            .filter_map(|p| {
                p.file_name()?
                    .to_str()?
                    .strip_prefix("D-")?
                    .strip_suffix(".md")?
                    .parse::<u64>()
                    .ok()
            })
            .max()
            .unwrap_or(0);
        format!("D-{}", max + 1)
    }

    fn open_docs(&self) -> (Vec<Doc>, Vec<String>) {
        let mut names: Vec<String> = fs::read_dir(&self.dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.starts_with("D-") && n.ends_with(".md"))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        let (mut docs, mut skipped) = (Vec::new(), Vec::new());
        for name in names {
            match fs::read_to_string(self.dir.join(&name))
                .map_err(io)
                .and_then(|t| parse(&t))
            {
                Ok(d) if d.get("status") == "open" => docs.push(d),
                Ok(_) => {}
                Err(e) => skipped.push(format!("skipped {name}: {e}")),
            }
        }
        docs.sort_by(|a, b| {
            (a.get("blocks").is_empty(), a.get("asked"))
                .cmp(&(b.get("blocks").is_empty(), b.get("asked")))
        });
        (docs, skipped)
    }

    fn summary(&self, d: &Doc) -> Decision {
        let opts = options(d.section("Options"));
        Decision {
            id: d.get("id").into(),
            key: d.get("key").into(),
            mr: d.get("mr").into(),
            mr_short: mr_short(d.get("mr"), &self.mr_strip),
            source: d.get("source").into(),
            question: one_line(d.section("Question")),
            asked: d.get("asked").into(),
            blocks: d.get("blocks").into(),
            question_back: question_back(d.section("Discussion")),
            recommended: opts.iter().find(|o| o.2).map(|o| o.0),
            options: opts
                .into_iter()
                .map(|(n, text, recommended)| Opt {
                    n,
                    text,
                    recommended,
                })
                .collect(),
            context: d.section("Context").into(),
            discussion: discussion(d.section("Discussion")),
        }
    }

    pub fn list(&self) -> (Vec<Decision>, Vec<String>) {
        let (docs, skipped) = self.open_docs();
        (docs.iter().map(|d| self.summary(d)).collect(), skipped)
    }

    pub fn list_json(&self) -> Value {
        let (docs, _) = self.open_docs();
        Value::Array(
            docs.iter()
                .map(|d| {
                    let mut v = serde_json::to_value(self.summary(d)).expect("json");
                    v["age"] = json!(age(d.get("asked")));
                    v
                })
                .collect(),
        )
    }

    pub fn list_lines(&self) -> Vec<String> {
        let (docs, _) = self.open_docs();
        docs.iter()
            .map(|doc| {
                let d = self.summary(doc);
                let mut tail = format!(" · {}", age(&d.asked));
                if !d.blocks.is_empty() {
                    tail.push_str(&format!(" · blocks {}", d.blocks));
                }
                if d.question_back {
                    tail.push_str(" · ?");
                }
                let key = if d.key.is_empty() {
                    "offplan"
                } else {
                    d.key.as_str()
                };
                let mr = if d.mr_short.is_empty() {
                    "-"
                } else {
                    d.mr_short.as_str()
                };
                format!("{:<5} {:<12} {:<14} {}{tail}", d.id, key, mr, d.question)
            })
            .collect()
    }

    pub fn count(&self) -> usize {
        self.open_docs().0.len()
    }

    pub fn show(&self, id: &str) -> Result<String> {
        fs::read_to_string(self.path_of(id)?).map_err(io)
    }

    pub fn find_open(&self, key: &str, question: &str) -> Option<String> {
        let (key, question) = (key.trim().to_uppercase(), one_line(question));
        self.open_docs()
            .0
            .into_iter()
            .find(|d| {
                d.get("key").to_uppercase() == key && one_line(d.section("Question")) == question
            })
            .map(|d| d.get("id").to_string())
    }

    /// The card this asker already filed for the same question: same source, question,
    /// options, ticket and `Ticket:` line. Open ones always; answered or withdrawn ones not
    /// yet closed when asked within `RECENT_HOURS`, so a retried ask_user gets the answer.
    pub fn find_asked(&self, n: &New) -> Option<String> {
        let want_opts: Vec<String> = n.options.iter().map(|o| one_line(o)).collect();
        let (key, question, source) = (
            n.key.trim().to_uppercase(),
            one_line(&n.question),
            one_line(&n.source),
        );
        let ticket = |c: &str| {
            c.lines()
                .next()
                .filter(|l| l.starts_with("Ticket: "))
                .unwrap_or("")
                .to_string()
        };
        let since = now().fixed_offset() - chrono::Duration::hours(RECENT_HOURS);
        let mut names: Vec<String> = fs::read_dir(&self.dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.starts_with("D-") && n.ends_with(".md"))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names.into_iter().find_map(|name| {
            let doc = parse(&fs::read_to_string(self.dir.join(&name)).ok()?).ok()?;
            let recent = doc.get("status") == "open"
                || DateTime::parse_from_rfc3339(doc.get("asked")).is_ok_and(|t| t >= since);
            let opts: Vec<String> = options(doc.section("Options"))
                .into_iter()
                .map(|o| o.1)
                .collect();
            (recent
                && doc.get("source") == source
                && doc.get("key").to_uppercase() == key
                && one_line(doc.section("Question")) == question
                && opts == want_opts
                && ticket(doc.section("Context")) == ticket(&n.context))
            .then(|| doc.get("id").to_string())
        })
    }

    /// How many discussion entries a decision has (open, answered or closed).
    pub fn discussion_count(&self, id: &str) -> Result<usize> {
        let doc = parse(&fs::read_to_string(self.find_file(id)?).map_err(io)?)?;
        Ok(discussion(doc.section("Discussion")).len())
    }

    fn find_file(&self, id: &str) -> Result<PathBuf> {
        if !is_id(id) {
            return fail(format!("not a decision id: {id}"));
        }
        let name = format!("{id}.md");
        let path = self.dir.join(&name);
        if path.exists() {
            return Ok(path);
        }
        let mut files = Vec::new();
        walk(&self.dir.join("done"), &mut files);
        files
            .into_iter()
            .find(|p| p.file_name().is_some_and(|n| n == name.as_str()))
            .ok_or_else(|| Fail(format!("{id} not found in {}", self.dir.display())))
    }

    pub fn add(&self, n: New) -> Result<String> {
        let _lock = self.lock()?;
        let opts: Vec<String> = n.options.iter().map(|o| one_line(o)).collect();
        if !(2..=9).contains(&opts.len()) || opts.iter().any(String::is_empty) {
            return fail("give 2 to 9 non-empty --option values");
        }
        if let Some(r) = n.recommend
            && !(1..=opts.len()).contains(&r)
        {
            return fail(format!("--recommend must be 1..{}", opts.len()));
        }
        let key = n.key.trim().to_uppercase();
        if !key.is_empty() && !is_key(&key) {
            return fail(format!("not a ticket key: {}", n.key));
        }
        let question = one_line(&n.question);
        if question.is_empty() {
            return fail("--question is empty");
        }
        let source = one_line(&n.source);
        if let Some(id) = self.open_docs().0.into_iter().find(|d| {
            d.get("key").to_uppercase() == key
                && one_line(d.section("Question")) == question
                && d.get("source") == source
        }) {
            return fail(format!("{} already asks this", id.get("id")));
        }
        let context: String = n
            .context
            .split('\n')
            .map(|l| {
                if l.starts_with("## ") {
                    format!("#{l}")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let (id, t) = (self.next_id(), now());
        let mut doc = Doc::default();
        doc.set("id", id.clone());
        doc.set("day", t.format("%Y-%m-%d").to_string());
        doc.set("key", key);
        doc.set("mr", n.mr.trim());
        doc.set("asked", iso(t));
        doc.set("blocks", one_line(&n.blocks));
        doc.set("status", "open");
        doc.set("source", one_line(&n.source));
        doc.set_section("Question", question);
        doc.set_section("Context", context);
        let lines: Vec<String> = opts
            .iter()
            .enumerate()
            .map(|(i, o)| {
                format!(
                    "{}. {o}{}",
                    i + 1,
                    if n.recommend == Some(i + 1) { REC } else { "" }
                )
            })
            .collect();
        doc.set_section("Options", lines.join("\n"));
        write_atomic(&self.dir.join(format!("{id}.md")), &render(&doc))?;
        Ok(id)
    }

    fn event(
        doc: &Doc,
        event: &'static str,
        option: Option<u8>,
        note: Option<String>,
        text: String,
        at: String,
    ) -> Event {
        Event {
            event,
            id: doc.get("id").into(),
            key: doc.get("key").into(),
            source: doc.get("source").into(),
            option,
            note,
            text,
            at,
        }
    }

    pub fn answer(&self, id: &str, n: u8, note: &str) -> Result<Event> {
        let _lock = self.lock()?;
        let (p, mut doc) = self.load(id)?;
        doc.require_open(id)?;
        let opts = options(doc.section("Options"));
        let Some((_, text, _)) = opts.iter().find(|o| o.0 == n) else {
            return fail(format!(
                "{id} has no option {n} (1..{})",
                opts.iter().map(|o| o.0).max().unwrap_or(0)
            ));
        };
        let (text, note, at) = (text.clone(), one_line(note), iso(now()));
        let mut answer = format!("{n} — {text}");
        let mut summary = format!("{id} answered: {n} — {text}.");
        if !note.is_empty() {
            answer.push_str(&format!("\nnote: {note}"));
            summary.push_str(&format!(" Note: {note}."));
        }
        answer.push_str(&format!("\nat: {at}"));
        doc.set_section("Answer", answer);
        doc.set("status", "answered");
        write_atomic(&p, &render(&doc))?;
        Ok(Self::event(
            &doc,
            "answered",
            Some(n),
            (!note.is_empty()).then_some(note),
            summary,
            at,
        ))
    }

    pub fn tell(&self, id: &str, text: &str) -> Result<Event> {
        let _lock = self.lock()?;
        let (p, mut doc) = self.load(id)?;
        doc.require_open(id)?;
        let (text, at) = (one_line(text), iso(now()));
        if text.is_empty() {
            return fail("empty text");
        }
        doc.set_section("Answer", format!("your words — {text}\nat: {at}"));
        doc.set("status", "answered");
        write_atomic(&p, &render(&doc))?;
        Ok(Self::event(
            &doc,
            "told",
            None,
            None,
            format!("{id} answered in your own words: {text}."),
            at,
        ))
    }

    pub fn ask(&self, id: &str, text: &str) -> Result<Event> {
        let _lock = self.lock()?;
        let (p, mut doc) = self.load(id)?;
        doc.require_open(id)?;
        doc.discuss("you", text)?;
        write_atomic(&p, &render(&doc))?;
        Ok(Self::event(
            &doc,
            "asked",
            None,
            None,
            format!("{id}: the user asks: {}", one_line(text)),
            iso(now()),
        ))
    }

    pub fn reply(&self, id: &str, text: &str) -> Result<()> {
        let _lock = self.lock()?;
        let (p, mut doc) = self.load(id)?;
        doc.discuss("session", text)?;
        write_atomic(&p, &render(&doc))
    }

    pub fn withdraw(&self, id: &str, reason: &str) -> Result<()> {
        let _lock = self.lock()?;
        let (p, mut doc) = self.load(id)?;
        doc.require_open(id)?;
        doc.set("status", "withdrawn");
        let reason = if reason.trim().is_empty() {
            "no longer needed"
        } else {
            reason
        };
        doc.discuss("session", &format!("withdrawn: {reason}"))?;
        write_atomic(&p, &render(&doc))
    }

    pub fn close(&self, id: &str) -> Result<PathBuf> {
        let _lock = self.lock()?;
        let (p, doc) = self.load(id)?;
        if !matches!(doc.get("status"), "answered" | "withdrawn") {
            return fail(format!("{id} is still open: answer or withdraw it first"));
        }
        let day = if doc.get("day").is_empty() {
            "unknown"
        } else {
            doc.get("day")
        };
        let dest = self.dir.join("done").join(day).join(format!("{id}.md"));
        fs::create_dir_all(dest.parent().expect("parent")).map_err(io)?;
        fs::rename(&p, &dest).map_err(io)?;
        Ok(dest)
    }

    /// What the asker should hear now. Reads without the lock: every write is atomic.
    pub fn outcome(&self, id: &str) -> Result<Outcome> {
        let path = self.find_file(id)?;
        let doc = parse(&fs::read_to_string(&path).map_err(io)?)?;
        Ok(match doc.get("status") {
            "answered" => {
                let answer = doc.section("Answer");
                let first = answer.lines().next().unwrap_or("");
                let note = answer
                    .lines()
                    .find_map(|l| l.strip_prefix("note: "))
                    .map(String::from);
                match first.strip_prefix("your words — ") {
                    Some(text) => Outcome::Told(text.into()),
                    None => {
                        let (n, text) = first.split_once(" — ").unwrap_or(("0", first));
                        Outcome::Answered {
                            n: n.trim().parse().unwrap_or(0),
                            text: text.into(),
                            note,
                        }
                    }
                }
            }
            "withdrawn" => {
                let reason = discussion(doc.section("Discussion"))
                    .into_iter()
                    .rev()
                    .find_map(|e| e.text.strip_prefix("withdrawn: ").map(String::from));
                Outcome::Withdrawn(reason.unwrap_or_else(|| "no longer needed".into()))
            }
            _ if question_back(doc.section("Discussion")) => {
                let last = discussion(doc.section("Discussion"))
                    .pop()
                    .map(|e| e.text)
                    .unwrap_or_default();
                Outcome::AskedBack(last)
            }
            _ => Outcome::Pending,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    fn temp() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let d = std::env::temp_dir().join(format!(
            "decisions-store-{}-{}",
            std::process::id(),
            N.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn store() -> Store {
        Store::new(temp(), "acme-")
    }

    fn new(key: &str, q: &str, opts: &[&str]) -> New {
        New {
            key: key.into(),
            question: q.into(),
            options: opts.iter().map(|o| o.to_string()).collect(),
            ..New::default()
        }
    }

    fn first(s: &Store) -> String {
        s.add(New {
            key: "acme-18313".into(),
            mr: "https://gitlab.com/acme/acme-admin/-/merge_requests/315".into(),
            blocks: "merge".into(),
            question: "Fix the nit, or merge now?".into(),
            options: vec![
                "Fix now, then merge".into(),
                "Merge, follow-up ticket".into(),
                "Decline: the name stays".into(),
            ],
            recommend: Some(1),
            context: "@alex: \"rename to trackCount\" (nit)\nPipeline green.\n".into(),
            ..New::default()
        })
        .unwrap()
    }

    #[test]
    fn add_writes_the_decide_format() {
        let s = store();
        assert_eq!(first(&s), "D-1");
        let text = fs::read_to_string(s.dir.join("D-1.md")).unwrap();
        let fields: Vec<&str> = text
            .lines()
            .skip(1)
            .take(7)
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(
            fields,
            ["id", "day", "key", "mr", "asked", "blocks", "status"]
        );
        assert!(text.contains("key: ACME-18313"));
        assert!(text.contains("status: open"));
        assert!(!text.contains("source:"), "no source line when empty");
        assert!(text.contains("@alex: \"rename to trackCount\" (nit)"));
        assert!(text.contains("1. Fix now, then merge (recommended)"));
        let names: Vec<String> = fs::read_dir(&s.dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            names.iter().all(|n| n == "D-1.md" || n == ".lock"),
            "{names:?}"
        );
    }

    #[test]
    fn awkward_text_survives_a_round_trip() {
        let s = store();
        let mut n = new("", "Weird: text | \"quotes\" ✓", &["a", "b"]);
        n.blocks = "reply: a \"quoted\" | piped".into();
        n.source = "claude-code · repo".into();
        let id = s.add(n).unwrap();
        let (list, _) = s.list();
        let d = list.iter().find(|d| d.id == id).unwrap();
        assert_eq!(d.question, "Weird: text | \"quotes\" ✓");
        assert_eq!(d.blocks, "reply: a \"quoted\" | piped");
        assert_eq!(d.source, "claude-code · repo");
    }

    #[test]
    fn add_refusals() {
        let s = store();
        first(&s);
        assert!(s.add(new("X-1", "q", &["only"])).is_err(), "one option");
        let mut n = new("X-1", "q", &["a", "b"]);
        n.recommend = Some(3);
        assert!(s.add(n).is_err(), "recommend out of range");
        assert!(
            s.add(new("not a key", "q", &["a", "b"])).is_err(),
            "bad key"
        );
        let dup = s
            .add(new("ACME-18313", "Fix the nit, or merge now?", &["a", "b"]))
            .unwrap_err();
        assert!(dup.0.contains("D-1"), "{dup}");
        assert_eq!(
            s.find_open("ACME-18313", "Fix the nit, or merge now?")
                .as_deref(),
            Some("D-1")
        );
    }

    #[test]
    fn list_orders_blocking_first_then_oldest_and_fills_every_field() {
        let s = store();
        s.add(new("", "Later?", &["a", "b"])).unwrap();
        first(&s);
        let (list, _) = s.list();
        assert_eq!(
            list.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            ["D-2", "D-1"]
        );
        let d = &list[0];
        assert_eq!(d.mr_short, "admin!315");
        assert_eq!(d.recommended, Some(1));
        assert!(d.context.contains("Pipeline green."));
        assert!(d.discussion.is_empty());
        assert_eq!(list[1].recommended, None);
        let json = s.list_json();
        assert_eq!(json[0]["age"], "0m");
        assert_eq!(json[0]["mr_short"], "admin!315");
        assert!(
            s.list_lines()[0].contains("admin!315") && s.list_lines()[0].contains("blocks merge")
        );
    }

    #[test]
    fn a_bad_file_is_skipped_and_named() {
        let s = store();
        first(&s);
        fs::write(s.dir.join("D-9.md"), "garbage\n").unwrap();
        let (list, skipped) = s.list();
        assert_eq!(list.len(), 1);
        assert!(skipped[0].contains("D-9.md"), "{skipped:?}");
    }

    #[test]
    fn show_and_count() {
        let s = store();
        first(&s);
        assert!(s.show("D-1").unwrap().contains("## Question"));
        assert!(s.show("D-77").is_err());
        assert_eq!(s.count(), 1);
    }

    #[test]
    fn ask_reply_answer_and_the_events() {
        let s = store();
        s.add(new("ACME-1", "Merge?", &["yes", "no"])).unwrap();
        let e = s.ask("D-1", "is it safe: really | \"sure\"?").unwrap();
        assert_eq!(
            (e.event, e.text.as_str()),
            (
                "asked",
                "D-1: the user asks: is it safe: really | \"sure\"?"
            )
        );
        assert!(
            s.show("D-1")
                .unwrap()
                .contains("you: is it safe: really | \"sure\"?")
        );
        assert!(s.list().0[0].question_back);
        assert_eq!(
            s.outcome("D-1").unwrap(),
            Outcome::AskedBack("is it safe: really | \"sure\"?".into())
        );
        s.reply("D-1", "yes, the pipeline is green").unwrap();
        let d = &s.list().0[0];
        assert!(!d.question_back);
        assert_eq!(
            d.discussion
                .iter()
                .map(|e| e.who.as_str())
                .collect::<Vec<_>>(),
            ["you", "session"]
        );
        assert_eq!(s.outcome("D-1").unwrap(), Outcome::Pending);
        assert!(s.answer("D-1", 7, "").is_err(), "no option 7");
        let e = s.answer("D-1", 1, "ship it: today").unwrap();
        assert_eq!(e.text, "D-1 answered: 1 — yes. Note: ship it: today.");
        assert_eq!(
            (e.event, e.option, e.note.as_deref(), e.key.as_str()),
            ("answered", Some(1), Some("ship it: today"), "ACME-1")
        );
        let shown = s.show("D-1").unwrap();
        assert!(
            shown.contains("1 — yes")
                && shown.contains("note: ship it: today")
                && shown.contains("status: answered")
        );
        assert!(s.answer("D-1", 2, "").is_err(), "answering twice");
        s.reply("D-1", "done, merged").unwrap();
        assert!(
            s.show("D-1").unwrap().contains("1 — yes"),
            "a reply after the answer keeps it"
        );
        assert_eq!(
            s.outcome("D-1").unwrap(),
            Outcome::Answered {
                n: 1,
                text: "yes".into(),
                note: Some("ship it: today".into())
            }
        );
    }

    #[test]
    fn tell_answers_in_your_own_words() {
        let s = store();
        s.add(new("ACME-3", "Ship?", &["yes", "no"])).unwrap();
        let e = s.tell("D-1", "none of these: close it").unwrap();
        assert_eq!(
            (e.event, e.text.as_str(), e.option),
            (
                "told",
                "D-1 answered in your own words: none of these: close it.",
                None
            )
        );
        assert!(
            s.show("D-1")
                .unwrap()
                .contains("your words — none of these: close it")
        );
        assert_eq!(
            s.outcome("D-1").unwrap(),
            Outcome::Told("none of these: close it".into())
        );
        assert!(s.tell("D-1", "again").is_err());
        assert!(s.tell("D-1", "  ").is_err());
    }

    #[test]
    fn withdraw_close_and_numbering() {
        let s = store();
        let id = first(&s);
        s.answer(&id, 1, "").unwrap();
        let wid = s.add(new("ACME-2", "Defer?", &["yes", "no"])).unwrap();
        assert!(s.close(&wid).is_err(), "still open");
        s.withdraw(&wid, "reviewer resolved it").unwrap();
        assert!(
            s.show(&wid)
                .unwrap()
                .contains("withdrawn: reviewer resolved it")
        );
        assert_eq!(
            s.outcome(&wid).unwrap(),
            Outcome::Withdrawn("reviewer resolved it".into())
        );
        let day = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert_eq!(
            s.close(&id).unwrap(),
            s.dir.join("done").join(&day).join(format!("{id}.md"))
        );
        s.close(&wid).unwrap();
        assert!(
            matches!(s.outcome(&id).unwrap(), Outcome::Answered { .. }),
            "outcome finds closed ones"
        );
        assert_eq!(
            s.add(new("ACME-3", "Next?", &["a", "b"])).unwrap(),
            "D-3",
            "numbering continues past done/"
        );
    }

    #[test]
    fn a_section_heading_in_the_context_cannot_fake_a_section() {
        let s = store();
        let mut n = new("ACME-50", "Headings in context?", &["a", "b"]);
        n.context = "Reviewer said:\n## Summary\nrename it\n## Answer\nnot a real answer\n".into();
        let id = s.add(n).unwrap();
        s.reply(&id, "noted").unwrap();
        let shown = s.show(&id).unwrap();
        assert!(shown.contains("rename it") && shown.contains("not a real answer"));
        assert_eq!(s.outcome(&id).unwrap(), Outcome::Pending);
    }

    #[test]
    fn concurrent_reply_and_answer_lose_nothing() {
        let s = Arc::new(store());
        for i in 0..15 {
            let id = s
                .add(new(
                    &format!("ACME-6{i}"),
                    &format!("Race {i}?"),
                    &["a", "b"],
                ))
                .unwrap();
            let (a, b) = (s.clone(), s.clone());
            let (ia, ib) = (id.clone(), id.clone());
            let t1 =
                std::thread::spawn(move || a.reply(&ia, &format!("session reply {i}")).unwrap());
            let t2 = std::thread::spawn(move || {
                b.answer(&ib, 1, "").unwrap();
            });
            t1.join().unwrap();
            t2.join().unwrap();
            let text = s.show(&id).unwrap();
            assert!(
                text.contains(&format!("session reply {i}"))
                    && text.contains("status: answered")
                    && text.contains("## Answer"),
                "{text}"
            );
        }
    }

    #[test]
    fn outcome_texts() {
        assert_eq!(
            Outcome::Answered {
                n: 2,
                text: "Push only".into(),
                note: None
            }
            .describe("D-7"),
            "answered: 2 — Push only"
        );
        assert_eq!(
            Outcome::Answered {
                n: 2,
                text: "Push only".into(),
                note: Some("x".into())
            }
            .describe("D-7"),
            "answered: 2 — Push only\nnote: x"
        );
        assert_eq!(
            Outcome::Told("close it".into()).describe("D-7"),
            "answered in your own words: close it"
        );
        assert_eq!(
            Outcome::AskedBack("which run?".into()).describe("D-7"),
            "the user asked back: which run? — answer with reply(\"D-7\", text), then call wait_for_answer(\"D-7\")"
        );
        assert_eq!(
            Outcome::Pending.describe("D-7"),
            "pending: D-7 is still open — call wait_for_answer(\"D-7\") when you need the answer"
        );
        assert_eq!(
            Outcome::Withdrawn("dup".into()).describe("D-7"),
            "withdrawn: D-7 (dup)"
        );
        assert_eq!(
            [
                Outcome::Told(String::new()).exit_code(),
                Outcome::Pending.exit_code(),
                Outcome::Withdrawn(String::new()).exit_code(),
                Outcome::AskedBack(String::new()).exit_code()
            ],
            [0, 3, 4, 5]
        );
    }

    #[test]
    fn a_reply_mentioning_you_is_not_a_question_back() {
        let s = store();
        s.add(new("", "Merge?", &["yes", "no"])).unwrap();
        s.ask("D-1", "which run?").unwrap();
        s.reply("D-1", "as I told you: the staging run").unwrap();
        assert!(!s.list().0[0].question_back);
        assert_eq!(s.outcome("D-1").unwrap(), Outcome::Pending);
    }

    #[test]
    fn decision_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let s = store();
        first(&s);
        let mode = fs::metadata(s.dir.join("D-1.md"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn the_same_question_from_another_asker_is_a_new_card() {
        let s = store();
        let mut a = new("", "Push to main?", &["yes", "no"]);
        a.source = "claude-code · repoA".into();
        let mut b = a.clone();
        b.source = "codex · repoB".into();
        assert_eq!(s.add(a.clone()).unwrap(), "D-1");
        assert_eq!(s.add(b).unwrap(), "D-2");
        assert!(
            s.add(a).is_err(),
            "the same asker repeating it is still refused"
        );
    }

    #[test]
    fn find_asked_matches_asker_question_options_and_context_head() {
        let s = store();
        let mut n = new("", "Push to main?", &["yes", "no"]);
        n.source = "claude-code · repoA".into();
        n.context = "Ticket: #41\n\nhotfix".into();
        s.add(n.clone()).unwrap();
        assert_eq!(s.find_asked(&n).as_deref(), Some("D-1"));
        let mut other = n.clone();
        other.source = "codex · repoB".into();
        assert_eq!(s.find_asked(&other), None, "another asker");
        let mut opts = n.clone();
        opts.options = vec!["yes, force".into(), "no".into()];
        assert_eq!(s.find_asked(&opts), None, "other options");
        let mut ticket = n.clone();
        ticket.context = "Ticket: #42\n\nhotfix".into();
        assert_eq!(s.find_asked(&ticket), None, "another ticket");
        s.answer("D-1", 1, "").unwrap();
        assert_eq!(
            s.find_asked(&n).as_deref(),
            Some("D-1"),
            "an answered, not yet closed one still matches"
        );
        s.close("D-1").unwrap();
        assert_eq!(s.find_asked(&n), None, "a closed one does not");
    }

    #[test]
    fn discussion_count() {
        let s = store();
        s.add(new("", "Merge?", &["yes", "no"])).unwrap();
        assert_eq!(s.discussion_count("D-1").unwrap(), 0);
        s.ask("D-1", "why?").unwrap();
        s.reply("D-1", "because").unwrap();
        assert_eq!(s.discussion_count("D-1").unwrap(), 2);
    }
}
