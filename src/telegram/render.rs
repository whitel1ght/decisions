//! Cards as Telegram messages (HTML parse mode): the text, the option buttons, and the status
//! lines edits append.

use serde::{Deserialize, Serialize};

use crate::app::age_label;
use crate::config::Context;
use crate::queue::Decision;
use crate::store::Outcome;
use crate::telegram::reply;

pub const LIMIT: usize = 4096;
pub const CONTEXT_LIMIT: usize = 3000;
pub const HINT: &str =
    "Reply: text = own words · \"2 text\" = option 2 with a note · \"? text\" = ask back";
pub const CLOSED: &str = "closed";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
    pub text: String,
    pub data: String,
}

pub type Keyboard = Vec<Vec<Button>>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Msg {
    pub text: String,
    pub html: bool,
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The text of an HTML message without its tags, for the plain-text fallback.
pub fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&amp;", "&")
}

/// At most `max` characters, ending in "…" when shortened.
pub fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// `[text](http…)` → a link, in already escaped text.
fn links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        if let Some(close) = after.find("](") {
            let (text, tail) = (&after[..close], &after[close + 2..]);
            if let Some(end) = tail.find(')') {
                let url = &tail[..end];
                let ok = !text.contains('[')
                    && (url.starts_with("http://") || url.starts_with("https://"))
                    && !url.contains(' ')
                    && !url.contains('"');
                if ok {
                    out.push_str(&rest[..open]);
                    out.push_str(&format!("<a href=\"{url}\">{text}</a>"));
                    rest = &tail[end + 1..];
                    continue;
                }
            }
        }
        out.push_str(&rest[..open + 1]);
        rest = &rest[open + 1..];
    }
    out.push_str(rest);
    out
}

/// Text between pairs of `mark` wrapped in `tag`; an unpaired mark stays as it is.
fn pairs(s: &str, mark: &str, tag: &str) -> String {
    let parts: Vec<&str> = s.split(mark).collect();
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i % 2 == 1 && i < parts.len() - 1 {
            out.push_str(&format!("<{tag}>{p}</{tag}>"));
        } else {
            if i % 2 == 1 {
                out.push_str(mark);
            }
            out.push_str(p);
        }
    }
    out
}

fn inline(line: &str) -> String {
    let parts: Vec<&str> = line.split('`').collect();
    let mut out = String::new();
    for (i, p) in parts.iter().enumerate() {
        if i % 2 == 1 && i < parts.len() - 1 {
            out.push_str(&format!("<code>{}</code>", escape(p)));
        } else {
            if i % 2 == 1 {
                out.push('`');
            }
            out.push_str(&pairs(&links(&escape(p)), "**", "b"));
        }
    }
    out
}

/// The Markdown a card's context is written in, as Telegram's HTML subset: bold, code, code
/// blocks, links and headings (as bold lines); everything else stays plain, escaped.
pub fn md_to_html(md: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut fence: Option<Vec<&str>> = None;
    for line in md.lines() {
        if line.trim_start().starts_with("```") {
            match fence.take() {
                Some(body) => out.push(format!("<pre>{}</pre>", escape(&body.join("\n")))),
                None => fence = Some(Vec::new()),
            }
            continue;
        }
        if let Some(body) = fence.as_mut() {
            body.push(line);
            continue;
        }
        let t = line.trim_start();
        if t.starts_with('#') {
            out.push(format!(
                "<b>{}</b>",
                inline(t.trim_start_matches('#').trim())
            ));
        } else {
            out.push(inline(line));
        }
    }
    if let Some(body) = fence {
        out.push(format!("<pre>{}</pre>", escape(&body.join("\n"))));
    }
    out.join("\n")
}

/// `✋ D-7 · ACME-1001 · reply · waiting 3m`, leaving out what is empty.
pub fn header(d: &Decision, now_ms: i64) -> String {
    let mut parts = vec![d.id.clone()];
    for p in [&d.key, &d.blocks] {
        if !p.is_empty() {
            parts.push(p.clone());
        }
    }
    if let Some(at) = d.asked_ms() {
        parts.push(if d.is_answer() {
            format!("answered {} ago", age_label(now_ms - at))
        } else {
            format!("waiting {}", age_label(now_ms - at))
        });
    }
    let mark = if d.is_answer() { "✉" } else { "✋" };
    format!("{mark} {}", parts.join(" · "))
}

