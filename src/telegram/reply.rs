//! What the user sends: reply text, button presses and chat commands.

use crate::store::is_id;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Ask(String),
    Answer { n: u8, note: String },
    Tell(String),
    Empty,
}

/// `? text` asks back; `<n> [text]` picks option n of `options` with a note; anything else is
/// an answer in the user's own words.
pub fn parse(text: &str, options: &[u8]) -> Reply {
    let t = text.trim();
    if t.is_empty() {
        return Reply::Empty;
    }
    if let Some(q) = t.strip_prefix('?') {
        let q = q.trim();
        return if q.is_empty() {
            Reply::Empty
        } else {
            Reply::Ask(q.into())
        };
    }
    let (first, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
    if first.bytes().all(|b| b.is_ascii_digit())
        && let Ok(n) = first.parse::<u8>()
        && options.contains(&n)
    {
        return Reply::Answer {
            n,
            note: rest.trim().into(),
        };
    }
    Reply::Tell(t.into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Press {
    Answer { id: String, n: u8 },
    Undo { id: String },
}

pub fn answer_data(id: &str, n: u8) -> String {
    format!("a:{id}:{n}")
}

pub fn undo_data(id: &str) -> String {
    format!("u:{id}")
}

pub fn press(data: &str) -> Option<Press> {
    let mut it = data.split(':');
    match (it.next(), it.next(), it.next(), it.next()) {
        (Some("a"), Some(id), Some(n), None)
            if is_id(id) && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) =>
        {
            Some(Press::Answer {
                id: id.into(),
                n: n.parse().ok()?,
            })
        }
        (Some("u"), Some(id), None, None) if is_id(id) => Some(Press::Undo { id: id.into() }),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    On,
    Off,
    Status,
    Help,
}

/// A message starting with `/`; unknown commands (and `/start`) get the help.
pub fn command(text: &str) -> Option<Command> {
    let word = text
        .trim()
        .strip_prefix('/')?
        .split_whitespace()
        .next()
        .unwrap_or("");
    Some(match word.split('@').next().unwrap_or("") {
        "on" => Command::On,
        "off" => Command::Off,
        "status" => Command::Status,
        _ => Command::Help,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPTS: &[u8] = &[1, 2];

    #[test]
    fn reply_forms() {
        assert_eq!(parse("? which run", OPTS), Reply::Ask("which run".into()));
        assert_eq!(parse("  ?which run ", OPTS), Reply::Ask("which run".into()));
        assert_eq!(
            parse("2 mention staging", OPTS),
            Reply::Answer {
                n: 2,
                note: "mention staging".into()
            }
        );
        assert_eq!(
            parse("2", OPTS),
            Reply::Answer {
                n: 2,
                note: String::new()
            }
        );
        assert_eq!(parse("9 nope", OPTS), Reply::Tell("9 nope".into()));
        assert_eq!(parse("+2 x", OPTS), Reply::Tell("+2 x".into()));
        assert_eq!(parse("close it", OPTS), Reply::Tell("close it".into()));
        assert_eq!(parse("   ", OPTS), Reply::Empty);
        assert_eq!(parse("?", OPTS), Reply::Empty);
    }

    #[test]
    fn button_data_round_trips() {
        assert_eq!(answer_data("D-7", 2), "a:D-7:2");
        assert_eq!(undo_data("D-7"), "u:D-7");
        assert_eq!(
            press("a:D-7:2"),
            Some(Press::Answer {
                id: "D-7".into(),
                n: 2
            })
        );
        assert_eq!(press("u:D-7"), Some(Press::Undo { id: "D-7".into() }));
        for bad in [
            "", "a:D-7", "a:D-7:x", "a:X-7:1", "u:D-", "u:D-7:1", "zzz", "a:D-7:+1",
        ] {
            assert_eq!(press(bad), None, "{bad}");
        }
    }

    #[test]
    fn commands() {
        assert_eq!(command("/on"), Some(Command::On));
        assert_eq!(command("/off@fake_bot"), Some(Command::Off));
        assert_eq!(command(" /status "), Some(Command::Status));
        assert_eq!(command("/help"), Some(Command::Help));
        assert_eq!(command("/start 123"), Some(Command::Help));
        assert_eq!(command("on"), None);
    }
}
