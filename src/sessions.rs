//! The running sessions a prompt from the deck can reach. Each `decisions mcp` server registers
//! the session it serves under `<dir>/sessions/`; the file goes when the server exits, and a
//! reader drops one whose harness process is gone. Claude Code's own list adds names and states.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::store::write_atomic;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    /// The id a card's `session` field holds, so the session picks up what is sent to it.
    pub owner: String,
    /// The harness process: the MCP server's parent.
    pub pid: u32,
    /// The MCP server that wrote this, so only it removes what it wrote.
    #[serde(default)]
    pub server: u32,
    pub client: String,
    pub cwd: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub state: String,
}

impl Session {
    /// The name to show: the session's own, else its directory.
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            self.dir_name()
        } else {
            &self.name
        }
    }

    pub fn dir_name(&self) -> &str {
        Path::new(&self.cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&self.cwd)
    }

    fn is_claude(&self) -> bool {
        self.client.contains("claude")
    }
}

fn folder(dir: &Path) -> PathBuf {
    dir.join("sessions")
}

/// One file per harness process: two processes can share an owner id, as a pi started from a
/// Claude session's shell inherits its id.
fn file(dir: &Path, pid: u32) -> PathBuf {
    folder(dir).join(format!("{pid}.json"))
}

/// Registers `session`; returns the file to `unregister` when it ends.
pub fn register(dir: &Path, session: &Session) -> Result<PathBuf, String> {
    let path = file(dir, session.pid);
    std::fs::create_dir_all(folder(dir)).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(session).map_err(|e| e.to_string())?;
    write_atomic(&path, &text).map_err(|e| e.0)?;
    Ok(path)
}

/// Removes `path` unless a newer server has written over it since: a restarted server can
/// register before the old one exits.
pub fn unregister(path: &Path, server: u32) {
    let mine = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<Session>(&t).ok())
        .is_some_and(|s| s.server == server);
    if mine {
        let _ = std::fs::remove_file(path);
    }
}

fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Every registered session whose harness still runs; the others' files are removed.
pub fn registered(dir: &Path) -> Vec<Session> {
    let Ok(entries) = std::fs::read_dir(folder(dir)) else {
        return Vec::new();
    };
    let mut list = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        if path.extension().is_none_or(|x| x != "json") {
            continue;
        }
        let session = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Session>(&t).ok());
        match session {
            Some(s) if alive(s.pid) => list.push(s),
            _ => {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    list
}

/// A row of `claude agents --json`.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Agent {
    #[serde(default)]
    pub pid: u32,
    #[serde(default)]
    pub name: String,
    /// A background session's progress.
    #[serde(default)]
    pub state: String,
    /// All an interactive session reports.
    #[serde(default)]
    pub status: String,
}

fn state_label(a: &Agent) -> String {
    let state = if a.state.is_empty() {
        &a.status
    } else {
        &a.state
    };
    match state.as_str() {
        "done" => "idle".into(),
        "busy" => "working".into(),
        other => other.replace('_', " "),
    }
}

/// Names and states from Claude Code's list. A Claude session missing from a list that could be
/// read is a headless run (`claude -p`), which nobody can talk to: it is left out.
pub fn merge(mut list: Vec<Session>, agents: Option<&[Agent]>) -> Vec<Session> {
    if let Some(agents) = agents {
        list.retain_mut(|s| {
            if !s.is_claude() {
                return true;
            }
            let Some(a) = agents.iter().find(|a| a.pid == s.pid) else {
                return false;
            };
            if !a.name.is_empty() {
                s.name = a.name.clone();
            }
            s.state = state_label(a);
            true
        });
    }
    list.sort_by_key(|s| s.label().to_lowercase());
    list
}

