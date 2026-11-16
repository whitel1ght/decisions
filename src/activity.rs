use std::path::PathBuf;

use chrono::DateTime;

use crate::agents;
use crate::queue::Decision;
use crate::sessions::Session;
use crate::transcript;

/// Declaration order is the screen's sort order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Idle,
    Waiting,
    Working,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Doing {
    pub text: String,
    /// Epoch ms.
    pub at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentActivity {
    pub label: String,
    pub status: Status,
    pub since: Option<i64>,
    pub doing: Option<Doing>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionActivity {
    pub session: Session,
    pub status: Status,
    pub since: Option<i64>,
    pub doing: Option<Doing>,
    pub agents: Vec<AgentActivity>,
    /// The oldest open question card's id.
    pub card: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Activity {
    pub sessions: Vec<SessionActivity>,
    pub others: Vec<AgentActivity>,
    pub at: i64,
}

impl Activity {
    fn count(&self, status: Status) -> usize {
        self.sessions.iter().filter(|s| s.status == status).count()
    }

    fn count_others(&self, status: Status) -> usize {
        self.others.iter().filter(|a| a.status == status).count()
    }

    pub fn working(&self) -> usize {
        self.count(Status::Working) + self.count_others(Status::Working)
    }

    pub fn waiting(&self) -> usize {
        self.count(Status::Waiting)
    }

    pub fn idle(&self) -> usize {
        self.count(Status::Idle)
    }

    pub fn unknown(&self) -> usize {
        self.count(Status::Unknown) + self.count_others(Status::Unknown)
    }

    pub fn sort(&mut self) {
        self.sessions
            .sort_by_cached_key(|s| (s.status, s.session.label().to_lowercase()));
    }
}

pub struct Signals<'a> {
    pub reported: &'a str,
    pub card: bool,
    pub background_jobs: usize,
    pub agents: &'a [AgentActivity],
}

pub fn classify(s: &Signals) -> Status {
    let any = |status| s.agents.iter().any(|a| a.status == status);
    if s.card || s.reported == "needs input" {
        Status::Waiting
    } else if s.reported == "working" || s.background_jobs > 0 || any(Status::Working) {
        Status::Working
    } else if any(Status::Unknown) {
        Status::Unknown
    } else if s.reported == "idle" {
        Status::Idle
    } else {
        Status::Unknown
    }
}

pub struct Sources {
    pub claude_projects: PathBuf,
    pub pi_sub: PathBuf,
}

impl Sources {
    pub fn from_env() -> Self {
        let var = |name| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = || var("HOME").unwrap_or_default();
        Self {
            claude_projects: var("CLAUDE_CONFIG_DIR")
                .unwrap_or_else(|| home().join(".claude"))
                .join("projects"),
            pi_sub: var("PI_SUB_DIR").unwrap_or_else(|| home().join(".local/state/pi-sub")),
        }
    }
}

fn asked_ms(card: &Decision) -> Option<i64> {
    DateTime::parse_from_rfc3339(&card.asked)
        .ok()
        .map(|t| t.timestamp_millis())
}

fn source_prefix(s: &Session) -> String {
    format!("{} · {}", s.client, s.dir_name())
}

fn oldest_card<'a>(s: &Session, cards: &'a [Decision], all: &[Session]) -> Option<&'a Decision> {
    let prefix = source_prefix(s);
    let by_source = |card: &Decision| {
        card.session.is_empty()
            && card.source.starts_with(&prefix)
            && all
                .iter()
                .filter(|o| card.source.starts_with(&source_prefix(o)))
                .count()
                == 1
    };
    cards
        .iter()
        .filter(|c| !c.is_answer() && (c.session == s.owner || by_source(c)))
        .min_by_key(|c| asked_ms(c).unwrap_or(i64::MAX))
}

/// The oldest open question card for `s`: one it owns, or an old one with no session whose
/// source names only this session.
pub fn card_for(s: &Session, cards: &[Decision], all: &[Session]) -> Option<String> {
    oldest_card(s, cards, all).map(|c| c.id.clone())
}

