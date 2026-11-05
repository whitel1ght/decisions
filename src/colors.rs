//! The colour a session's cards wear: its own from Claude Code's `/color`, else one the deck
//! assigns it. `<dir>/colors/sessions.json` remembers both, so a card keeps its colour after its
//! session ends, and how far each transcript has been read, since one can run to hundreds of
//! megabytes with the last colour record anywhere in it. It is in a folder of its own because the
//! deck reloads on any change in `<dir>`, and each reload would write it again.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::queue::Decision;
use crate::sessions::{self, Session};
use crate::store::write_atomic;
use crate::theme;

const FOLDER: &str = "colors";
const FILE: &str = "sessions.json";
/// Starts a line of its own in a transcript. A transcript quoting this text has it escaped.
const MARK: &[u8] = b"{\"type\":\"agent-color\",\"agentColor\":\"";
/// What `/color default` writes: the session no longer has a colour of its own.
const UNSET: &str = "default";
/// Red is left out, since on a card it reads as something wrong.
pub const ASSIGNED: [&str; 7] = [
    "blue", "green", "yellow", "purple", "cyan", "orange", "pink",
];
const CHUNK: usize = 1 << 20;
/// A record cut by the end of a chunk, or still being written, is read again.
const OVERLAP: usize = 256;
const KEEP: usize = 256;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Known {
    /// By the id on the cards: what `/color` last said.
    #[serde(default)]
    own: BTreeMap<String, String>,
    /// By the id on the cards: what the deck picked.
    #[serde(default)]
    assigned: BTreeMap<String, String>,
    /// By transcript id.
    #[serde(default)]
    scanned: BTreeMap<String, Scan>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Scan {
    /// Where the next read starts.
    offset: u64,
    /// Just past the last record taken: the overlap reads it again, and it is not news then.
    #[serde(default)]
    taken: u64,
}

fn projects() -> PathBuf {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")))
        .unwrap_or_default();
    config.join("projects")
}

/// Sets each card's `color` from the session that wrote it. A card with no session has none.
pub fn paint(dir: &Path, list: &mut [Decision]) {
    if list.iter().all(|d| d.session.is_empty()) {
        return;
    }
    paint_with(dir, &projects(), &sessions::running(dir), list);
}

fn paint_with(dir: &Path, projects: &Path, running: &[Session], list: &mut [Decision]) {
    let folder = dir.join(FOLDER);
    let path = folder.join(FILE);
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    let mut known: Known = serde_json::from_str(&before).unwrap_or_default();
    let mut owners: Vec<String> = list
        .iter()
        .filter(|d| !d.session.is_empty())
        .map(|d| d.session.clone())
        .collect();
    owners.sort();
    owners.dedup();

    let mut used_ids = HashSet::new();
    for owner in &owners {
        // `/clear` gives a session a new id, and the cards keep the one it started with. A pi
        // started from a Claude session's shell shares its owner and has no current id.
        let current = running
            .iter()
            .find(|s| &s.owner == owner && !s.current.is_empty() && &s.current != owner)
            .map(|s| s.current.as_str());
        // Only a new record changes anything: an older transcript's last word may be out of date,
        // and another id the session had since may no longer be in view.
        let mut said = None;
        for id in current.into_iter().chain([owner.as_str()]) {
            used_ids.insert(id.to_string());
            let new = scan(&mut known, projects, id);
            said = said.or(new);
        }
        match said.as_deref() {
            Some(UNSET) => {
                known.own.remove(owner);
            }
            Some(c) if theme::session_color(c).is_some() => {
                known.own.insert(owner.clone(), c.to_string());
            }
            _ => {}
        }
    }

    let colour_of = |known: &Known, owner: &String| {
        known
            .own
            .get(owner)
            .or_else(|| known.assigned.get(owner))
            .cloned()
    };
    let mut in_use: HashMap<String, usize> = HashMap::new();
    for owner in &owners {
        if let Some(c) = colour_of(&known, owner) {
            *in_use.entry(c).or_default() += 1;
        }
    }
    for owner in &owners {
        if colour_of(&known, owner).is_none() {
            let c = pick(owner, &in_use);
            *in_use.entry(c.to_string()).or_default() += 1;
            known.assigned.insert(owner.clone(), c.to_string());
        }
    }
    for d in list.iter_mut() {
        d.color = colour_of(&known, &d.session).unwrap_or_default();
    }

    if known.own.len() + known.assigned.len() + known.scanned.len() > KEEP {
        known.own.retain(|k, _| owners.contains(k));
        known.assigned.retain(|k, _| owners.contains(k));
        known.scanned.retain(|k, _| used_ids.contains(k));
    }
    if let Ok(text) = serde_json::to_string_pretty(&known)
        && text != before
    {
        // Colours are a nicety: failing to remember them must not get in the way of the cards.
        let _ = std::fs::create_dir_all(&folder);
        let _ = write_atomic(&path, &text);
    }
}

/// The colour other sessions' open cards use least, starting at a place that depends on the
/// session, so sessions do not all take colours in the same order.
fn pick(owner: &str, in_use: &HashMap<String, usize>) -> &'static str {
    let mut hasher = DefaultHasher::new();
    owner.hash(&mut hasher);
    let start = (hasher.finish() % ASSIGNED.len() as u64) as usize;
    (0..ASSIGNED.len())
        .map(|i| ASSIGNED[(start + i) % ASSIGNED.len()])
        .min_by_key(|c| in_use.get(*c).copied().unwrap_or(0))
        .unwrap_or(ASSIGNED[0])
}

