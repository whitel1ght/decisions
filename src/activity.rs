use crate::sessions::Session;

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
}