fn claude_agents() -> Option<Vec<Agent>> {
    let out = Command::new("claude")
        .args(["agents", "--json"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

/// The sessions a prompt can reach now.
pub fn running(dir: &Path) -> Vec<Session> {
    let list = registered(dir);
    let agents = list
        .iter()
        .any(Session::is_claude)
        .then(claude_agents)
        .flatten();
    merge(list, agents.as_deref())
}

/// The session `query` names: its owner id, its name, or the only one whose name or directory
/// contains it.
pub fn resolve<'a>(list: &'a [Session], query: &str) -> Result<&'a Session, String> {
    let q = query.trim().to_lowercase();
    if let Some(s) = list.iter().find(|s| s.owner == query.trim()) {
        return Ok(s);
    }
    let named: Vec<&Session> = list
        .iter()
        .filter(|s| s.label().to_lowercase() == q)
        .collect();
    let found = if named.is_empty() {
        list.iter().filter(|s| matches(s, &q)).collect()
    } else {
        named
    };
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("no running session matches {query}")),
        many => Err(format!(
            "{query} matches {}; name one by its id",
            many.iter()
                .map(|s| format!("{} ({})", s.label(), s.owner))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// `query`, already lower case, is in the session's name or directory.
pub fn matches(s: &Session, query: &str) -> bool {
    s.label().to_lowercase().contains(query) || s.dir_name().to_lowercase().contains(query)
}

const TARGET: &str = ".prompt-target";

/// The session the last prompt went to.
pub fn last_target(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(TARGET)).ok()?;
    Some(text.trim().to_string()).filter(|t| !t.is_empty())
}

pub fn save_target(dir: &Path, owner: &str) {
    let _ = std::fs::create_dir_all(dir);
    let _ = write_atomic(&dir.join(TARGET), owner);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("decisions-sessions-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn session(owner: &str, pid: u32, client: &str, cwd: &str) -> Session {
        Session {
            owner: owner.into(),
            pid,
            client: client.into(),
            cwd: cwd.into(),
            ..Session::default()
        }
    }

    #[test]
    fn a_registered_session_is_listed_while_its_harness_runs() {
        let d = dir("alive");
        let me = session("a/b", std::process::id(), "pi", "/x/dotfiles");
        register(&d, &me).unwrap();
        assert_eq!(registered(&d), vec![me]);
    }

    #[test]
    fn two_harnesses_sharing_an_owner_id_each_keep_their_row() {
        let d = dir("shared");
        let first = session("same", std::process::id(), "claude-code", "/x");
        let second = session("same", std::os::unix::process::parent_id(), "pi", "/x");
        register(&d, &first).unwrap();
        let path = register(&d, &second).unwrap();
        assert_eq!(registered(&d).len(), 2);
        unregister(&path, second.server);
        assert_eq!(registered(&d), vec![first]);
    }

    #[test]
    fn a_server_leaves_the_file_of_the_newer_server_that_replaced_it() {
        let d = dir("restart");
        let mut old = session("o", std::process::id(), "pi", "/x");
        old.server = 111;
        let path = register(&d, &old).unwrap();
        let mut new = old.clone();
        new.server = 222;
        register(&d, &new).unwrap();
        unregister(&path, 111);
        assert_eq!(registered(&d), vec![new]);
        unregister(&path, 222);
        assert!(registered(&d).is_empty());
    }

    #[test]
    fn a_dead_harness_drops_the_session_and_its_file() {
        let d = dir("dead");
        let path = register(&d, &session("gone", 4_000_000, "pi", "/x")).unwrap();
        assert!(registered(&d).is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn claude_names_and_states_are_merged_and_headless_runs_hidden() {
        let list = vec![
            session("c1", 10, "claude-code", "/x/dotfiles"),
            session("c2", 11, "claude-code", "/x/headless"),
            session("p1", 12, "pi", "/x/backend"),
        ];
        let agents = [Agent {
            pid: 10,
            name: "the system".into(),
            state: "done".into(),
            ..Agent::default()
        }];
        let got = merge(list, Some(&agents));
        let rows: Vec<_> = got.iter().map(|s| (s.label(), s.state.as_str())).collect();
        assert_eq!(rows, [("backend", ""), ("the system", "idle")]);
    }

    #[test]
    fn an_interactive_session_has_only_a_status() {
        let state = |status: &str, state: &str| {
            state_label(&Agent {
                status: status.into(),
                state: state.into(),
                ..Agent::default()
            })
        };
        assert_eq!(state("busy", ""), "working");
        assert_eq!(state("idle", ""), "idle");
        assert_eq!(state("busy", "done"), "idle", "a background state wins");
        assert_eq!(state("busy", "needs_input"), "needs input");
    }

    #[test]
    fn without_claudes_list_every_session_stays() {
        let list = vec![session("c1", 10, "claude-code", "/x/dotfiles")];
        assert_eq!(merge(list, None).len(), 1);
    }

    #[test]
    fn resolve_takes_an_owner_a_name_or_one_match() {
        let mut a = session("o1", 1, "pi", "/x/dotfiles");
        a.name = "the system".into();
        let b = session("o2", 2, "pi", "/x/backend");
        let c = session("o3", 3, "pi", "/x/backend-2");
        let list = [a, b, c];
        assert_eq!(resolve(&list, "o2").unwrap().owner, "o2");
        assert_eq!(resolve(&list, "The System").unwrap().owner, "o1");
        assert_eq!(resolve(&list, "dotf").unwrap().owner, "o1");
        assert_eq!(resolve(&list, "backend").unwrap().owner, "o2");
        assert!(resolve(&list, "back").unwrap_err().contains("matches"));
        assert!(resolve(&list, "nope").unwrap_err().contains("no running"));
    }

    #[test]
    fn two_sessions_with_the_same_name_are_never_guessed_between() {
        let list = [
            session("o1", 1, "pi", "/x/dotfiles"),
            session("o2", 2, "pi", "/y/dotfiles"),
        ];
        let err = resolve(&list, "dotfiles").unwrap_err();
        assert!(err.contains("o1") && err.contains("o2"), "{err}");
        assert_eq!(resolve(&list, "o2").unwrap().owner, "o2");
    }

    #[test]
    fn the_last_target_survives_between_decks() {
        let d = dir("target");
        assert_eq!(last_target(&d), None);
        save_target(&d, "o1");
        assert_eq!(last_target(&d).as_deref(), Some("o1"));
    }
}