/// The last colour record added to a session's transcript since the last time, if any.
fn scan(known: &mut Known, projects: &Path, id: &str) -> Option<String> {
    let path = transcript(projects, id)?;
    let seen = known.scanned.entry(id.to_string()).or_default();
    let len = std::fs::metadata(&path).ok()?.len();
    if seen.offset > len {
        *seen = Scan::default();
    }
    let (name, at) = last_record(&path, seen.offset, seen.taken).ok()??;
    seen.offset = len.saturating_sub(OVERLAP as u64);
    seen.taken = at + 1;
    Some(name)
}

/// A session's transcript, in whichever project folder it started.
fn transcript(projects: &Path, id: &str) -> Option<PathBuf> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|e| e.path().join(format!("{id}.jsonl")))
        .find(|p| p.is_file())
}

/// The last complete record at or after `taken`, reading from `from`, and where it starts.
fn last_record(path: &Path, from: u64, taken: u64) -> io::Result<Option<(String, u64)>> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(from))?;
    let finder = memchr::memmem::Finder::new(MARK);
    let mut buf = vec![0; CHUNK + OVERLAP];
    let (mut have, mut base, mut found) = (0, from, None);
    loop {
        let n = file.read(&mut buf[have..])?;
        if n == 0 {
            return Ok(found);
        }
        have += n;
        for start in finder.find_iter(&buf[..have]) {
            let at = base + start as u64;
            if at >= taken
                && let Some(name) = name_at(&buf[start + MARK.len()..have])
            {
                found = Some((name, at));
            }
        }
        let keep = have.min(OVERLAP);
        buf.copy_within(have - keep..have, 0);
        base += (have - keep) as u64;
        have = keep;
    }
}