fn build(d: &Decision, now_ms: i64, ctx: Context, limit: usize) -> String {
    let mut s = escape(&header(d, now_ms));
    if !d.source.is_empty() {
        s.push('\n');
        s.push_str(&escape(&d.source));
    }
    if d.is_answer() {
        s.push_str(&format!("\n\n<b>you asked: {}</b>", escape(&d.question)));
    } else {
        s.push_str(&format!("\n\n<b>{}</b>", escape(&d.question)));
    }
    if ctx == Context::Full && limit > 0 && !d.context.trim().is_empty() {
        s.push_str("\n\n");
        s.push_str(&md_to_html(&cut(d.context.trim(), limit)));
    }
    if !d.mr.is_empty() {
        let label = if d.mr_short.is_empty() {
            &d.mr
        } else {
            &d.mr_short
        };
        s.push_str(&format!(
            "\n\n🔗 <a href=\"{}\">{}</a>",
            escape(&d.mr).replace('"', "&quot;"),
            escape(label)
        ));
    }
    if !d.is_answer() {
        s.push('\n');
        s.push_str(&escape(HINT));
    }
    s
}

/// The card's message: the context shrinks until it fits; when even no context does not fit,
/// the whole message goes as cut plain text.
pub fn card(d: &Decision, now_ms: i64, ctx: Context) -> Msg {
    let mut limit = CONTEXT_LIMIT;
    loop {
        let s = build(d, now_ms, ctx, limit);
        if s.chars().count() <= LIMIT {
            return Msg {
                text: s,
                html: true,
            };
        }
        if limit == 0 {
            return Msg {
                text: cut(&plain(&s), LIMIT),
                html: false,
            };
        }
        limit = limit.saturating_sub(500);
    }
}

pub fn buttons(d: &Decision) -> Keyboard {
    if d.is_answer() {
        return vec![vec![Button {
            text: "✓ Got it".into(),
            data: reply::seen_data(&d.id),
        }]];
    }
    d.options
        .iter()
        .map(|o| {
            vec![Button {
                text: format!(
                    "{} · {}{}",
                    o.n,
                    o.text,
                    if o.recommended { " ★" } else { "" }
                ),
                data: reply::answer_data(&d.id, o.n),
            }]
        })
        .collect()
}

pub fn undo(id: &str) -> Keyboard {
    vec![vec![Button {
        text: "↩ Undo".into(),
        data: reply::undo_data(id),
    }]]
}

pub fn pending_line(n: u8, text: &str) -> String {
    format!("⏳ {n} — {text}")
}

/// How a card ended, for its message; None while it is still open.
pub fn outcome_line(o: &Outcome, elsewhere: bool) -> Option<String> {
    let tail = if elsewhere { " (in the deck)" } else { "" };
    match o {
        Outcome::Answered { n, text, note } => Some(match note {
            Some(note) => format!("✓ {n} — {text}{tail}\nnote: {note}"),
            None => format!("✓ {n} — {text}{tail}"),
        }),
        Outcome::Told(t) => Some(format!("✓ your words — {t}{tail}")),
        Outcome::Withdrawn(r) => Some(format!("withdrawn: {r}")),
        Outcome::Seen => Some(format!("✓ seen{tail}")),
        Outcome::Pending | Outcome::AskedBack(_) => None,
    }
}

