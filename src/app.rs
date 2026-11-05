//! All state and key handling. `update` is the whole behaviour: an event and the time in,
//! the new state and the commands for `main` to run out. Nothing here touches the
//! terminal, the clock or `decide`.

use std::collections::HashMap;

use crate::prompt::{self, Image};
use crate::queue::Decision;
use crate::sessions::{self, Session};
use crate::stats::Stats;
use crate::theme::Icons;

pub const UNDO_MS: i64 = 5_000;
pub const GHOST_MS: i64 = 200;
pub const FLIP_MS: i64 = 300;
pub const TOAST_MS: i64 = 3_000;
const HALF_PAGE: u16 = 10;
/// How many files the `@` list shows.
const FILE_ROWS: usize = 8;
/// How often the open prompt line re-reads the running sessions.
const SESSIONS_EVERY_MS: i64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Up,
    Down,
    Space,
    Backspace,
    Tab,
    CtrlD,
    CtrlU,
    CtrlV,
    Interrupt,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Key(Key),
    Tick,
    Changed,
    Loaded(Result<Vec<Decision>, String>),
    /// A `decide` call finished; `ask` tells an ask back from an answer on the same card.
    Sent {
        id: String,
        ask: bool,
        result: Result<Option<String>, String>,
    },
    Signal,
    /// The statistics asked for with `S` are ready.
    StatsLoaded(Result<Box<Stats>, String>),
    /// Text pasted into the terminal, in one piece.
    Paste(String),
    SessionsLoaded {
        sessions: Vec<Session>,
        last: Option<String>,
    },
    DirLoaded {
        dir: String,
        entries: Vec<String>,
    },
    FilesLoaded {
        cwd: String,
        files: Vec<String>,
    },
    ImagePasted(Result<Image, String>),
    /// A message from the prompt line went out; the card it opened, or why it did not.
    PromptSent(Result<String, String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Reload,
    LoadStats,
    Answer {
        id: String,
        n: u8,
        note: Option<String>,
    },
    Ask {
        id: String,
        text: String,
    },
    /// An answer in the user's own words, none of the options.
    Tell {
        id: String,
        text: String,
    },
    /// The user has read an answer card.
    Seen {
        id: String,
    },
    LoadSessions,
    LoadFiles(String),
    LoadDir {
        cwd: String,
        dir: String,
    },
    SaveTarget(String),
    PasteImage,
    SendPrompt {
        session: String,
        source: String,
        text: String,
    },
    OpenUrl(String),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Band {
    Fresh,
    Warm,
    Old,
    Stale,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Anim {
    Deal,
    Next,
    Prev,
    Throw,
    Arrive(String),
    Leave(String),
    AgeBand { id: String, from: Band, to: Band },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Side {
    #[default]
    Front,
    Back,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    /// A note on the answer in its undo window.
    Note,
    /// A note on a card before it is answered; the answer carries it.
    CardNote {
        id: String,
    },
    Ask {
        id: String,
    },
    /// The last slot: an answer in the user's own words.
    Tell {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub kind: InputKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    pub decision: Decision,
    pub n: u8,
    pub note: Option<String>,
    pub custom: Option<String>,
    pub due: i64,
    pub paused: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Failed {
    pub n: u8,
    pub note: Option<String>,
    pub custom: Option<String>,
    pub error: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AskFailed {
    pub text: String,
    pub error: String,
}

/// The list open above the prompt line.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum Pick {
    #[default]
    None,
    /// Running sessions, filtered by what follows the `#`.
    Sessions { query: String, sel: usize },
    /// The session's files for the `@` at byte `at` of the text.
    Files { at: usize, sel: usize },
}

/// The line for messages to running sessions. Closing it keeps the draft.
#[derive(Debug, Default)]
pub struct PromptLine {
    pub open: bool,
    pub text: String,
    pub images: Vec<Image>,
    /// The chosen session's owner id.
    pub target: Option<String>,
    pub pick: Pick,
    pub sessions: Vec<Session>,
    pub files: Vec<String>,
    pub files_cwd: String,
    /// The folder an `@` path is in, as typed, and what is in it.
    pub dir: String,
    pub dir_entries: Vec<String>,
    /// The draft of a message on its way, to put back if it fails.
    pub sending: Option<(String, Vec<Image>)>,
    /// Messages that failed as the deck closed, with why: reported on the way out.
    pub lost: Vec<(String, String)>,
    pub loaded_at: i64,
}

impl PromptLine {
    pub fn target_session(&self) -> Option<&Session> {
        let target = self.target.as_ref()?;
        self.sessions.iter().find(|s| &s.owner == target)
    }

    /// The sessions the open list shows.
    pub fn session_rows(&self) -> Vec<&Session> {
        let Pick::Sessions { query, .. } = &self.pick else {
            return Vec::new();
        };
        let q = query.to_lowercase();
        self.sessions
            .iter()
            .filter(|s| sessions::matches(s, &q))
            .collect()
    }

    /// The files the open list shows.
    pub fn file_rows(&self) -> Vec<String> {
        let Pick::Files { at, .. } = &self.pick else {
            return Vec::new();
        };
        let query = &self.text[at + 1..];
        match prompt::outside_path(query) {
            Some((dir, name)) if dir == self.dir => {
                prompt::path_rows(&self.dir_entries, dir, name, FILE_ROWS)
            }
            Some(_) => Vec::new(),
            None => prompt::search(&self.files, query, FILE_ROWS),
        }
    }
}

#[derive(Debug, Default)]
pub struct State {
    pub prompt: PromptLine,
    pub queue: Vec<Decision>,
    /// Cards kept out of the queue until their session replies.
    pub sent: usize,
    pub cur: usize,
    pub side: Side,
    pub cursor: Option<usize>,
    pub scroll: u16,
    pub scroll_max: u16,
    pub skipped: Vec<String>,
    pub pending: Option<Pending>,
    pub sending: HashMap<String, Pending>,
    pub failed: HashMap<String, Failed>,
    pub asking: HashMap<String, String>,
    pub ask_failed: HashMap<String, AskFailed>,
    pub notes: HashMap<String, String>,
    pub input: Option<Input>,
    pub help: bool,
    /// The stats screen, while it is open.
    pub stats: Option<Stats>,
    pub error: Option<String>,
    pub toast: Option<(String, i64)>,
    pub ghost: Option<(Decision, i64)>,
    pub flip: Option<i64>,
    pub anims: Vec<Anim>,
    pub bands: HashMap<String, Band>,
    pub now: i64,
    pub loaded: bool,
    pub quitting: bool,
    pub quit_failed: bool,
    pub demo: bool,
    pub title: String,
    pub icons: Icons,
}

pub fn band(age_ms: i64) -> Band {
    match age_ms / 60_000 {
        ..15 => Band::Fresh,
        15..60 => Band::Warm,
        60..180 => Band::Old,
        _ => Band::Stale,
    }
}

pub fn age_label(age_ms: i64) -> String {
    let m = age_ms.max(0) / 60_000;
    if m < 60 {
        format!("{m}m")
    } else {
        format!("{}h {:02}m", m / 60, m % 60)
    }
}

impl State {
    pub fn new(now: i64) -> Self {
        Self {
            now,
            title: "decisions".into(),
            ..Self::default()
        }
    }

    pub fn current(&self) -> Option<&Decision> {
        self.queue.get(self.cur)
    }

    pub fn age_ms(&self, d: &Decision) -> i64 {
        d.asked_ms().map_or(0, |at| (self.now - at).max(0))
    }

    pub fn oldest_ms(&self) -> i64 {
        self.queue.iter().map(|d| self.age_ms(d)).max().unwrap_or(0)
    }

    /// The flip or a thrown card's ghost is on screen, so frames are needed.
    pub fn animating(&self) -> bool {
        self.ghost.is_some() || self.flip.is_some()
    }

    fn toast(&mut self, msg: String) {
        self.toast = Some((msg, self.now + TOAST_MS));
    }

    fn reset_view(&mut self) {
        self.side = Side::Front;
        self.cursor = None;
        self.scroll = 0;
    }
}

pub fn update(s: &mut State, ev: Event, now: i64) -> Vec<Command> {
    s.now = now;
    let mut out = Vec::new();
    match ev {
        Event::Tick => tick(s, &mut out),
        Event::Changed => out.push(Command::Reload),
        Event::Loaded(Ok(list)) => loaded(s, list),
        Event::Loaded(Err(e)) => s.error = Some(e),
        Event::StatsLoaded(Ok(st)) => s.stats = Some(*st),
        Event::StatsLoaded(Err(e)) => s.toast(format!("stats failed: {e}")),
        Event::Sent { id, ask, result } => sent(s, id, ask, result, &mut out),
        Event::Paste(text) => paste(s, text, &mut out),
        Event::SessionsLoaded { sessions, last } => sessions_loaded(s, sessions, last, &mut out),
        Event::DirLoaded { dir, entries } => {
            if s.prompt.dir == dir {
                s.prompt.dir_entries = entries;
            }
        }
        Event::FilesLoaded { cwd, files } => {
            if s.prompt.files_cwd == cwd {
                s.prompt.files = files;
            }
        }
        Event::ImagePasted(Ok(image)) => s.prompt.images.push(image),
        Event::ImagePasted(Err(e)) => s.toast(e),
        Event::PromptSent(result) => prompt_sent(s, result),
        Event::Signal => {
            send_pending(s, &mut out);
            s.quitting = true;
            out.push(Command::Quit);
        }
        Event::Key(k) => key(s, k, &mut out),
    }
    out
}

fn tick(s: &mut State, out: &mut Vec<Command>) {
    if s.prompt.open && s.now - s.prompt.loaded_at >= SESSIONS_EVERY_MS {
        s.prompt.loaded_at = s.now;
        out.push(Command::LoadSessions);
    }
    if s.pending
        .as_ref()
        .is_some_and(|p| !p.paused && s.now >= p.due)
    {
        send_pending(s, out);
    }
    if s.ghost.as_ref().is_some_and(|(_, until)| s.now >= *until) {
        s.ghost = None;
    }
    if s.flip.is_some_and(|t| s.now - t >= FLIP_MS) {
        s.flip = None;
    }
    if s.toast.as_ref().is_some_and(|(_, until)| s.now >= *until) {
        s.toast = None;
    }
    track_bands(s);
}

fn track_bands(s: &mut State) {
    let bands: Vec<(String, Band)> = s
        .queue
        .iter()
        .map(|d| (d.id.clone(), band(s.age_ms(d))))
        .collect();
    for (id, to) in bands {
        if let Some(from) = s.bands.insert(id.clone(), to)
            && from != to
        {
            s.anims.push(Anim::AgeBand { id, from, to });
        }
    }
}

fn loaded(s: &mut State, list: Vec<Decision>) {
    s.error = None;
    let first = !s.loaded;
    s.loaded = true;
    let before: Vec<String> = s.queue.iter().map(|d| d.id.clone()).collect();
    let cur_id = s.current().map(|d| d.id.clone());
    let held: Vec<String> = s
        .pending
        .iter()
        .map(|p| p.decision.id.clone())
        .chain(s.sending.keys().cloned())
        .collect();
    let (waiting, mut list): (Vec<Decision>, Vec<Decision>) = list
        .into_iter()
        .filter(|d| !held.contains(&d.id))
        .partition(|d| d.waiting);
    s.sent = waiting.len();
    s.skipped.retain(|id| list.iter().any(|d| &d.id == id));
    list.sort_by_key(|d| {
        if s.failed.contains_key(&d.id) || s.ask_failed.contains_key(&d.id) {
            (0, 0)
        } else if let Some(i) = s.skipped.iter().position(|id| id == &d.id) {
            (2, i)
        } else {
            (1, 0)
        }
    });
    let ids: Vec<String> = list.iter().map(|d| d.id.clone()).collect();
    if first {
        s.anims.push(Anim::Deal);
    } else {
        for id in ids.iter().filter(|id| !before.contains(id)) {
            s.anims.push(Anim::Arrive(id.clone()));
        }
        if let Some(gone) = cur_id.as_ref().filter(|c| !ids.contains(c)) {
            s.anims.push(Anim::Leave(gone.clone()));
            if s.side == Side::Back && !waiting.iter().any(|d| &d.id == gone) {
                s.toast(format!("{gone} was closed elsewhere"));
            }
            s.reset_view();
        }
    }
    s.queue = list;
    let keep = cur_id.and_then(|c| s.queue.iter().position(|d| d.id == c));
    s.cur = keep.unwrap_or(s.cur.min(s.queue.len().saturating_sub(1)));
    track_bands(s);
}

fn key(s: &mut State, k: Key, out: &mut Vec<Command>) {
    s.ghost = None;
    s.flip = None;
    if k == Key::Interrupt {
        return quit(s, out);
    }
    if s.prompt.open {
        return prompt_key(s, k, out);
    }
    if k == Key::Tab && !s.help && s.stats.is_none() {
        return message_session(s, out);
    }
    if s.input.is_some() {
        return input_key(s, k, out);
    }
    if s.help {
        s.help = false;
        return;
    }
    if s.stats.is_some() {
        if matches!(k, Key::Esc | Key::Char('S' | 'q')) {
            s.stats = None;
        }
        return;
    }
    match k {
        Key::Char('q') => quit(s, out),
        Key::Esc if s.side == Side::Back => flip(s),
        Key::Esc => quit(s, out),
        Key::Char(c @ '1'..='9') => choose(s, c as u8 - b'0', out),
        Key::Char('u') => undo(s),
        Key::Char('n') => note(s),
        Key::Char('a') => ask(s, out),
        Key::Char('o') => open(s, out),
        Key::Char('r') => out.push(Command::Reload),
        Key::Char('?') => s.help = true,
        Key::Char('S') => out.push(Command::LoadStats),
        Key::Space => flip(s),
        Key::Char('J') => go(s, s.cur + 1),
        Key::Char('K') if s.cur > 0 => go(s, s.cur - 1),
        _ if s.side == Side::Back => back_key(s, k),
        _ => front_key(s, k, out),
    }
}

/// On the front, j/k (and the arrows) move between the card's options; J/K change cards.
fn front_key(s: &mut State, k: Key, out: &mut Vec<Command>) {
    if s.current().is_some_and(|d| d.is_answer()) {
        // An answer card has no options to move between.
        match k {
            Key::Char('j') | Key::Down => return go(s, s.cur + 1),
            Key::Char('k') | Key::Up => return go(s, s.cur.saturating_sub(1)),
            _ => {}
        }
    }
    match k {
        Key::Char('j' | 'k' | 'g' | 'G') | Key::Down | Key::Up => {
            let Some(d) = s.current() else { return };
            let max = d.options.len();
            s.cursor = Some(match (k, s.cursor) {
                (Key::Char('g'), _) => 0,
                (Key::Char('G'), _) => max,
                (Key::Char('j') | Key::Down, None) => 0,
                (Key::Char('j') | Key::Down, Some(c)) => (c + 1).min(max),
                (_, None) => max,
                (_, Some(c)) => c.saturating_sub(1),
            });
        }
        Key::Char('s') => skip(s),
        Key::Enter => enter(s, out),
        _ => {}
    }
}

fn back_key(s: &mut State, k: Key) {
    s.scroll = match k {
        Key::Char('j') | Key::Down => s.scroll.saturating_add(1).min(s.scroll_max),
        Key::Char('k') | Key::Up => s.scroll.saturating_sub(1),
        Key::CtrlD => s.scroll.saturating_add(HALF_PAGE).min(s.scroll_max),
        Key::CtrlU => s.scroll.saturating_sub(HALF_PAGE),
        Key::Char('g') => 0,
        Key::Char('G') => s.scroll_max,
        _ => s.scroll,
    };
}

fn go(s: &mut State, idx: usize) {
    if idx >= s.queue.len() || idx == s.cur {
        return;
    }
    s.anims
        .push(if idx > s.cur { Anim::Next } else { Anim::Prev });
    s.cur = idx;
    s.reset_view();
}

fn flip(s: &mut State) {
    if s.current().is_none() {
        return;
    }
    s.side = match s.side {
        Side::Front => Side::Back,
        Side::Back => Side::Front,
    };
    s.flip = Some(s.now);
    s.scroll = 0;
}

fn skip(s: &mut State) {
    if s.queue.len() < 2 {
        return;
    }
    let d = s.queue.remove(s.cur);
    s.notes.remove(&d.id);
    s.skipped.retain(|id| id != &d.id);
    s.skipped.push(d.id.clone());
    s.queue.push(d);
    if s.cur == s.queue.len() - 1 {
        s.cur = 0;
    }
    s.reset_view();
    s.anims.push(Anim::Next);
}

fn open(s: &mut State, out: &mut Vec<Command>) {
    let Some(d) = s.current() else { return };
    if d.mr.is_empty() {
        let msg = format!("no MR on {}", d.id);
        s.toast(msg);
    } else {
        out.push(Command::OpenUrl(d.mr.clone()));
    }
}

/// A keyboard quit sends what is pending and waits for every `decide` call to land, so a
/// failure can still be shown; a second quit leaves at once.
fn quit(s: &mut State, out: &mut Vec<Command>) {
    if s.quitting {
        out.push(Command::Quit);
        return;
    }
    send_pending(s, out);
    s.quitting = true;
    s.quit_failed = false;
    if s.sending.is_empty() && s.asking.is_empty() {
        out.push(Command::Quit);
    }
}

fn to_front(s: &mut State, id: &str) {
    if let Some(i) = s.queue.iter().position(|d| d.id == id) {
        let d = s.queue.remove(i);
        s.queue.insert(0, d);
        s.cur = 0;
        s.reset_view();
    }
}

/// On an answer card the actions are ⏎ (seen) and tab (a message); other answering keys say so.
fn answer_card_hint(s: &mut State) -> bool {
    let Some(id) = s.current().filter(|d| d.is_answer()).map(|d| d.id.clone()) else {
        return false;
    };
    s.toast(format!(
        "{id} is an answer to your question: ⏎ got it · tab message its session"
    ));
    true
}

fn choose(s: &mut State, n: u8, out: &mut Vec<Command>) {
    if answer_card_hint(s) {
        return;
    }
    let Some(d) = s.current() else { return };
    if usize::from(n) == d.options.len() + 1 {
        open_tell(s);
    } else if d.options.iter().any(|o| o.n == n) {
        throw(s, n, None, None, out);
    }
}

fn open_tell(s: &mut State) {
    if let Some(id) = s.current().map(|d| d.id.clone()) {
        s.input = Some(Input {
            kind: InputKind::Tell { id },
            text: String::new(),
        });
    }
}

fn enter(s: &mut State, out: &mut Vec<Command>) {
    let Some(d) = s.current() else { return };
    if let Some(f) = s.failed.get(&d.id) {
        let (n, note, custom) = (f.n, f.note.clone(), f.custom.clone());
        return throw(s, n, note, custom, out);
    }
    if d.is_answer() {
        return throw(s, 0, None, None, out);
    }
    if s.cursor == Some(d.options.len()) {
        return open_tell(s);
    }
    let n = s
        .cursor
        .and_then(|c| d.options.get(c))
        .map(|o| o.n)
        .or(d.recommended);
    if let Some(n) = n {
        throw(s, n, None, None, out);
    }
}

/// Throws the current card: option `n`, or the user's own words in `custom`. A note
/// attached to the card before goes with it.
fn throw(
    s: &mut State,
    n: u8,
    note: Option<String>,
    custom: Option<String>,
    out: &mut Vec<Command>,
) {
    if s.current().is_none() {
        return;
    }
    send_pending(s, out);
    let d = s.queue.remove(s.cur);
    s.failed.remove(&d.id);
    let note = note.or_else(|| s.notes.remove(&d.id));
    let (note, custom) = match (custom, note) {
        (Some(text), Some(note)) => (None, Some(format!("{text} (note: {note})"))),
        (custom, note) => (note, custom),
    };
    s.ghost = Some((d.clone(), s.now + GHOST_MS));
    s.pending = Some(Pending {
        decision: d,
        n,
        note,
        custom,
        due: s.now + UNDO_MS,
        paused: false,
    });
    s.anims.push(Anim::Throw);
    s.cur = s.cur.min(s.queue.len().saturating_sub(1));
    s.reset_view();
}

fn send_pending(s: &mut State, out: &mut Vec<Command>) {
    if let Some(p) = s.pending.take() {
        let id = p.decision.id.clone();
        out.push(match &p.custom {
            _ if p.decision.is_answer() => Command::Seen { id: id.clone() },
            Some(text) => Command::Tell {
                id: id.clone(),
                text: text.clone(),
            },
            None => Command::Answer {
                id: id.clone(),
                n: p.n,
                note: p.note.clone(),
            },
        });
        s.sending.insert(id, p);
    }
}

fn undo(s: &mut State) {
    if let Some(p) = s.pending.take() {
        s.queue.insert(0, p.decision);
        s.cur = 0;
        s.reset_view();
        s.anims.push(Anim::Prev);
    }
}

/// n: a note on the answer in its undo window, else a note on the current card.
fn note(s: &mut State) {
    if s.pending.as_ref().is_some_and(|p| p.decision.is_answer()) {
        return;
    }
    if s.pending.is_none() && answer_card_hint(s) {
        return;
    }
    if let Some(p) = s.pending.as_mut() {
        p.paused = true;
        s.input = Some(Input {
            kind: InputKind::Note,
            text: String::new(),
        });
    } else if let Some(id) = s.current().map(|d| d.id.clone()) {
        let text = s.notes.get(&id).cloned().unwrap_or_default();
        s.input = Some(Input {
            kind: InputKind::CardNote { id },
            text,
        });
    }
}

/// a: ask the session back. An answer card waits on nothing, so it takes a message instead.
fn ask(s: &mut State, out: &mut Vec<Command>) {
    if s.current().is_some_and(|d| d.is_answer()) {
        return message_session(s, out);
    }
    if let Some(id) = s.current().map(|d| d.id.clone()) {
        let text = s
            .ask_failed
            .get(&id)
            .map(|f| f.text.clone())
            .unwrap_or_default();
        s.input = Some(Input {
            kind: InputKind::Ask { id },
            text,
        });
    }
}

/// The prompt line, aimed at the session that wrote the answer card in front. A draft keeps its
/// session, as does a message on its way, whose failure brings it back as the draft.
fn message_session(s: &mut State, out: &mut Vec<Command>) {
    let idle = s.prompt.text.is_empty() && s.prompt.images.is_empty() && s.prompt.sending.is_none();
    if idle
        && let Some(d) = s
            .current()
            .filter(|d| d.is_answer() && !d.session.is_empty())
    {
        s.prompt.target = Some(d.session.clone());
    }
    open_prompt(s, out);
}

/// Tab, or `decisions --prompt`: the prompt line opens with its draft and its last session.
pub fn open_prompt(s: &mut State, out: &mut Vec<Command>) {
    s.prompt.open = true;
    s.prompt.pick = Pick::None;
    s.prompt.loaded_at = s.now;
    out.push(Command::LoadSessions);
}

fn sessions_loaded(
    s: &mut State,
    list: Vec<Session>,
    last: Option<String>,
    out: &mut Vec<Command>,
) {
    let p = &mut s.prompt;
    p.sessions = list;
    // Only an unset target takes a default. One that left the list stays chosen, and nothing can
    // be sent until it returns or another is picked: a draft never moves to another session.
    if p.target.is_none() {
        let has = |id: &String| p.sessions.iter().any(|x| &x.owner == id);
        p.target = last
            .filter(has)
            .or_else(|| p.sessions.first().map(|x| x.owner.clone()));
    }
    load_files(s, out);
}

/// The target session's files, once per session; `@` searches them.
fn load_files(s: &mut State, out: &mut Vec<Command>) {
    let Some(cwd) = s.prompt.target_session().map(|x| x.cwd.clone()) else {
        return;
    };
    if s.prompt.files_cwd != cwd {
        s.prompt.files.clear();
        s.prompt.files_cwd = cwd.clone();
        out.push(Command::LoadFiles(cwd));
    }
}

/// The folder an `@` path has reached, once each time it changes.
fn load_dir(s: &mut State, out: &mut Vec<Command>) {
    let Pick::Files { at, .. } = s.prompt.pick else {
        return;
    };
    let Some((dir, _)) = prompt::outside_path(&s.prompt.text[at + 1..]) else {
        return;
    };
    if s.prompt.dir != dir {
        let dir = dir.to_string();
        s.prompt.dir = dir.clone();
        s.prompt.dir_entries.clear();
        let cwd = s
            .prompt
            .target_session()
            .map_or(String::new(), |x| x.cwd.clone());
        out.push(Command::LoadDir { cwd, dir });
    }
}

fn choose_session(s: &mut State, owner: String, out: &mut Vec<Command>) {
    s.prompt.pick = Pick::None;
    s.prompt.target = Some(owner.clone());
    out.push(Command::SaveTarget(owner));
    load_files(s, out);
}

fn prompt_key(s: &mut State, k: Key, out: &mut Vec<Command>) {
    match s.prompt.pick.clone() {
        Pick::None => prompt_text_key(s, k, out),
        Pick::Sessions { query, sel } => session_pick_key(s, k, query, sel, out),
        Pick::Files { at, sel } => {
            file_pick_key(s, k, at, sel);
            load_dir(s, out);
        }
    }
}

fn open_session_pick(s: &mut State) {
    let sel = s
        .prompt
        .sessions
        .iter()
        .position(|x| Some(&x.owner) == s.prompt.target.as_ref())
        .unwrap_or(0);
    s.prompt.pick = Pick::Sessions {
        query: String::new(),
        sel,
    };
    if s.prompt.sessions.is_empty() {
        s.toast("no running sessions".into());
    }
}

fn prompt_text_key(s: &mut State, k: Key, out: &mut Vec<Command>) {
    let p = &mut s.prompt;
    match k {
        Key::Esc => p.open = false,
        Key::Tab => open_session_pick(s),
        Key::CtrlV => out.push(Command::PasteImage),
        Key::Enter => send_prompt(s, out),
        Key::Backspace => {
            if p.text.pop().is_none() {
                p.images.pop();
            }
        }
        Key::Space => p.text.push(' '),
        // `#` picks the session only as the line's first character; later it is plain text.
        Key::Char('#') if p.text.is_empty() => open_session_pick(s),
        Key::Char('@') if p.text.is_empty() || p.text.ends_with(char::is_whitespace) => {
            p.text.push('@');
            // A new `@` reads its folder afresh.
            p.dir.clear();
            p.dir_entries.clear();
            p.pick = Pick::Files {
                at: p.text.len() - 1,
                sel: 0,
            };
        }
        Key::Char(c) => p.text.push(c),
        _ => {}
    }
}

fn session_pick_key(
    s: &mut State,
    k: Key,
    mut query: String,
    mut sel: usize,
    out: &mut Vec<Command>,
) {
    let rows: Vec<String> = s
        .prompt
        .session_rows()
        .iter()
        .map(|x| x.owner.clone())
        .collect();
    match k {
        Key::Esc => s.prompt.pick = Pick::None,
        Key::Enter | Key::Tab => match rows.get(sel) {
            Some(owner) => choose_session(s, owner.clone(), out),
            None => s.prompt.pick = Pick::None,
        },
        Key::Down => sel = (sel + 1).min(rows.len().saturating_sub(1)),
        Key::Up => sel = sel.saturating_sub(1),
        // A number picks a row until a filter is typed; a session may have digits in its name.
        Key::Char(c @ '1'..='9') if query.is_empty() => {
            if let Some(owner) = rows.get(c as usize - '1' as usize) {
                choose_session(s, owner.clone(), out);
            }
        }
        Key::Backspace if query.is_empty() => s.prompt.pick = Pick::None,
        Key::Backspace => {
            query.pop();
            sel = 0;
        }
        Key::Space => {
            query.push(' ');
            sel = 0;
        }
        Key::Char(c) => {
            query.push(c);
            sel = 0;
        }
        _ => {}
    }
    if matches!(s.prompt.pick, Pick::Sessions { .. }) {
        s.prompt.pick = Pick::Sessions { query, sel };
    }
}

fn file_pick_key(s: &mut State, k: Key, at: usize, mut sel: usize) {
    let rows = s.prompt.file_rows();
    let p = &mut s.prompt;
    match k {
        Key::Esc => {
            p.pick = Pick::None;
            return;
        }
        Key::Enter | Key::Tab => {
            let Some(path) = rows.get(sel) else {
                p.pick = Pick::None;
                return;
            };
            let range = p.text[at + 1..]
                .find('#')
                .map(|i| p.text[at + 1 + i..].to_string())
                .unwrap_or_default();
            p.text.truncate(at + 1);
            p.text.push_str(path);
            // A directory stays open for the next part of its path.
            if !path.ends_with('/') {
                p.text.push_str(&range);
                p.text.push(' ');
                p.pick = Pick::None;
            } else {
                p.pick = Pick::Files { at, sel: 0 };
            }
            return;
        }
        Key::Down => sel = (sel + 1).min(rows.len().saturating_sub(1)),
        Key::Up => sel = sel.saturating_sub(1),
        Key::Backspace => {
            p.text.pop();
            sel = 0;
            if p.text.len() <= at {
                p.pick = Pick::None;
                return;
            }
        }
        Key::Space => {
            p.text.push(' ');
            p.pick = Pick::None;
            return;
        }
        Key::Char(c) => {
            p.text.push(c);
            sel = 0;
        }
        _ => {}
    }
    p.pick = Pick::Files { at, sel };
}

fn send_prompt(s: &mut State, out: &mut Vec<Command>) {
    let text = s.prompt.text.trim().to_string();
    if text.is_empty() && s.prompt.images.is_empty() {
        return;
    }
    if s.prompt.sending.is_some() {
        return s.toast("still sending the last message".into());
    }
    let Some((owner, label)) = s
        .prompt
        .target_session()
        .map(|x| (x.owner.clone(), x.label().to_string()))
    else {
        s.toast("pick a running session first".into());
        return open_session_pick(s);
    };
    let images = std::mem::take(&mut s.prompt.images);
    out.push(Command::SendPrompt {
        session: owner,
        source: format!("prompt · {label}"),
        text: prompt::compose(&text, &images),
    });
    s.prompt.sending = Some((std::mem::take(&mut s.prompt.text), images));
    s.prompt.open = false;
    s.toast(format!("sending to {label}…"));
}

fn prompt_sent(s: &mut State, result: Result<String, String>) {
    let draft = s.prompt.sending.take();
    match result {
        Ok(id) => {
            let label = s
                .prompt
                .target_session()
                .map_or("the session", |x| x.label())
                .to_string();
            s.toast(format!("✓ sent to {label} · its reply comes as {id}"));
        }
        Err(e) => {
            if let Some((text, mut images)) = draft {
                if s.quitting {
                    s.prompt.lost.push((text.clone(), e.clone()));
                }
                // Anything typed since goes after the message that came back.
                let p = &mut s.prompt;
                p.text = format!("{text} {}", p.text).trim_end().to_string();
                images.append(&mut p.images);
                p.images = images;
                p.pick = Pick::None;
            }
            s.prompt.open = true;
            s.toast(format!("not sent: {e}"));
        }
    }
}

/// A paste on the prompt line attaches images or inserts text in one line. Anywhere else it
/// types itself, a newline being Enter, as a terminal paste did before pastes came in one piece.
fn paste(s: &mut State, text: String, out: &mut Vec<Command>) {
    if s.prompt.open {
        if let Some(mut images) = prompt::pasted_images(&text) {
            s.prompt.images.append(&mut images);
            return;
        }
        if s.prompt.pick == Pick::None {
            s.prompt.text.push_str(&crate::store::one_line(&text));
            return;
        }
    }
    for c in text.chars() {
        let k = match c {
            '\n' | '\r' => Key::Enter,
            ' ' => Key::Space,
            '\t' => continue,
            c => Key::Char(c),
        };
        key(s, k, out);
    }
}

fn input_key(s: &mut State, k: Key, out: &mut Vec<Command>) {
    let Some(input) = s.input.as_mut() else {
        return;
    };
    match k {
        Key::Char(c) => input.text.push(c),
        Key::Space => input.text.push(' '),
        Key::Backspace => {
            input.text.pop();
        }
        Key::Esc => {
            let was_note = input.kind == InputKind::Note;
            s.input = None;
            if was_note && let Some(p) = s.pending.as_mut() {
                p.paused = false;
                p.due = s.now + UNDO_MS;
            }
        }
        Key::Enter => {
            let Some(Input { kind, text }) = s.input.take() else {
                return;
            };
            let text = text.trim().to_string();
            match kind {
                InputKind::Note => {
                    if let Some(p) = s.pending.as_mut()
                        && !text.is_empty()
                    {
                        p.note = Some(text);
                    }
                    send_pending(s, out);
                }
                InputKind::CardNote { id } => {
                    if text.is_empty() {
                        s.notes.remove(&id);
                    } else {
                        s.notes.insert(id, text);
                    }
                }
                InputKind::Tell { id } => {
                    if text.is_empty() {
                        return;
                    }
                    match s.queue.iter().position(|d| d.id == id) {
                        Some(i) => {
                            s.cur = i;
                            throw(s, 0, None, Some(text), out);
                        }
                        None => s.toast(format!("{id} was closed elsewhere")),
                    }
                }
                InputKind::Ask { id } => {
                    if !text.is_empty() {
                        out.push(Command::Ask {
                            id: id.clone(),
                            text: text.clone(),
                        });
                        s.asking.insert(id.clone(), text);
                        s.toast(format!("asking {id}…"));
                    }
                }
            }
        }
        _ => {}
    }
}

fn sent(
    s: &mut State,
    id: String,
    ask: bool,
    result: Result<Option<String>, String>,
    out: &mut Vec<Command>,
) {
    if ask {
        let text = s.asking.remove(&id).unwrap_or_default();
        match result {
            Ok(warning) => {
                s.ask_failed.remove(&id);
                s.toast(warning.unwrap_or(format!("asked {id}")));
            }
            Err(error) => {
                if let Some(input) = s.input.as_mut()
                    && input.kind == (InputKind::Ask { id: id.clone() })
                    && input.text.is_empty()
                {
                    input.text = text.clone();
                }
                s.ask_failed.insert(id.clone(), AskFailed { text, error });
                to_front(s, &id);
                s.quit_failed |= s.quitting;
            }
        }
    } else if let Some(p) = s.sending.remove(&id) {
        match result {
            Ok(warning) => {
                s.failed.remove(&id);
                if let Some(w) = warning {
                    s.toast(w);
                }
            }
            Err(error) => {
                s.failed.insert(
                    id.clone(),
                    Failed {
                        n: p.n,
                        note: p.note.clone(),
                        custom: p.custom.clone(),
                        error,
                    },
                );
                s.queue.retain(|d| d.id != id);
                s.queue.insert(0, p.decision);
                s.cur = 0;
                s.reset_view();
                s.quit_failed |= s.quitting;
            }
        }
    }
    if s.quitting && s.sending.is_empty() && s.asking.is_empty() {
        if std::mem::take(&mut s.quit_failed) {
            s.quitting = false;
            s.toast("not sent: the card is back in front · q to quit anyway".into());
        } else {
            out.push(Command::Quit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::decode;

    pub const T0: i64 = 1_792_483_448_000;
    pub const MIN: i64 = 60_000;

    pub fn fixture() -> Vec<Decision> {
        decode(include_str!("../tests/fixtures/list.json")).unwrap()
    }

    /// D-1 from the fixture, then D-2..D-n copied from the fixture's D-2.
    pub fn many(n: usize) -> Vec<Decision> {
        (1..=n)
            .map(|i| {
                let mut d = fixture()[usize::from(i > 1)].clone();
                d.id = format!("D-{i}");
                d
            })
            .collect()
    }

    pub fn loaded(list: Vec<Decision>) -> State {
        let mut s = State::new(T0);
        update(&mut s, Event::Loaded(Ok(list)), T0);
        s.anims.clear();
        s
    }

    pub fn press(s: &mut State, k: Key) -> Vec<Command> {
        let now = s.now;
        update(s, Event::Key(k), now)
    }

    fn ids(s: &State) -> Vec<&str> {
        s.queue.iter().map(|d| d.id.as_str()).collect()
    }

    #[test]
    fn first_load_deals_the_deck() {
        let mut s = State::new(T0);
        assert!(update(&mut s, Event::Loaded(Ok(fixture())), T0).is_empty());
        assert_eq!(ids(&s), ["D-1", "D-2"]);
        assert_eq!(s.current().unwrap().id, "D-1");
        assert_eq!(s.anims, [Anim::Deal]);
    }

    #[test]
    fn shift_j_and_k_switch_cards_and_stop_at_the_ends() {
        let mut s = loaded(many(3));
        press(&mut s, Key::Char('K'));
        assert_eq!(s.cur, 0);
        assert!(s.anims.is_empty());
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Char('J'));
        assert_eq!(s.cur, 2);
        assert_eq!(s.anims, [Anim::Next, Anim::Next]);
        press(&mut s, Key::Char('K'));
        assert_eq!(s.cur, 1);
        assert_eq!(s.anims.last(), Some(&Anim::Prev));
    }

    #[test]
    fn j_and_k_move_between_the_options_on_the_front() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('j'));
        assert_eq!(s.cursor, Some(0));
        for _ in 0..5 {
            press(&mut s, Key::Char('j'));
        }
        assert_eq!(s.cursor, Some(3), "three options, then the tell slot");
        press(&mut s, Key::Char('k'));
        assert_eq!(s.cursor, Some(2));
        assert_eq!(s.cur, 0, "j/k stay on the card");
        press(&mut s, Key::Up);
        assert_eq!(s.cursor, Some(1), "the arrows do the same");
        press(&mut s, Key::Char('J'));
        assert_eq!(s.cursor, None, "a new card starts without a cursor");
    }

    #[test]
    fn g_and_shift_g_jump_to_the_first_and_last_option() {
        let mut s = loaded(many(4));
        press(&mut s, Key::Char('G'));
        assert_eq!(s.cursor, Some(3), "the tell slot is last");
        press(&mut s, Key::Char('g'));
        assert_eq!(s.cursor, Some(0));
        assert_eq!(s.cur, 0);
    }

    #[test]
    fn shift_j_from_the_back_goes_to_the_next_card_front() {
        let mut s = loaded(many(2));
        press(&mut s, Key::Space);
        press(&mut s, Key::Char('J'));
        assert_eq!(s.cur, 1);
        assert_eq!(s.side, Side::Front);
    }

    #[test]
    fn space_flips_and_the_back_scrolls_within_its_max() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Space);
        assert_eq!(s.side, Side::Back);
        assert_eq!(s.flip, Some(T0));
        s.scroll_max = 12;
        press(&mut s, Key::Char('j'));
        assert_eq!(s.scroll, 1);
        press(&mut s, Key::CtrlD);
        assert_eq!(s.scroll, 11);
        press(&mut s, Key::CtrlD);
        assert_eq!(s.scroll, 12);
        press(&mut s, Key::Char('g'));
        assert_eq!(s.scroll, 0);
        press(&mut s, Key::Char('G'));
        assert_eq!(s.scroll, 12);
        press(&mut s, Key::CtrlU);
        assert_eq!(s.scroll, 2);
        assert_eq!(
            s.cur, 0,
            "j/g/G scroll on the back, they do not change cards"
        );
    }

    #[test]
    fn esc_on_the_back_flips_to_the_front_instead_of_quitting() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Space);
        assert!(press(&mut s, Key::Esc).is_empty());
        assert_eq!(s.side, Side::Front);
    }

    #[test]
    fn the_flip_ends_after_flip_ms() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Space);
        update(&mut s, Event::Tick, T0 + FLIP_MS - 1);
        assert!(s.animating());
        update(&mut s, Event::Tick, T0 + FLIP_MS);
        assert!(!s.animating());
    }

    #[test]
    fn skip_moves_the_card_to_the_end_and_a_reload_keeps_it_there() {
        let mut s = loaded(many(3));
        press(&mut s, Key::Char('s'));
        assert_eq!(ids(&s), ["D-2", "D-3", "D-1"]);
        assert_eq!(s.current().unwrap().id, "D-2");
        update(&mut s, Event::Loaded(Ok(many(3))), T0);
        assert_eq!(ids(&s), ["D-2", "D-3", "D-1"]);
        assert_eq!(s.current().unwrap().id, "D-2");
    }

    #[test]
    fn skipping_the_last_card_goes_back_to_the_first() {
        let mut s = loaded(many(2));
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Char('s'));
        assert_eq!(s.cur, 0);
    }

    #[test]
    fn a_reload_keeps_the_same_card_in_front() {
        let mut s = loaded(many(3));
        press(&mut s, Key::Char('J'));
        let mut list = many(3);
        list.insert(0, {
            let mut d = list[2].clone();
            d.id = "D-9".into();
            d
        });
        update(&mut s, Event::Loaded(Ok(list)), T0);
        assert_eq!(s.current().unwrap().id, "D-2");
        assert!(s.anims.contains(&Anim::Arrive("D-9".into())));
    }

    #[test]
    fn a_card_closed_elsewhere_leaves_and_the_next_one_takes_its_place() {
        let mut s = loaded(many(3));
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Space);
        s.anims.clear();
        let mut list = many(3);
        list.remove(1);
        update(&mut s, Event::Loaded(Ok(list)), T0);
        assert_eq!(s.current().unwrap().id, "D-3");
        assert_eq!(s.side, Side::Front);
        assert_eq!(s.anims, [Anim::Leave("D-2".into())]);
        assert_eq!(s.toast.as_ref().unwrap().0, "D-2 was closed elsewhere");
    }

    #[test]
    fn a_failed_read_keeps_the_queue_and_the_next_good_one_clears_the_error() {
        let mut s = loaded(fixture());
        update(&mut s, Event::Loaded(Err("decide not runnable".into())), T0);
        assert_eq!(s.queue.len(), 2);
        assert_eq!(s.error.as_deref(), Some("decide not runnable"));
        update(&mut s, Event::Loaded(Ok(fixture())), T0);
        assert_eq!(s.error, None);
    }

    #[test]
    fn changed_files_and_r_ask_for_a_reload() {
        let mut s = loaded(fixture());
        assert_eq!(update(&mut s, Event::Changed, T0), [Command::Reload]);
        assert_eq!(press(&mut s, Key::Char('r')), [Command::Reload]);
    }

    #[test]
    fn o_opens_the_mr_or_says_there_is_none() {
        let mut s = loaded(fixture());
        assert_eq!(
            press(&mut s, Key::Char('o')),
            [Command::OpenUrl(
                "https://gitlab.com/acme/acme-backend/-/merge_requests/870".into()
            )]
        );
        press(&mut s, Key::Char('J'));
        assert!(press(&mut s, Key::Char('o')).is_empty());
        assert_eq!(s.toast.as_ref().unwrap().0, "no MR on D-2");
        update(&mut s, Event::Tick, T0 + TOAST_MS);
        assert_eq!(s.toast, None);
    }

    #[test]
    fn s_opens_the_stats_screen_and_esc_s_or_q_close_it() {
        let mut s = State::new(T0);
        assert_eq!(press(&mut s, Key::Char('S')), [Command::LoadStats]);
        assert!(s.stats.is_none());
        let st = crate::stats::compute(&[], chrono::Local::now().fixed_offset(), 14);
        update(&mut s, Event::StatsLoaded(Ok(Box::new(st))), T0);
        assert!(s.stats.is_some());
        for k in [Key::Esc, Key::Char('S'), Key::Char('q')] {
            s.stats = Some(crate::stats::compute(
                &[],
                chrono::Local::now().fixed_offset(),
                14,
            ));
            assert!(press(&mut s, Key::Char('j')).is_empty() && s.stats.is_some());
            assert!(press(&mut s, k).is_empty(), "{k:?} does not quit");
            assert!(s.stats.is_none());
        }
        update(&mut s, Event::StatsLoaded(Err("boom".into())), T0);
        assert!(s.stats.is_none() && s.toast.is_some());
    }

    #[test]
    fn help_opens_with_a_question_mark_and_any_key_closes_it_without_acting() {
        let mut s = loaded(many(2));
        press(&mut s, Key::Char('?'));
        assert!(s.help);
        press(&mut s, Key::Char('J'));
        assert!(!s.help);
        assert_eq!(s.cur, 0);
    }

    #[test]
    fn bands_and_labels() {
        assert_eq!(band(14 * MIN), Band::Fresh);
        assert_eq!(band(15 * MIN), Band::Warm);
        assert_eq!(band(60 * MIN), Band::Old);
        assert_eq!(band(180 * MIN), Band::Stale);
        assert_eq!(age_label(59 * MIN + 59_999), "59m");
        assert_eq!(age_label(65 * MIN), "1h 05m");
        assert_eq!(age_label(-5 * MIN), "0m");
    }

    #[test]
    fn future_or_bad_asked_is_age_zero() {
        let mut list = fixture();
        list[0].asked = "2026-10-21T11:04:08+03:00".into();
        list[1].asked = "soon".into();
        let s = loaded(list);
        assert_eq!(s.age_ms(&s.queue[0]), 0);
        assert_eq!(s.age_ms(&s.queue[1]), 0);
        assert_eq!(band(s.age_ms(&s.queue[0])), Band::Fresh);
    }

    #[test]
    fn crossing_an_age_band_animates_once() {
        let mut s = loaded(fixture());
        update(&mut s, Event::Tick, T0 + 14 * MIN);
        assert!(s.anims.is_empty());
        update(&mut s, Event::Tick, T0 + 15 * MIN);
        assert!(s.anims.contains(&Anim::AgeBand {
            id: "D-1".into(),
            from: Band::Fresh,
            to: Band::Warm
        }));
        s.anims.clear();
        update(&mut s, Event::Tick, T0 + 16 * MIN);
        assert!(s.anims.is_empty());
    }

    fn answer(id: &str, n: u8, note: Option<&str>) -> Command {
        Command::Answer {
            id: id.into(),
            n,
            note: note.map(String::from),
        }
    }

    #[test]
    fn a_number_throws_the_card_and_holds_the_answer_for_the_undo_window() {
        let mut s = loaded(fixture());
        assert!(press(&mut s, Key::Char('2')).is_empty());
        assert_eq!(ids(&s), ["D-2"]);
        assert_eq!(s.pending.as_ref().unwrap().n, 2);
        assert_eq!(s.ghost.as_ref().unwrap().0.id, "D-1");
        assert!(s.anims.contains(&Anim::Throw));
        assert!(update(&mut s, Event::Tick, T0 + UNDO_MS - 1).is_empty());
        assert_eq!(
            update(&mut s, Event::Tick, T0 + UNDO_MS),
            [answer("D-1", 2, None)]
        );
        assert!(s.pending.is_none());
        assert!(s.sending.contains_key("D-1"));
    }

    pub fn answer_card(id: &str) -> Decision {
        let mut d = fixture()[1].clone();
        d.id = id.into();
        d.kind = "answer".into();
        d.options.clear();
        d.recommended = None;
        d.question = "Why is CI red?".into();
        d.context = "A flaky e2e test.".into();
        d
    }

    #[test]
    fn enter_marks_an_answer_card_seen_after_the_undo_window() {
        let mut s = loaded(vec![answer_card("D-5"), fixture()[0].clone()]);
        assert!(press(&mut s, Key::Enter).is_empty());
        assert_eq!(ids(&s), ["D-1"]);
        assert!(update(&mut s, Event::Tick, T0 + UNDO_MS - 1).is_empty());
        assert_eq!(
            update(&mut s, Event::Tick, T0 + UNDO_MS),
            [Command::Seen { id: "D-5".into() }]
        );
        assert!(s.sending.contains_key("D-5"));
    }

    #[test]
    fn undo_brings_an_answer_card_back() {
        let mut s = loaded(vec![answer_card("D-5")]);
        press(&mut s, Key::Enter);
        press(&mut s, Key::Char('u'));
        assert_eq!(ids(&s), ["D-5"]);
        assert!(update(&mut s, Event::Tick, T0 + UNDO_MS).is_empty());
    }

    #[test]
    fn an_answer_card_takes_the_deck_keys() {
        let mut s = loaded(vec![
            answer_card("D-5"),
            answer_card("D-6"),
            fixture()[0].clone(),
        ]);
        assert!(s.input.is_none());
        press(&mut s, Key::Down);
        assert_eq!(s.current().unwrap().id, "D-6");
        press(&mut s, Key::Char('k'));
        assert_eq!(s.current().unwrap().id, "D-5");
        press(&mut s, Key::Up);
        assert_eq!(s.current().unwrap().id, "D-5", "already the first");
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Char('j'));
        assert_eq!(s.current().unwrap().id, "D-1");
        press(&mut s, Key::Char('K'));
        press(&mut s, Key::Enter);
        assert_eq!(ids(&s), ["D-5", "D-1"], "⏎ on the card is got it");
        press(&mut s, Key::Char('u'));
        assert_eq!(ids(&s), ["D-6", "D-5", "D-1"], "u brings it back");
        press(&mut s, Key::Char('q'));
        assert!(s.quitting);
    }

    #[test]
    fn numbers_and_notes_only_hint_on_an_answer_card() {
        let mut s = loaded(vec![answer_card("D-5")]);
        for k in [Key::Char('1'), Key::Char('2'), Key::Char('n')] {
            assert!(press(&mut s, k).is_empty(), "{k:?}");
            assert!(s.input.is_none(), "{k:?} opened an input");
            assert!(s.pending.is_none(), "{k:?} threw the card");
        }
        assert_eq!(
            s.toast.as_ref().unwrap().0,
            "D-5 is an answer to your question: ⏎ got it · tab message its session"
        );
    }

    #[test]
    fn a_failed_seen_comes_back_and_enter_retries_it() {
        let mut s = loaded(vec![answer_card("D-5")]);
        press(&mut s, Key::Enter);
        update(&mut s, Event::Tick, T0 + UNDO_MS);
        update(
            &mut s,
            Event::Sent {
                id: "D-5".into(),
                ask: false,
                result: Err("disk full".into()),
            },
            T0 + UNDO_MS,
        );
        assert_eq!(ids(&s), ["D-5"]);
        press(&mut s, Key::Enter);
        assert_eq!(
            update(&mut s, Event::Tick, T0 + 2 * UNDO_MS),
            [Command::Seen { id: "D-5".into() }]
        );
    }

    #[test]
    fn the_ghost_goes_after_ghost_ms() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('1'));
        update(&mut s, Event::Tick, T0 + GHOST_MS);
        assert!(s.ghost.is_none());
    }

    #[test]
    fn a_number_above_the_options_does_nothing() {
        let mut s = loaded(fixture());
        assert!(press(&mut s, Key::Char('4')).is_empty());
        assert!(s.pending.is_none());
        assert_eq!(s.queue.len(), 2);
    }

    #[test]
    fn enter_throws_the_star_or_the_option_under_the_cursor() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Enter);
        assert_eq!(s.pending.as_ref().unwrap().n, 1);
        let mut s = loaded(fixture());
        press(&mut s, Key::Down);
        press(&mut s, Key::Down);
        press(&mut s, Key::Enter);
        assert_eq!(s.pending.as_ref().unwrap().n, 2);
    }

    #[test]
    fn enter_without_star_or_cursor_does_nothing() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('J'));
        assert!(press(&mut s, Key::Enter).is_empty());
        assert!(s.pending.is_none());
    }

    #[test]
    fn no_options_means_nothing_to_throw() {
        let mut list = fixture();
        list[0].options.clear();
        list[0].recommended = None;
        let mut s = loaded(list);
        press(&mut s, Key::Down);
        press(&mut s, Key::Enter);
        press(&mut s, Key::Char('1'));
        assert!(s.pending.is_none());
        assert_eq!(s.queue.len(), 2);
    }

    #[test]
    fn undo_puts_the_card_back_in_front_and_sends_nothing() {
        let mut s = loaded(many(3));
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Char('1'));
        press(&mut s, Key::Char('u'));
        assert_eq!(ids(&s), ["D-2", "D-1", "D-3"]);
        assert_eq!(s.cur, 0);
        assert!(s.pending.is_none() && s.ghost.is_none());
        assert!(update(&mut s, Event::Tick, T0 + UNDO_MS).is_empty());
    }

    #[test]
    fn a_note_pauses_the_countdown_and_enter_sends_with_it() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('2'));
        press(&mut s, Key::Char('n'));
        assert!(s.pending.as_ref().unwrap().paused);
        for c in "use".chars() {
            press(&mut s, Key::Char(c));
        }
        press(&mut s, Key::Space);
        for c in "stagingx".chars() {
            press(&mut s, Key::Char(c));
        }
        press(&mut s, Key::Backspace);
        assert!(
            update(&mut s, Event::Tick, T0 + 2 * UNDO_MS).is_empty(),
            "paused"
        );
        assert_eq!(
            press(&mut s, Key::Enter),
            [answer("D-1", 2, Some("use staging"))]
        );
        assert!(s.input.is_none());
    }

    #[test]
    fn esc_in_the_note_drops_it_and_restarts_the_countdown() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('2'));
        press(&mut s, Key::Char('n'));
        press(&mut s, Key::Char('x'));
        update(&mut s, Event::Key(Key::Esc), T0 + 1_000);
        assert!(s.input.is_none());
        assert!(update(&mut s, Event::Tick, T0 + 1_000 + UNDO_MS - 1).is_empty());
        assert_eq!(
            update(&mut s, Event::Tick, T0 + 1_000 + UNDO_MS),
            [answer("D-1", 2, None)]
        );
    }

    #[test]
    fn q_typed_in_an_input_is_text_not_quit() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('a'));
        assert!(press(&mut s, Key::Char('q')).is_empty());
        assert_eq!(s.input.as_ref().unwrap().text, "q");
    }

    #[test]
    fn a_second_throw_sends_the_first_at_once() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('1'));
        assert_eq!(press(&mut s, Key::Char('2')), [answer("D-1", 1, None)]);
        assert_eq!(s.pending.as_ref().unwrap().decision.id, "D-2");
    }

    #[test]
    fn ask_back_sends_the_question_and_empty_text_cancels() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('a'));
        for c in "why".chars() {
            press(&mut s, Key::Char(c));
        }
        assert_eq!(
            press(&mut s, Key::Enter),
            [Command::Ask {
                id: "D-1".into(),
                text: "why".into()
            }]
        );
        assert_eq!(s.toast.as_ref().unwrap().0, "asking D-1…");
        assert_eq!(s.queue.len(), 2, "asking does not answer");
        press(&mut s, Key::Char('a'));
        assert!(press(&mut s, Key::Enter).is_empty());
        assert!(s.input.is_none());
    }

    fn sent(id: &str, ask: bool, result: Result<(), &str>) -> Event {
        Event::Sent {
            id: id.into(),
            ask,
            result: result.map(|_| None).map_err(String::from),
        }
    }

    fn type_text(s: &mut State, text: &str) {
        for c in text.chars() {
            press(s, if c == ' ' { Key::Space } else { Key::Char(c) });
        }
    }

    #[test]
    fn a_signal_sends_the_pending_answer_and_quits_at_once() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('3'));
        assert_eq!(
            update(&mut s, Event::Signal, T0),
            [answer("D-1", 3, None), Command::Quit]
        );
    }

    #[test]
    fn q_esc_and_interrupt_send_the_pending_answer_and_quit_once_it_lands() {
        for k in [Key::Char('q'), Key::Esc, Key::Interrupt] {
            let mut s = loaded(fixture());
            press(&mut s, Key::Char('3'));
            assert_eq!(press(&mut s, k), [answer("D-1", 3, None)], "{k:?}");
            assert!(s.quitting);
            assert_eq!(
                update(&mut s, sent("D-1", false, Ok(())), T0),
                [Command::Quit],
                "{k:?}"
            );
        }
    }

    #[test]
    fn quitting_with_nothing_in_flight_is_immediate() {
        let mut s = loaded(fixture());
        assert_eq!(press(&mut s, Key::Char('q')), [Command::Quit]);
    }

    #[test]
    fn a_send_that_fails_while_quitting_cancels_the_quit() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('2'));
        press(&mut s, Key::Char('q'));
        assert!(update(&mut s, sent("D-1", false, Err("disk full")), T0).is_empty());
        assert!(!s.quitting);
        assert_eq!(s.current().unwrap().id, "D-1");
        assert!(s.toast.as_ref().unwrap().0.contains("not sent"));
        assert_eq!(press(&mut s, Key::Char('q')), [Command::Quit]);
    }

    #[test]
    fn q_again_while_sending_quits_anyway() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('2'));
        press(&mut s, Key::Char('q'));
        assert_eq!(press(&mut s, Key::Char('q')), [Command::Quit]);
    }

    #[test]
    fn interrupt_quits_even_from_an_input() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('2'));
        press(&mut s, Key::Char('n'));
        assert_eq!(press(&mut s, Key::Interrupt), [answer("D-1", 2, None)]);
        assert!(s.quitting);
    }

    #[test]
    fn an_ask_says_asked_only_once_it_lands() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('a'));
        type_text(&mut s, "why");
        press(&mut s, Key::Enter);
        assert_eq!(s.toast.as_ref().unwrap().0, "asking D-1…");
        update(&mut s, sent("D-1", true, Ok(())), T0);
        assert_eq!(s.toast.as_ref().unwrap().0, "asked D-1");
        assert!(s.asking.is_empty());
    }

    #[test]
    fn a_failed_ask_keeps_its_text_and_marks_the_card_until_retried() {
        let mut s = loaded(many(3));
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Char('a'));
        type_text(&mut s, "why not");
        press(&mut s, Key::Enter);
        update(&mut s, sent("D-2", true, Err("no such decision")), T0);
        assert_eq!(s.current().unwrap().id, "D-2");
        assert_eq!(s.ask_failed["D-2"].error, "no such decision");
        update(&mut s, Event::Loaded(Ok(many(3))), T0);
        assert!(
            s.ask_failed.contains_key("D-2"),
            "a reload does not clear it"
        );
        assert_eq!(s.current().unwrap().id, "D-2", "it stays in front");
        press(&mut s, Key::Char('a'));
        assert_eq!(s.input.as_ref().unwrap().text, "why not");
        press(&mut s, Key::Enter);
        update(&mut s, sent("D-2", true, Ok(())), T0);
        assert!(s.ask_failed.is_empty());
    }

    #[test]
    fn an_ask_result_does_not_settle_an_answer_on_the_same_card() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('a'));
        type_text(&mut s, "why");
        press(&mut s, Key::Enter);
        press(&mut s, Key::Char('1'));
        update(&mut s, Event::Tick, T0 + UNDO_MS);
        update(&mut s, sent("D-1", true, Ok(())), T0 + UNDO_MS);
        assert!(
            s.sending.contains_key("D-1"),
            "the answer is still in flight"
        );
        update(&mut s, sent("D-1", false, Err("disk full")), T0 + UNDO_MS);
        assert_eq!(s.failed["D-1"].error, "disk full");
    }

    #[test]
    fn quitting_waits_for_an_ask_too() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('a'));
        type_text(&mut s, "why");
        press(&mut s, Key::Enter);
        assert!(press(&mut s, Key::Char('q')).is_empty());
        assert_eq!(
            update(&mut s, sent("D-1", true, Ok(())), T0),
            [Command::Quit]
        );
    }

    #[test]
    fn a_reload_while_an_answer_is_pending_or_sending_keeps_that_card_out() {
        let mut s = loaded(many(3));
        press(&mut s, Key::Char('1'));
        update(&mut s, Event::Loaded(Ok(many(3))), T0);
        assert_eq!(ids(&s), ["D-2", "D-3"]);
        update(&mut s, Event::Tick, T0 + UNDO_MS);
        update(&mut s, Event::Loaded(Ok(many(3))), T0 + UNDO_MS);
        assert_eq!(ids(&s), ["D-2", "D-3"]);
    }

    #[test]
    fn a_failed_send_brings_the_card_back_and_enter_retries_it() {
        let mut s = loaded(many(3));
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Char('2'));
        update(&mut s, Event::Tick, T0 + UNDO_MS);
        update(
            &mut s,
            Event::Sent {
                id: "D-2".into(),
                ask: false,
                result: Err("disk full".into()),
            },
            T0 + UNDO_MS,
        );
        assert_eq!(s.current().unwrap().id, "D-2");
        assert_eq!(s.failed["D-2"].error, "disk full");
        update(&mut s, Event::Loaded(Ok(many(3))), T0 + UNDO_MS);
        assert_eq!(
            s.current().unwrap().id,
            "D-2",
            "a failed card stays in front"
        );
        press(&mut s, Key::Enter);
        assert_eq!(s.pending.as_ref().unwrap().n, 2);
        assert!(!s.failed.contains_key("D-2"));
    }

    #[test]
    fn a_successful_send_is_forgotten() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('1'));
        update(&mut s, Event::Tick, T0 + UNDO_MS);
        update(
            &mut s,
            Event::Sent {
                id: "D-1".into(),
                ask: false,
                result: Ok(None),
            },
            T0 + UNDO_MS,
        );
        assert!(s.sending.is_empty() && s.failed.is_empty());
    }

    #[test]
    fn any_key_ends_a_running_flip_and_ghost() {
        let mut s = loaded(many(3));
        press(&mut s, Key::Char('1'));
        press(&mut s, Key::Space);
        assert!(s.ghost.is_none(), "the flip key ended the ghost");
        press(&mut s, Key::Char('J'));
        assert!(s.flip.is_none());
    }

    #[test]
    fn throwing_from_the_back_returns_to_the_front() {
        let mut s = loaded(many(2));
        press(&mut s, Key::Space);
        press(&mut s, Key::Char('1'));
        assert_eq!(s.side, Side::Front);
        assert_eq!(s.scroll, 0);
    }

    fn tell(id: &str, text: &str) -> Command {
        Command::Tell {
            id: id.into(),
            text: text.into(),
        }
    }

    #[test]
    fn the_number_after_the_options_opens_the_tell_line() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('4'));
        assert_eq!(
            s.input.as_ref().unwrap().kind,
            InputKind::Tell { id: "D-1".into() }
        );
        type_text(&mut s, "close it");
        assert!(
            press(&mut s, Key::Enter).is_empty(),
            "held for the undo window"
        );
        assert_eq!(ids(&s), ["D-2"]);
        assert_eq!(
            update(&mut s, Event::Tick, T0 + UNDO_MS),
            [tell("D-1", "close it")]
        );
    }

    #[test]
    fn the_cursor_reaches_the_tell_slot_and_enter_opens_it() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('G'));
        assert_eq!(s.cursor, Some(3), "three options, then the slot");
        press(&mut s, Key::Enter);
        assert!(matches!(
            s.input.as_ref().unwrap().kind,
            InputKind::Tell { .. }
        ));
    }

    #[test]
    fn an_empty_tell_or_esc_throws_nothing() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('4'));
        press(&mut s, Key::Enter);
        assert!(s.pending.is_none() && s.input.is_none());
        press(&mut s, Key::Char('4'));
        type_text(&mut s, "x");
        press(&mut s, Key::Esc);
        assert!(s.pending.is_none());
        assert_eq!(s.queue.len(), 2);
    }

    #[test]
    fn a_tell_can_be_undone_and_quit_sends_it() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('4'));
        type_text(&mut s, "close it");
        press(&mut s, Key::Enter);
        press(&mut s, Key::Char('u'));
        assert_eq!(s.current().unwrap().id, "D-1");
        press(&mut s, Key::Char('4'));
        type_text(&mut s, "close it");
        press(&mut s, Key::Enter);
        assert_eq!(press(&mut s, Key::Char('q')), [tell("D-1", "close it")]);
    }

    #[test]
    fn a_failed_tell_comes_back_and_enter_retries_it() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('4'));
        type_text(&mut s, "close it");
        press(&mut s, Key::Enter);
        update(&mut s, Event::Tick, T0 + UNDO_MS);
        update(&mut s, sent("D-1", false, Err("disk full")), T0 + UNDO_MS);
        assert_eq!(s.current().unwrap().id, "D-1");
        press(&mut s, Key::Enter);
        assert_eq!(
            update(&mut s, Event::Tick, T0 + 2 * UNDO_MS),
            [tell("D-1", "close it")]
        );
    }

    #[test]
    fn n_before_answering_attaches_a_note_the_answer_carries() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('n'));
        assert_eq!(
            s.input.as_ref().unwrap().kind,
            InputKind::CardNote { id: "D-1".into() }
        );
        type_text(&mut s, "use staging");
        press(&mut s, Key::Enter);
        assert_eq!(s.notes["D-1"], "use staging");
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Char('K'));
        assert_eq!(
            s.notes["D-1"], "use staging",
            "kept when moving away and back"
        );
        press(&mut s, Key::Char('2'));
        assert_eq!(
            update(&mut s, Event::Tick, T0 + UNDO_MS),
            [answer("D-1", 2, Some("use staging"))]
        );
        assert!(s.notes.is_empty());
    }

    #[test]
    fn n_again_edits_the_note_and_an_empty_one_removes_it() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('n'));
        type_text(&mut s, "ab");
        press(&mut s, Key::Enter);
        press(&mut s, Key::Char('n'));
        assert_eq!(s.input.as_ref().unwrap().text, "ab");
        press(&mut s, Key::Backspace);
        press(&mut s, Key::Backspace);
        press(&mut s, Key::Enter);
        assert!(s.notes.is_empty());
    }

    #[test]
    fn skipping_a_card_drops_its_note() {
        let mut s = loaded(many(2));
        press(&mut s, Key::Char('n'));
        type_text(&mut s, "x");
        press(&mut s, Key::Enter);
        press(&mut s, Key::Char('s'));
        assert!(s.notes.is_empty());
    }

    #[test]
    fn n_during_the_undo_window_still_notes_the_pending_answer() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('2'));
        press(&mut s, Key::Char('n'));
        assert_eq!(s.input.as_ref().unwrap().kind, InputKind::Note);
    }
    #[test]
    fn a_hook_warning_is_toasted() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('1'));
        update(&mut s, Event::Tick, T0 + UNDO_MS);
        update(
            &mut s,
            Event::Sent {
                id: "D-1".into(),
                ask: false,
                result: Ok(Some("on_answer failed (exit 9): broken".into())),
            },
            T0 + UNDO_MS,
        );
        assert_eq!(
            s.toast.as_ref().unwrap().0,
            "on_answer failed (exit 9): broken"
        );
        assert!(s.failed.is_empty(), "the answer stands");
    }

    fn session(owner: &str, name: &str) -> Session {
        Session {
            owner: owner.into(),
            name: name.into(),
            client: "claude-code".into(),
            cwd: format!("/x/{owner}"),
            ..Session::default()
        }
    }

    /// A deck with the prompt line open on two running sessions, the first the target.
    fn prompting() -> State {
        let mut s = loaded(fixture());
        press(&mut s, Key::Tab);
        update(
            &mut s,
            Event::SessionsLoaded {
                sessions: vec![session("o1", "the system"), session("o2", "ACME-1")],
                last: None,
            },
            T0,
        );
        s
    }

    fn type_in(s: &mut State, text: &str) {
        for c in text.chars() {
            press(s, if c == ' ' { Key::Space } else { Key::Char(c) });
        }
    }

    #[test]
    fn tab_opens_the_prompt_line_and_asks_for_the_sessions() {
        let mut s = loaded(fixture());
        assert_eq!(press(&mut s, Key::Tab), [Command::LoadSessions]);
        assert!(s.prompt.open);
    }

    #[test]
    fn the_prompt_line_takes_the_deck_keys_as_text() {
        let mut s = prompting();
        assert!(press(&mut s, Key::Char('1')).is_empty());
        assert!(press(&mut s, Key::Char('q')).is_empty());
        assert_eq!(s.prompt.text, "1q");
        assert!(s.pending.is_none() && !s.quitting);
    }

    #[test]
    fn esc_closes_the_line_and_keeps_the_draft_and_its_images() {
        let mut s = prompting();
        type_in(&mut s, "half a thought");
        let image = Image {
            path: "/a.png".into(),
            bytes: 1,
        };
        update(&mut s, Event::ImagePasted(Ok(image)), T0);
        press(&mut s, Key::Esc);
        assert!(!s.prompt.open);
        press(&mut s, Key::Tab);
        assert_eq!(s.prompt.text, "half a thought");
        assert_eq!(s.prompt.images.len(), 1);
    }

    fn sessions_arrive(s: &mut State) {
        update(
            s,
            Event::SessionsLoaded {
                sessions: vec![session("o1", "a"), session("o2", "b")],
                last: Some("o1".into()),
            },
            T0,
        );
    }

    fn written_by(id: &str, owner: &str) -> Decision {
        let mut d = answer_card(id);
        d.session = owner.into();
        d
    }

    #[test]
    fn tab_and_a_on_an_answer_card_aim_at_the_session_that_wrote_it() {
        for k in [Key::Tab, Key::Char('a')] {
            let mut s = loaded(vec![written_by("D-5", "o2")]);
            assert_eq!(press(&mut s, k), [Command::LoadSessions]);
            assert!(s.prompt.open && s.input.is_none());
            sessions_arrive(&mut s);
            assert_eq!(s.prompt.target.as_deref(), Some("o2"), "not the last used");
        }
    }

    #[test]
    fn a_question_card_keeps_the_last_session_and_a_asks_back() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Tab);
        sessions_arrive(&mut s);
        assert_eq!(s.prompt.target.as_deref(), Some("o1"));
        press(&mut s, Key::Esc);
        press(&mut s, Key::Char('a'));
        assert!(!s.prompt.open);
        assert_eq!(
            s.input.as_ref().unwrap().kind,
            InputKind::Ask { id: "D-1".into() }
        );
    }

    #[test]
    fn an_answer_card_with_no_session_leaves_the_target_alone() {
        let mut s = loaded(vec![answer_card("D-5")]);
        press(&mut s, Key::Tab);
        sessions_arrive(&mut s);
        assert_eq!(s.prompt.target.as_deref(), Some("o1"));
    }

    #[test]
    fn a_draft_is_not_moved_to_the_card_in_front() {
        let mut s = loaded(vec![fixture()[0].clone(), written_by("D-5", "o2")]);
        press(&mut s, Key::Tab);
        sessions_arrive(&mut s);
        type_in(&mut s, "half a thought");
        press(&mut s, Key::Esc);
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Tab);
        assert_eq!(s.prompt.target.as_deref(), Some("o1"));

        s.prompt.text.clear();
        press(&mut s, Key::Esc);
        press(&mut s, Key::Tab);
        assert_eq!(s.prompt.target.as_deref(), Some("o2"), "nothing to move");
    }

    #[test]
    fn a_message_on_its_way_keeps_its_session() {
        let mut s = loaded(vec![fixture()[0].clone(), written_by("D-5", "o2")]);
        press(&mut s, Key::Tab);
        sessions_arrive(&mut s);
        type_in(&mut s, "first");
        press(&mut s, Key::Enter);
        press(&mut s, Key::Char('J'));
        press(&mut s, Key::Tab);
        assert_eq!(s.prompt.target.as_deref(), Some("o1"));
    }

    #[test]
    fn the_last_session_is_the_target_while_it_runs_else_the_first() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Tab);
        let list = vec![session("o1", "a"), session("o2", "b")];
        let load = |s: &mut State, last: &str| {
            s.prompt.target = None;
            update(
                s,
                Event::SessionsLoaded {
                    sessions: list.clone(),
                    last: Some(last.into()),
                },
                T0,
            );
        };
        load(&mut s, "o2");
        assert_eq!(s.prompt.target.as_deref(), Some("o2"));
        load(&mut s, "gone");
        assert_eq!(s.prompt.target.as_deref(), Some("o1"));
    }

    #[test]
    fn a_target_that_leaves_the_list_stays_chosen_and_nothing_goes_elsewhere() {
        let mut s = prompting();
        type_in(&mut s, "for the first one");
        let reload = |s: &mut State, list: Vec<Session>| {
            update(
                s,
                Event::SessionsLoaded {
                    sessions: list,
                    last: Some("o2".into()),
                },
                T0,
            );
        };
        reload(&mut s, vec![session("o2", "ACME-1")]);
        assert_eq!(s.prompt.target.as_deref(), Some("o1"));
        assert!(s.prompt.target_session().is_none());
        assert!(press(&mut s, Key::Enter).is_empty(), "nothing is sent");
        assert_eq!(s.prompt.text, "for the first one");
        press(&mut s, Key::Esc);
        reload(&mut s, vec![session("o1", "the system")]);
        assert_eq!(s.prompt.target_session().unwrap().owner, "o1");
    }

    #[test]
    fn a_card_waiting_on_its_session_stays_off_the_stack_until_the_reply() {
        let mut d = answer_card("D-5");
        d.prompt = true;
        d.discussion = vec![crate::queue::Entry {
            at: "10:00".into(),
            who: "you".into(),
            text: "status?".into(),
        }];
        d.waiting = true;
        let mut s = loaded(vec![d.clone()]);
        assert!(s.queue.is_empty());
        assert_eq!(s.sent, 1);

        d.waiting = false;
        d.discussion.push(crate::queue::Entry {
            at: "10:01".into(),
            who: "session".into(),
            text: "fine".into(),
        });
        update(&mut s, Event::Loaded(Ok(vec![d])), T0);
        assert_eq!(s.sent, 0);
        assert_eq!(s.current().unwrap().id, "D-5", "the reply brings it back");
        press(&mut s, Key::Enter);
        assert!(s.queue.is_empty(), "one ⏎ is got it");
    }

    #[test]
    fn a_card_back_with_no_reply_is_filed_by_one_enter() {
        let mut d = answer_card("D-5");
        d.prompt = true;
        d.discussion = vec![crate::queue::Entry {
            at: "10:00".into(),
            who: "you".into(),
            text: "status?".into(),
        }];
        let mut s = loaded(vec![d]);
        press(&mut s, Key::Enter);
        assert!(s.queue.is_empty(), "nothing asks twice");
    }

    #[test]
    fn a_card_going_off_the_stack_to_wait_was_not_closed_elsewhere() {
        let mut d = answer_card("D-5");
        let mut s = loaded(vec![d.clone(), answer_card("D-6")]);
        s.side = Side::Back;
        d.waiting = true;
        update(&mut s, Event::Loaded(Ok(vec![d, answer_card("D-6")])), T0);
        assert_eq!(s.current().unwrap().id, "D-6");
        assert!(s.toast.is_none(), "{:?}", s.toast);
    }

    #[test]
    fn a_failed_send_keeps_what_was_typed_since_and_closes_any_list() {
        let mut s = prompting();
        type_in(&mut s, "first");
        press(&mut s, Key::Enter);
        press(&mut s, Key::Tab);
        type_in(&mut s, "much longer second text @");
        assert!(matches!(s.prompt.pick, Pick::Files { .. }));
        update(&mut s, Event::PromptSent(Err("disk full".into())), T0);
        assert_eq!(s.prompt.text, "first much longer second text @");
        assert_eq!(s.prompt.pick, Pick::None);
        assert!(s.prompt.file_rows().is_empty());
    }

    #[test]
    fn only_one_message_is_on_its_way_at_a_time() {
        let mut s = prompting();
        type_in(&mut s, "first");
        press(&mut s, Key::Enter);
        press(&mut s, Key::Tab);
        type_in(&mut s, "second");
        assert!(press(&mut s, Key::Enter).is_empty());
        assert_eq!(s.prompt.text, "second");
    }

    #[test]
    fn a_message_that_fails_as_the_deck_closes_is_kept_for_the_report() {
        let mut s = prompting();
        type_in(&mut s, "first");
        press(&mut s, Key::Enter);
        press(&mut s, Key::Char('q'));
        assert!(s.quitting);
        update(&mut s, Event::PromptSent(Err("disk full".into())), T0);
        assert_eq!(
            s.prompt.lost,
            [("first".to_string(), "disk full".to_string())]
        );
    }

    #[test]
    fn a_typed_line_range_survives_picking_the_file() {
        let mut s = prompting();
        s.prompt.files = vec!["src/app.rs".into()];
        type_in(&mut s, "@app#L10-20");
        press(&mut s, Key::Enter);
        assert_eq!(s.prompt.text, "@src/app.rs#L10-20 ");
    }

    #[test]
    fn hash_as_the_first_character_picks_the_session() {
        let mut s = prompting();
        press(&mut s, Key::Char('#'));
        assert!(matches!(s.prompt.pick, Pick::Sessions { .. }));
        type_in(&mut s, "acme");
        assert_eq!(s.prompt.session_rows().len(), 1);
        assert_eq!(
            press(&mut s, Key::Enter)[0],
            Command::SaveTarget("o2".into())
        );
        assert_eq!(s.prompt.pick, Pick::None);
        assert_eq!(s.prompt.text, "", "the filter does not go in the message");
    }

    #[test]
    fn a_number_picks_a_session_until_a_filter_is_typed() {
        let mut s = prompting();
        press(&mut s, Key::Tab);
        press(&mut s, Key::Char('2'));
        assert_eq!(s.prompt.target.as_deref(), Some("o2"));
    }

    #[test]
    fn hash_later_in_the_line_is_text() {
        let mut s = prompting();
        type_in(&mut s, "see #123");
        assert_eq!(s.prompt.text, "see #123");
        assert_eq!(s.prompt.pick, Pick::None);
    }

    #[test]
    fn at_starts_a_file_search_at_the_start_of_a_word_only() {
        let mut s = prompting();
        type_in(&mut s, "me@work ");
        assert_eq!(s.prompt.pick, Pick::None);
        press(&mut s, Key::Char('@'));
        assert!(matches!(s.prompt.pick, Pick::Files { .. }));
    }

    #[test]
    fn picking_a_file_puts_its_path_in_the_text() {
        let mut s = prompting();
        s.prompt.files = vec!["src/app.rs".into(), "src/".into(), "README.md".into()];
        type_in(&mut s, "look at @app");
        assert_eq!(s.prompt.file_rows(), ["src/app.rs"]);
        press(&mut s, Key::Tab);
        assert_eq!(s.prompt.text, "look at @src/app.rs ");
        assert_eq!(s.prompt.pick, Pick::None);
    }

    #[test]
    fn picking_a_directory_keeps_the_search_open_inside_it() {
        let mut s = prompting();
        s.prompt.files = vec!["src/".into()];
        type_in(&mut s, "@sr");
        press(&mut s, Key::Enter);
        assert_eq!(s.prompt.text, "@src/");
        assert!(matches!(s.prompt.pick, Pick::Files { .. }));
    }

    fn dir_loads(cmds: Vec<Command>) -> Vec<(String, String)> {
        cmds.into_iter()
            .filter_map(|c| match c {
                Command::LoadDir { cwd, dir } => Some((cwd, dir)),
                _ => None,
            })
            .collect()
    }

    fn press_all(s: &mut State, text: &str) -> Vec<Command> {
        text.chars().flat_map(|c| press(s, Key::Char(c))).collect()
    }

    fn dir_arrives(s: &mut State, dir: &str, names: &[&str]) {
        let entries = names.iter().map(|n| format!("{dir}{n}")).collect();
        update(
            s,
            Event::DirLoaded {
                dir: dir.into(),
                entries,
            },
            T0,
        );
    }

    #[test]
    fn an_at_path_lists_its_folder_once_and_filters_by_the_name() {
        let mut s = prompting();
        let loads = dir_loads(press_all(&mut s, "@~/wik"));
        assert_eq!(
            loads,
            [(s.prompt.target_session().unwrap().cwd.clone(), "~/".into())]
        );
        assert!(s.prompt.file_rows().is_empty(), "not read yet");
        dir_arrives(&mut s, "~/", &["bin/", "wiki/", "work.md"]);
        assert_eq!(s.prompt.file_rows(), ["~/wiki/"]);
    }

    #[test]
    fn every_way_to_name_a_folder_outside_is_listed() {
        for (typed, dir) in [
            ("@/Users/me/pro", "/Users/me/"),
            ("@~/wik", "~/"),
            ("@../dot", "../"),
            ("@./sr", "./"),
        ] {
            let mut s = prompting();
            let loads = dir_loads(press_all(&mut s, typed));
            let last = loads.last().map(|(_, d)| d.as_str());
            assert_eq!(last, Some(dir), "{typed}");
            assert_eq!(s.prompt.dir, dir);
        }
    }

    #[test]
    fn picking_a_folder_goes_into_it_and_a_file_ends_the_list() {
        let mut s = prompting();
        press_all(&mut s, "see @~/");
        dir_arrives(&mut s, "~/", &["notes.md", "wiki/"]);
        assert_eq!(s.prompt.file_rows(), ["~/notes.md", "~/wiki/"]);
        press(&mut s, Key::Down);
        let loads = dir_loads(press(&mut s, Key::Enter));
        assert_eq!(s.prompt.text, "see @~/wiki/");
        assert_eq!(loads.last().unwrap().1, "~/wiki/");
        assert!(matches!(s.prompt.pick, Pick::Files { .. }));
        dir_arrives(&mut s, "~/wiki/", &["index.md"]);
        press(&mut s, Key::Enter);
        assert_eq!(s.prompt.text, "see @~/wiki/index.md ");
        assert_eq!(s.prompt.pick, Pick::None);
    }

    #[test]
    fn a_line_range_survives_picking_from_a_folder_outside() {
        let mut s = prompting();
        press_all(&mut s, "@~/ind");
        dir_arrives(&mut s, "~/", &["index.md"]);
        press_all(&mut s, "#L1-2");
        assert_eq!(s.prompt.file_rows(), ["~/index.md"]);
        press(&mut s, Key::Enter);
        assert_eq!(s.prompt.text, "@~/index.md#L1-2 ");
    }

    #[test]
    fn backspace_out_of_a_folder_lists_the_one_above_and_late_news_is_ignored() {
        let mut s = prompting();
        press_all(&mut s, "@~/a/");
        assert_eq!(s.prompt.dir, "~/a/");
        let loads = dir_loads(press(&mut s, Key::Backspace));
        assert_eq!(loads.last().unwrap().1, "~/");
        dir_arrives(&mut s, "~/a/", &["late"]);
        assert!(
            s.prompt.dir_entries.is_empty(),
            "that folder is left behind"
        );
    }

    #[test]
    fn a_new_at_reads_the_folder_again() {
        let mut s = prompting();
        press_all(&mut s, "@~/");
        dir_arrives(&mut s, "~/", &["old.md"]);
        press(&mut s, Key::Esc);
        press(&mut s, Key::Space);
        let loads = dir_loads(press_all(&mut s, "@~/"));
        assert_eq!(loads.len(), 1, "the folder may have changed");
        assert!(s.prompt.dir_entries.is_empty());
    }

    #[test]
    fn a_plain_search_still_searches_the_sessions_files() {
        let mut s = prompting();
        s.prompt.files = vec!["src/app.rs".into()];
        let loads = dir_loads(press_all(&mut s, "@src/ap"));
        assert!(loads.is_empty());
        assert_eq!(s.prompt.file_rows(), ["src/app.rs"]);
    }

    #[test]
    fn backspace_over_the_at_ends_the_search_and_esc_keeps_the_text() {
        let mut s = prompting();
        type_in(&mut s, "@a");
        press(&mut s, Key::Backspace);
        press(&mut s, Key::Backspace);
        assert_eq!((s.prompt.text.as_str(), &s.prompt.pick), ("", &Pick::None));
        type_in(&mut s, "@b");
        press(&mut s, Key::Esc);
        assert_eq!(
            (s.prompt.text.as_str(), &s.prompt.pick),
            ("@b", &Pick::None)
        );
        assert!(s.prompt.open);
    }

    #[test]
    fn enter_sends_the_text_and_images_to_the_target_and_closes_the_line() {
        let mut s = prompting();
        type_in(&mut s, "look");
        let image = Image {
            path: "/a.png".into(),
            bytes: 1,
        };
        update(&mut s, Event::ImagePasted(Ok(image)), T0);
        assert_eq!(
            press(&mut s, Key::Enter),
            [Command::SendPrompt {
                session: "o1".into(),
                source: "prompt · the system".into(),
                text: "look [image: /a.png]".into(),
            }]
        );
        assert!(!s.prompt.open && s.prompt.text.is_empty() && s.prompt.images.is_empty());
    }

    #[test]
    fn a_failed_send_puts_the_draft_back() {
        let mut s = prompting();
        type_in(&mut s, "look");
        press(&mut s, Key::Enter);
        update(&mut s, Event::PromptSent(Err("disk full".into())), T0);
        assert!(s.prompt.open);
        assert_eq!(s.prompt.text, "look");
        assert_eq!(s.toast.as_ref().unwrap().0, "not sent: disk full");
    }

    #[test]
    fn enter_with_nothing_typed_sends_nothing() {
        let mut s = prompting();
        assert!(press(&mut s, Key::Enter).is_empty());
        assert!(s.prompt.open);
    }

    #[test]
    fn enter_with_no_session_asks_for_one() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Tab);
        type_in(&mut s, "hello");
        assert!(press(&mut s, Key::Enter).is_empty());
        assert_eq!(s.prompt.text, "hello");
        assert!(matches!(s.prompt.pick, Pick::Sessions { .. }));
    }

    #[test]
    fn a_pasted_path_to_an_image_attaches_it_and_other_text_goes_in_one_line() {
        let d = std::env::temp_dir().join(format!("decisions-app-paste-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let png = d.join("a.png");
        std::fs::write(&png, "x").unwrap();
        let mut s = prompting();
        update(&mut s, Event::Paste(png.display().to_string()), T0);
        assert_eq!(s.prompt.images[0].path, png);
        assert_eq!(s.prompt.text, "");
        update(&mut s, Event::Paste("two\nlines".into()), T0);
        assert_eq!(s.prompt.text, "two lines");
    }

    #[test]
    fn a_paste_with_the_line_closed_types_itself_with_newline_as_enter() {
        let mut s = loaded(fixture());
        press(&mut s, Key::Char('n'));
        update(&mut s, Event::Paste("a b\n".into()), T0);
        assert!(s.input.is_none(), "the newline sent it");
        assert_eq!(s.notes.get("D-1").map(String::as_str), Some("a b"));
    }

    #[test]
    fn the_open_line_reloads_the_sessions_now_and_then() {
        let mut s = prompting();
        assert!(update(&mut s, Event::Tick, T0 + SESSIONS_EVERY_MS - 1).is_empty());
        assert_eq!(
            update(&mut s, Event::Tick, T0 + SESSIONS_EVERY_MS),
            [Command::LoadSessions]
        );
    }
}