fn name_at(rest: &[u8]) -> Option<String> {
    let end = rest.iter().position(|b| *b == b'"')?;
    std::str::from_utf8(&rest[..end]).ok().map(String::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};

    static NEXT: AtomicU32 = AtomicU32::new(0);

    struct Fix {
        dir: PathBuf,
        projects: PathBuf,
    }

    fn fix() -> Fix {
        let root = std::env::temp_dir().join(format!(
            "decisions-colors-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        let (dir, projects) = (root.join("q"), root.join("projects"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(projects.join("-some-project")).unwrap();
        Fix { dir, projects }
    }

    impl Fix {
        fn memory(&self) -> PathBuf {
            self.dir.join(FOLDER).join(FILE)
        }

        fn append(&self, id: &str, text: &str) {
            let mut f = File::options()
                .create(true)
                .append(true)
                .open(self.projects.join(format!("-some-project/{id}.jsonl")))
                .unwrap();
            f.write_all(text.as_bytes()).unwrap();
        }

        fn say(&self, id: &str, colour: &str) {
            self.append(
                id,
                &format!(
                    "{{\"type\":\"agent-color\",\"agentColor\":\"{colour}\",\"sessionId\":\"{id}\"}}\n"
                ),
            );
        }

        fn colours(&self, running: &[Session], owners: &[&str]) -> Vec<String> {
            let mut list: Vec<Decision> = owners.iter().map(|o| card(o)).collect();
            paint_with(&self.dir, &self.projects, running, &mut list);
            list.into_iter().map(|d| d.color).collect()
        }
    }

    fn running(owner: &str) -> Vec<Session> {
        vec![Session {
            owner: owner.into(),
            ..Default::default()
        }]
    }

    fn card(owner: &str) -> Decision {
        let mut d = crate::queue::decode(r#"[{"id":"D-1","question":"Q","asked":""}]"#).unwrap();
        let mut d = d.remove(0);
        d.session = owner.into();
        d
    }

    #[test]
    fn a_session_wears_its_own_colour_and_the_last_one_wins() {
        let f = fix();
        let live = running("s-1");
        f.say("s-1", "red");
        assert_eq!(f.colours(&live, &["s-1"]), ["red"]);
        f.append("s-1", "{\"type\":\"user\"}\n");
        f.say("s-1", "pink");
        assert_eq!(f.colours(&live, &["s-1"]), ["pink"]);
    }

    #[test]
    fn a_session_with_none_keeps_the_colour_it_was_assigned() {
        let f = fix();
        let first = f.colours(&[], &["s-1", "s-2"]);
        assert!(ASSIGNED.contains(&first[0].as_str()), "{first:?}");
        assert_ne!(first[0], first[1], "two sessions, two colours");
        assert_eq!(
            f.colours(&[], &["s-2", "s-1"]),
            [first[1].clone(), first[0].clone()]
        );
        assert_eq!(
            f.colours(&[], &["s-1"]),
            [first[0].clone()],
            "alone it keeps it too"
        );
    }

    #[test]
    fn every_session_gets_a_different_colour_while_any_are_left() {
        let f = fix();
        let owners: Vec<String> = (0..ASSIGNED.len()).map(|i| format!("s-{i}")).collect();
        let refs: Vec<&str> = owners.iter().map(String::as_str).collect();
        let got: HashSet<String> = f.colours(&[], &refs).into_iter().collect();
        assert_eq!(got.len(), ASSIGNED.len());
    }

    #[test]
    fn an_assigned_colour_avoids_one_a_session_chose() {
        let f = fix();
        for (i, name) in ASSIGNED.iter().enumerate().skip(1) {
            f.say(&format!("s-{i}"), name);
        }
        let got = f.colours(&[], &["s-1", "s-2", "s-3", "s-4", "s-5", "s-6", "new"]);
        assert_eq!(got[6], ASSIGNED[0], "the only one nobody uses: {got:?}");
    }

    #[test]
    fn default_gives_back_the_sessions_own_colour() {
        let f = fix();
        let live = running("s-1");
        f.say("s-1", "red");
        assert_eq!(f.colours(&live, &["s-1"]), ["red"]);
        f.say("s-1", "default");
        let assigned = f.colours(&live, &["s-1"]);
        assert!(ASSIGNED.contains(&assigned[0].as_str()), "{assigned:?}");
        assert_eq!(f.colours(&live, &["s-1"]), assigned, "and stays put");
    }

    #[test]
    fn a_colour_set_after_clear_is_on_the_sessions_current_id() {
        let f = fix();
        f.say("start", "red");
        let run = |current: &str| Session {
            owner: "start".into(),
            current: current.into(),
            ..Default::default()
        };
        assert_eq!(f.colours(&[run("now")], &["start"]), ["red"], "not yet");
        f.say("now", "cyan");
        assert_eq!(f.colours(&[run("now")], &["start"]), ["cyan"]);
        assert_eq!(f.colours(&[], &["start"]), ["cyan"], "after it ends too");
    }

    fn cleared_to(current: &str) -> Vec<Session> {
        vec![Session {
            owner: "start".into(),
            current: current.into(),
            ..Default::default()
        }]
    }

    #[test]
    fn a_reset_after_clear_lasts_after_the_session_ends() {
        let f = fix();
        f.say("start", "red");
        f.say("now", "default");
        let assigned = f.colours(&cleared_to("now"), &["start"]);
        assert_ne!(assigned, ["red"]);
        assert_eq!(f.colours(&cleared_to("now"), &["start"]), assigned);
        assert_eq!(f.colours(&[], &["start"]), assigned, "after it ends");
    }

    #[test]
    fn a_second_clear_does_not_bring_back_an_older_colour() {
        let f = fix();
        f.say("start", "red");
        assert_eq!(f.colours(&cleared_to("now"), &["start"]), ["red"]);
        f.say("now", "cyan");
        assert_eq!(f.colours(&cleared_to("now"), &["start"]), ["cyan"]);
        assert_eq!(f.colours(&cleared_to("then"), &["start"]), ["cyan"]);

        f.say("then", "default");
        let assigned = f.colours(&cleared_to("then"), &["start"]);
        assert_ne!(assigned, ["red"]);
        assert_eq!(f.colours(&cleared_to("later"), &["start"]), assigned);
    }

    #[test]
    fn a_failed_session_list_does_not_change_a_colour() {
        let f = fix();
        f.say("start", "red");
        f.say("now", "cyan");
        assert_eq!(f.colours(&cleared_to("now"), &["start"]), ["cyan"]);
        assert_eq!(f.colours(&cleared_to(""), &["start"]), ["cyan"]);
        assert_eq!(f.colours(&cleared_to("now"), &["start"]), ["cyan"]);
    }

    #[test]
    fn a_pi_sharing_the_id_does_not_hide_the_current_one() {
        let f = fix();
        f.say("start", "red");
        f.say("now", "cyan");
        let mut rows = cleared_to("");
        rows.extend(cleared_to("now"));
        assert_eq!(f.colours(&rows, &["start"]), ["cyan"]);
    }

    #[test]
    fn a_record_near_the_end_is_news_only_once() {
        let f = fix();
        f.say("start", "red");
        assert_eq!(f.colours(&cleared_to("now"), &["start"]), ["red"]);
        f.say("now", "cyan");
        assert_eq!(f.colours(&cleared_to("now"), &["start"]), ["cyan"]);
        f.say("start", "default");
        let assigned = f.colours(&cleared_to("now"), &["start"]);
        let known: Known =
            serde_json::from_str(&std::fs::read_to_string(f.memory()).unwrap()).unwrap();
        assert!(known.own.is_empty(), "a real new record still counts");
        assert_eq!(f.colours(&cleared_to("now"), &["start"]), assigned);
    }

    #[test]
    fn a_record_still_being_written_is_read_when_it_is_whole() {
        let f = fix();
        f.say("s-1", "red");
        let live = running("s-1");
        assert_eq!(f.colours(&live, &["s-1"]), ["red"]);
        f.append("s-1", "{\"type\":\"agent-color\",\"agentColor\":\"cy");
        assert_eq!(f.colours(&live, &["s-1"]), ["red"]);
        f.append("s-1", "an\",\"sessionId\":\"s-1\"}\n");
        assert_eq!(f.colours(&live, &["s-1"]), ["cyan"]);
    }

    #[test]
    fn a_record_across_two_chunks_and_a_long_transcript_are_read() {
        let f = fix();
        f.say("s-1", "red");
        let pad = CHUNK - MARK.len() / 2 - 41;
        f.append("s-1", &format!("{}\n", "x".repeat(pad)));
        f.say("s-1", "cyan");
        f.append("s-1", &format!("{}\n", "y".repeat(3 * CHUNK)));
        assert_eq!(f.colours(&[], &["s-1"]), ["cyan"]);
    }

    #[test]
    fn only_what_is_new_is_read_again() {
        let f = fix();
        f.say("s-1", "red");
        f.append("s-1", &format!("{}\n", "x".repeat(1000)));
        assert_eq!(f.colours(&[], &["s-1"]), ["red"]);
        let known: Known =
            serde_json::from_str(&std::fs::read_to_string(f.memory()).unwrap()).unwrap();
        assert!(known.scanned["s-1"].offset > 0);
        // A record before that point is not looked at again; the colour stays remembered.
        assert_eq!(f.colours(&[], &["s-1"]), ["red"]);
    }

    #[test]
    fn nothing_new_writes_nothing_and_nothing_lands_where_the_deck_watches() {
        let f = fix();
        for i in 0..8 {
            f.say(&format!("s-{i}"), "red");
        }
        let owners = [
            "s-0", "s-1", "s-2", "s-3", "s-4", "s-5", "s-6", "s-7", "a", "b", "c",
        ];
        f.colours(&[], &owners);
        let at = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        File::options()
            .write(true)
            .open(f.memory())
            .unwrap()
            .set_modified(at)
            .unwrap();
        let kept = std::fs::read_to_string(f.memory()).unwrap();
        for _ in 0..5 {
            f.colours(&[], &owners);
            assert_eq!(std::fs::read_to_string(f.memory()).unwrap(), kept);
        }
        let now = std::fs::metadata(f.memory()).unwrap().modified().unwrap();
        assert_eq!(now, at, "a write would make the deck reload");
        let top: Vec<_> = std::fs::read_dir(&f.dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(top, [FOLDER], "only the folder itself");
    }

    #[test]
    fn a_quoted_record_and_an_unknown_name_are_not_a_colour() {
        let f = fix();
        f.append(
            "s-1",
            "{\"text\":\"{\\\"type\\\":\\\"agent-color\\\",\\\"agentColor\\\":\\\"red\\\"}\"}\n",
        );
        let got = f.colours(&[], &["s-1"]);
        assert_ne!(got, ["red"]);
        f.say("s-1", "chartreuse");
        assert_eq!(f.colours(&[], &["s-1"]), got);
    }

    #[test]
    fn a_card_with_no_session_has_no_colour_and_an_odd_id_reads_no_file() {
        let f = fix();
        assert_eq!(f.colours(&[], &[""]), [""]);
        assert_eq!(transcript(&f.projects, "../secret"), None);
        assert_eq!(transcript(&f.projects, "pid:123"), None);
        assert!(!f.colours(&[], &["pid:123"])[0].is_empty());
    }

    #[test]
    fn a_broken_memory_starts_over() {
        let f = fix();
        std::fs::create_dir_all(f.memory().parent().unwrap()).unwrap();
        std::fs::write(f.memory(), "not json").unwrap();
        f.say("s-1", "red");
        assert_eq!(f.colours(&[], &["s-1"]), ["red"]);
    }
}