pub fn scan(
    src: &Sources,
    sessions: Vec<Session>,
    cards: &[Decision],
    now: i64,
    detail: bool,
) -> Activity {
    let mut pi = agents::pi_sub(&src.pi_sub, now);
    let mut rows = Vec::new();
    for s in &sessions {
        let card = oldest_card(s, cards, &sessions);
        let id = if s.current.is_empty() {
            &s.owner
        } else {
            &s.current
        };
        let path = s
            .is_claude()
            .then(|| transcript::find(&src.claude_projects, id))
            .flatten();
        let records = path
            .as_ref()
            .map(|p| transcript::tail(p, transcript::TAIL_CAP))
            .unwrap_or_default();
        let mut agent_list = path
            .as_ref()
            .map(|p| agents::claude(&p.with_extension(""), now, detail))
            .unwrap_or_default();
        let mine = std::mem::take(&mut pi);
        let (own, rest): (Vec<_>, Vec<_>) = mine.into_iter().partition(|(p, _)| *p == Some(s.pid));
        pi = rest;
        agent_list.extend(own.into_iter().map(|(_, a)| a));
        let status = classify(&Signals {
            reported: &s.state,
            card: card.is_some(),
            background_jobs: transcript::open_background(&records, now),
            agents: &agent_list,
        });
        let own_since = (s.since > 0).then_some(s.since);
        let since = match status {
            Status::Waiting => card.and_then(asked_ms),
            Status::Working if s.is_claude() => transcript::turn_start(&records),
            Status::Idle if s.is_claude() => transcript::last_at(&records),
            Status::Working | Status::Idle => own_since,
            Status::Unknown => None,
        };
        rows.push(SessionActivity {
            session: s.clone(),
            status,
            since,
            doing: if detail {
                transcript::doing(&records)
            } else {
                None
            },
            agents: agent_list,
            card: card.map(|c| c.id.clone()),
        });
    }
    let mut activity = Activity {
        sessions: rows,
        others: pi.into_iter().map(|(_, a)| a).collect(),
        at: now,
    };
    activity.sort();
    activity
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(status: Status) -> AgentActivity {
        AgentActivity {
            label: "a".into(),
            status,
            since: None,
            doing: None,
        }
    }

    fn sig<'a>(reported: &'a str, agents: &'a [AgentActivity]) -> Signals<'a> {
        Signals {
            reported,
            card: false,
            background_jobs: 0,
            agents,
        }
    }

    fn row(name: &str, status: Status) -> SessionActivity {
        SessionActivity {
            session: Session {
                name: name.into(),
                ..Session::default()
            },
            status,
            since: None,
            doing: None,
            agents: vec![],
            card: None,
        }
    }

    #[test]
    fn an_open_card_wins_over_working() {
        let s = Signals {
            card: true,
            ..sig("working", &[])
        };
        assert_eq!(classify(&s), Status::Waiting);
    }

    #[test]
    fn needs_input_is_waiting() {
        assert_eq!(classify(&sig("needs input", &[])), Status::Waiting);
    }

    #[test]
    fn a_running_agent_makes_an_idle_session_working() {
        let a = [agent(Status::Working)];
        assert_eq!(classify(&sig("idle", &a)), Status::Working);
    }

    #[test]
    fn a_background_job_makes_it_working() {
        let s = Signals {
            background_jobs: 1,
            ..sig("idle", &[])
        };
        assert_eq!(classify(&s), Status::Working);
    }

    #[test]
    fn an_unknown_agent_makes_an_idle_session_unknown() {
        let a = [agent(Status::Unknown)];
        assert_eq!(classify(&sig("idle", &a)), Status::Unknown);
    }

    #[test]
    fn idle_is_idle_and_anything_else_unknown() {
        assert_eq!(classify(&sig("idle", &[])), Status::Idle);
        assert_eq!(classify(&sig("", &[])), Status::Unknown);
        assert_eq!(classify(&sig("starting", &[])), Status::Unknown);
    }

    #[test]
    fn counts_include_other_agents_and_sort_puts_idle_first() {
        let mut act = Activity {
            sessions: vec![
                row("b", Status::Working),
                row("a", Status::Idle),
                row("c", Status::Waiting),
                row("d", Status::Unknown),
            ],
            others: vec![agent(Status::Working), agent(Status::Unknown)],
            at: 0,
        };
        assert_eq!(act.working(), 2);
        assert_eq!(act.waiting(), 1);
        assert_eq!(act.idle(), 1);
        assert_eq!(act.unknown(), 2);
        act.sort();
        let labels: Vec<_> = act.sessions.iter().map(|s| s.session.label()).collect();
        assert_eq!(labels, ["a", "c", "b", "d"]);
    }

    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);
    const NOW: i64 = 1_800_000_000_000;

    fn sources() -> Sources {
        let root = std::env::temp_dir().join(format!(
            "decisions-activity-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let src = Sources {
            claude_projects: root.join("projects"),
            pi_sub: root.join("pi-sub"),
        };
        std::fs::create_dir_all(src.claude_projects.join("-p")).unwrap();
        std::fs::create_dir_all(&src.pi_sub).unwrap();
        src
    }

    fn transcript(src: &Sources, id: &str) {
        let lines = [
            r#"{"type":"user","timestamp":"2026-11-05T21:09:00.000Z","message":{"content":"go"}}"#,
            r#"{"type":"assistant","timestamp":"2026-11-05T21:10:00.000Z","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#,
        ];
        let path = src.claude_projects.join("-p").join(format!("{id}.jsonl"));
        std::fs::write(path, lines.join("\n")).unwrap();
    }

    fn claude(owner: &str, state: &str) -> Session {
        Session {
            owner: owner.into(),
            pid: 10,
            client: "claude-code".into(),
            cwd: "/work/proj".into(),
            state: state.into(),
            ..Session::default()
        }
    }

    fn card(id: &str, session: &str, source: &str, asked: &str) -> Decision {
        Decision {
            id: id.into(),
            session: session.into(),
            source: source.into(),
            asked: asked.into(),
            ..Default::default()
        }
    }

    fn pi_agent(root: &Path, id: &str, parent: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("meta.json"),
            format!(r#"{{"label":"{id}","parent_pid":{parent}}}"#),
        )
        .unwrap();
    }

    #[test]
    fn a_session_with_an_open_card_is_waiting_and_names_it() {
        let src = sources();
        let s = claude("a", "working");
        let cards = [
            card("new", "a", "", "2026-11-05T21:00:00Z"),
            card("old", "a", "", "2026-11-05T20:00:00Z"),
            card("other", "b", "", "2026-11-05T19:00:00Z"),
        ];
        let act = scan(&src, vec![s], &cards, NOW, false);
        let row = &act.sessions[0];
        assert_eq!(row.status, Status::Waiting);
        assert_eq!(row.card.as_deref(), Some("old"));
        assert_eq!(row.since, Some(1_793_908_800_000));
    }

    #[test]
    fn an_old_card_without_a_session_matches_by_source_only_when_unambiguous() {
        let a = Session {
            cwd: "/work/alpha".into(),
            ..claude("a", "idle")
        };
        let b = Session {
            cwd: "/work/beta".into(),
            pid: 11,
            ..claude("b", "idle")
        };
        let twin = Session {
            pid: 12,
            ..claude("c", "idle")
        };
        let cards = [card("x", "", "claude-code · alpha", "2026-11-05T20:00:00Z")];
        let all = [a.clone(), b.clone()];
        assert_eq!(card_for(&a, &cards, &all), Some("x".into()));
        assert_eq!(card_for(&b, &cards, &all), None);
        let all = [
            a.clone(),
            Session {
                cwd: "/else/alpha".into(),
                ..twin
            },
        ];
        assert_eq!(card_for(&a, &cards, &all), None);
    }

    #[test]
    fn answer_cards_do_not_make_a_session_wait() {
        let mut c = card("ans", "a", "", "2026-11-05T20:00:00Z");
        c.kind = "answer".into();
        let s = claude("a", "idle");
        assert_eq!(card_for(&s, &[c], std::slice::from_ref(&s)), None);
    }

    #[test]
    fn a_cleared_session_reads_its_current_transcript_but_owner_cards() {
        let src = sources();
        transcript(&src, "new-id");
        let s = Session {
            current: "new-id".into(),
            ..claude("old-id", "idle")
        };
        let cards = [card("c", "old-id", "", "2026-11-05T20:00:00Z")];
        let act = scan(&src, vec![s.clone()], &cards, NOW, true);
        assert_eq!(act.sessions[0].card.as_deref(), Some("c"));
        assert_eq!(act.sessions[0].doing.as_ref().unwrap().text, "Bash: ls");
        let act = scan(&src, vec![s], &[], NOW, true);
        assert_eq!(act.sessions[0].status, Status::Idle);
        assert!(act.sessions[0].since.is_some());
    }

    #[test]
    fn registry_only_claude_session_is_unknown() {
        let src = sources();
        let act = scan(&src, vec![claude("a", "")], &[], NOW, false);
        assert_eq!(act.sessions[0].status, Status::Unknown);
        assert_eq!(act.sessions[0].since, None);
    }

    #[test]
    fn orphan_pi_sub_agents_go_to_others() {
        let src = sources();
        pi_agent(&src.pi_sub, "mine", "10");
        pi_agent(&src.pi_sub, "stray", "999");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let act = scan(&src, vec![claude("a", "idle")], &[], now, false);
        assert_eq!(act.sessions[0].agents.len(), 1);
        assert_eq!(act.sessions[0].agents[0].label, "mine");
        assert_eq!(act.others.len(), 1);
        assert_eq!(act.others[0].label, "stray");
    }

    #[test]
    fn doing_is_left_out_without_detail() {
        let src = sources();
        transcript(&src, "a");
        let s = claude("a", "working");
        let act = scan(&src, vec![s.clone()], &[], NOW, false);
        assert_eq!(act.sessions[0].doing, None);
        assert!(act.sessions[0].since.is_some());
        let act = scan(&src, vec![s], &[], NOW, true);
        assert!(act.sessions[0].doing.is_some());
    }
}
