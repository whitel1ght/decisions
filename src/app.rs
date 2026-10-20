//! All state and key handling. `update` is the whole behaviour: an event and the time in,
//! the new state and the commands for `main` to run out. Nothing here touches the
//! terminal, the clock or `decide`.

use std::collections::HashMap;

use crate::queue::Decision;

pub const UNDO_MS: i64 = 5_000;
pub const GHOST_MS: i64 = 200;
pub const FLIP_MS: i64 = 300;
pub const TOAST_MS: i64 = 3_000;
const HALF_PAGE: u16 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Up,
    Down,
    Space,
    Backspace,
    CtrlD,
    CtrlU,
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
        result: Result<(), String>,
    },
    Signal,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Reload,
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

#[derive(Debug, Default)]
pub struct State {
    pub queue: Vec<Decision>,
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
        Event::Sent { id, ask, result } => sent(s, id, ask, result, &mut out),
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
    let mut list: Vec<Decision> = list.into_iter().filter(|d| !held.contains(&d.id)).collect();
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
            if s.side == Side::Back {
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
    if s.input.is_some() {
        return input_key(s, k, out);
    }
    if s.help {
        s.help = false;
        return;
    }
    match k {
        Key::Char('q') => quit(s, out),
        Key::Esc if s.side == Side::Back => flip(s),
        Key::Esc => quit(s, out),
        Key::Char(c @ '1'..='9') => choose(s, c as u8 - b'0', out),
        Key::Char('u') => undo(s),
        Key::Char('n') => note(s),
        Key::Char('a') => ask(s),
        Key::Char('o') => open(s, out),
        Key::Char('r') => out.push(Command::Reload),
        Key::Char('?') => s.help = true,
        Key::Space => flip(s),
        Key::Char('J') => go(s, s.cur + 1),
        Key::Char('K') if s.cur > 0 => go(s, s.cur - 1),
        _ if s.side == Side::Back => back_key(s, k),
        _ => front_key(s, k, out),
    }
}

/// On the front, j/k (and the arrows) move between the card's options; J/K change cards.
fn front_key(s: &mut State, k: Key, out: &mut Vec<Command>) {
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

fn choose(s: &mut State, n: u8, out: &mut Vec<Command>) {
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

fn ask(s: &mut State) {
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

fn sent(s: &mut State, id: String, ask: bool, result: Result<(), String>, out: &mut Vec<Command>) {
    if ask {
        let text = s.asking.remove(&id).unwrap_or_default();
        match result {
            Ok(()) => {
                s.ask_failed.remove(&id);
                s.toast(format!("asked {id}"));
            }
            Err(error) => {
                s.ask_failed.insert(id.clone(), AskFailed { text, error });
                to_front(s, &id);
                s.quit_failed |= s.quitting;
            }
        }
    } else if let Some(p) = s.sending.remove(&id) {
        match result {
            Ok(()) => {
                s.failed.remove(&id);
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
            result: result.map_err(String::from),
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
                result: Ok(()),
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
}