/// The sent text with a status line under it; plain and cut when that would not fit.
pub fn with_status(text: &str, html: bool, status: &str) -> Msg {
    let status = &cut(status, LIMIT / 2);
    let line = if html {
        format!("<b>{}</b>", escape(status))
    } else {
        status.to_string()
    };
    let s = format!("{text}\n\n{line}");
    if s.chars().count() <= LIMIT {
        return Msg { text: s, html };
    }
    let base = if html { plain(text) } else { text.to_string() };
    let room = LIMIT.saturating_sub(status.chars().count() + 2);
    Msg {
        text: format!("{}\n\n{status}", cut(&base, room)),
        html: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::Opt;

    fn d() -> Decision {
        Decision {
            id: "D-7".into(),
            key: "ACME-1001".into(),
            mr: "https://gitlab.com/g/acme-backend/-/merge_requests/870".into(),
            mr_short: "backend!870".into(),
            source: "claude-code · acme-backend".into(),
            kind: String::new(),
            question: "Push the fix & post?".into(),
            asked: "2026-10-21T11:00:00+03:00".into(),
            blocks: "reply".into(),
            question_back: false,
            options: vec![
                Opt {
                    n: 1,
                    text: "Push and post".into(),
                    recommended: true,
                },
                Opt {
                    n: 2,
                    text: "Push only".into(),
                    recommended: false,
                },
            ],
            recommended: Some(1),
            context: "The **QA** run `x<y` failed.".into(),
            discussion: vec![],
        }
    }

    fn at(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn escaping_and_back() {
        assert_eq!(escape("a < b & c > d"), "a &lt; b &amp; c &gt; d");
        assert_eq!(
            plain("<b>a &amp; &lt;b&gt;</b> <a href=\"u\">x</a>"),
            "a & <b> x"
        );
    }

    #[test]
    fn markdown_to_telegram_html() {
        assert_eq!(
            md_to_html("**bold** and `x<y`"),
            "<b>bold</b> and <code>x&lt;y</code>"
        );
        assert_eq!(
            md_to_html("[MR](https://h/x?a=1&b=2)"),
            "<a href=\"https://h/x?a=1&amp;b=2\">MR</a>"
        );
        assert_eq!(md_to_html("## Title"), "<b>Title</b>");
        assert_eq!(
            md_to_html("```rust\nfn a() {}\n<t>\n```"),
            "<pre>fn a() {}\n&lt;t&gt;</pre>"
        );
        assert_eq!(md_to_html("```\nopen"), "<pre>open</pre>");
        assert_eq!(md_to_html("a **b"), "a **b");
        assert_eq!(
            md_to_html("[x](javascript:alert(1))"),
            "[x](javascript:alert(1))"
        );
        assert_eq!(md_to_html("<script>"), "&lt;script&gt;");
    }

    #[test]
    fn hostile_markdown_gives_balanced_tags() {
        let inputs = [
            "**a `b** c`",
            "``` x",
            "**",
            "`",
            "[a](https://x\"y)",
            "# **x",
            "**[l](https://u)**",
            "a & b < c",
            "```\n**x**\n```\n**y",
            "[[x](https://u)",
            "`a` **b `c`** d",
        ];
        for md in inputs {
            let h = md_to_html(md);
            for (open, close) in [
                ("<b>", "</b>"),
                ("<code>", "</code>"),
                ("<pre>", "</pre>"),
                ("<a ", "</a>"),
            ] {
                assert_eq!(
                    h.matches(open).count(),
                    h.matches(close).count(),
                    "{md:?} → {h:?}"
                );
            }
            assert!(!h.contains("href=\"https://x\"y"), "{h}");
        }
    }

    #[test]
    fn a_card() {
        let m = card(&d(), at("2026-10-21T11:03:00+03:00"), Context::Full);
        assert!(m.html);
        let lines: Vec<&str> = m.text.lines().collect();
        assert_eq!(lines[0], "✋ D-7 · ACME-1001 · reply · waiting 3m");
        assert_eq!(lines[1], "claude-code · acme-backend");
        assert_eq!(lines[3], "<b>Push the fix &amp; post?</b>");
        assert!(
            m.text
                .contains("The <b>QA</b> run <code>x&lt;y</code> failed.")
        );
        assert!(m.text.contains(
            "🔗 <a href=\"https://gitlab.com/g/acme-backend/-/merge_requests/870\">backend!870</a>"
        ));
        assert!(m.text.ends_with(&escape(HINT)));
    }

    #[test]
    fn a_bare_card_leaves_out_empty_parts() {
        let mut x = d();
        (x.key, x.blocks, x.source, x.mr, x.mr_short) = Default::default();
        let m = card(&x, at("2026-10-21T11:00:00+03:00"), Context::None);
        assert!(
            m.text.starts_with("✋ D-7 · waiting 0m\n\n<b>"),
            "{}",
            m.text
        );
        assert!(!m.text.contains("QA"), "context = none");
        assert!(!m.text.contains("🔗"));
    }

    #[test]
    fn buttons_mark_the_recommended_option() {
        assert_eq!(
            buttons(&d()),
            vec![
                vec![Button {
                    text: "1 · Push and post ★".into(),
                    data: "a:D-7:1".into()
                }],
                vec![Button {
                    text: "2 · Push only".into(),
                    data: "a:D-7:2".into()
                }],
            ]
        );
        assert_eq!(
            undo("D-7"),
            vec![vec![Button {
                text: "↩ Undo".into(),
                data: "u:D-7".into()
            }]]
        );
    }

    #[test]
    fn long_context_is_cut_and_the_message_capped() {
        let mut x = d();
        x.context = "x".repeat(5000);
        let m = card(&x, 0, Context::Full);
        assert!(m.html && m.text.contains('…'));
        assert!(m.text.chars().count() <= LIMIT);
        x.question = "q".repeat(5000);
        let m = card(&x, 0, Context::Full);
        assert!(!m.html, "too long even without context: plain text");
        assert!(m.text.chars().count() <= LIMIT);
    }

    #[test]
    fn cutting_is_by_characters() {
        for s in [
            "я".repeat(5000),
            "🙂".repeat(5000),
            format!("{}🙂", "a".repeat(2999)),
        ] {
            let c = cut(&s, CONTEXT_LIMIT);
            assert!(c.chars().count() <= CONTEXT_LIMIT);
            let mut x = d();
            x.context = s.clone();
            x.question = "я".repeat(4000);
            assert!(card(&x, 0, Context::Full).text.chars().count() <= LIMIT);
        }
    }

    #[test]
    fn status_lines() {
        let a = Outcome::Answered {
            n: 2,
            text: "Push only".into(),
            note: None,
        };
        assert_eq!(outcome_line(&a, false).unwrap(), "✓ 2 — Push only");
        assert_eq!(
            outcome_line(&a, true).unwrap(),
            "✓ 2 — Push only (in the deck)"
        );
        let n = Outcome::Answered {
            n: 2,
            text: "Push only".into(),
            note: Some("staging".into()),
        };
        assert_eq!(
            outcome_line(&n, false).unwrap(),
            "✓ 2 — Push only\nnote: staging"
        );
        assert_eq!(
            outcome_line(&Outcome::Told("dup".into()), true).unwrap(),
            "✓ your words — dup (in the deck)"
        );
        assert_eq!(
            outcome_line(&Outcome::Withdrawn("gone".into()), false).unwrap(),
            "withdrawn: gone"
        );
        assert_eq!(outcome_line(&Outcome::Pending, false), None);
        assert_eq!(pending_line(2, "Push only"), "⏳ 2 — Push only");
    }

    #[test]
    fn status_is_appended_and_overflow_goes_plain() {
        let m = with_status("<b>Q</b>", true, "✓ 1 — a<b");
        assert_eq!(
            m,
            Msg {
                text: "<b>Q</b>\n\n<b>✓ 1 — a&lt;b</b>".into(),
                html: true
            }
        );
        let m = with_status("plain", false, "✓ 1 — a<b");
        assert_eq!(m.text, "plain\n\n✓ 1 — a<b");
        let big = format!("<b>{}</b>", "x".repeat(4090));
        let m = with_status(&big, true, "✓ 1 — Yes");
        assert!(!m.html && m.text.ends_with("\n\n✓ 1 — Yes") && m.text.chars().count() <= LIMIT);
    }
}
