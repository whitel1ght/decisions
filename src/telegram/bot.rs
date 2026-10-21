//! One turn of the Telegram service: save answers whose undo window ran out, bring sent cards up
//! to date, send new cards while the user is away, and handle what the chat sent.

use std::path::PathBuf;

use crate::app::age_label;
use crate::config::{Config, Context};
use crate::hook;
use crate::idle::{self, Idle};
use crate::queue::Decision;
use crate::store::{Event, Outcome, Store, one_line};
use crate::telegram::api::{Api, ApiError, Update};
use crate::telegram::render::{self, Keyboard, Msg};
use crate::telegram::state::{self, Sent, State};

pub const UNDO_MS: i64 = 10_000;
pub const POLL_SECS: u64 = 5;
pub const HELP: &str = "Cards come here while you're away from the computer.
Tap an option to answer (10 s to undo), or reply to a card:
  text: answer in your own words
  2 text: option 2, with the text as a note
  ? text: ask the agent a question back
/on · /off · /status · /help";

pub struct Bot<A: Api> {
    pub api: A,
    pub store: Store,
    pub dir: PathBuf,
    pub hook: Option<String>,
    pub context: Context,
    pub idle_minutes: u64,
    pub idle: Box<dyn Idle>,
    pub now: Box<dyn Fn() -> i64>,
    pub log: Box<dyn FnMut(&str)>,
    pub poll_secs: u64,
    idle_warned: bool,
}

/// A disk error, retried like a network one.
fn local(e: String) -> ApiError {
    ApiError::Retry {
        after: None,
        msg: e,
    }
}

/// `on · away (idle 12m)`, then the open cards and whether each was sent.
pub fn summary(st: &State, open: &[Decision], idle_secs: Option<u64>, minutes: u64) -> String {
    let idle = idle_secs.map_or("idle time unknown".to_string(), |s| {
        format!("idle {}", age_label(s as i64 * 1000))
    });
    let mut out = format!(
        "{} · {} ({idle})\n{} open",
        if st.enabled { "on" } else { "off" },
        if idle::away(idle_secs, minutes) {
            "away"
        } else {
            "at the computer"
        },
        open.len()
    );
    for d in open {
        let sent = if st.messages.contains_key(&d.id) {
            "sent"
        } else {
            "not sent yet"
        };
        out.push_str(&format!(
            "\n{} · {} · {sent}",
            d.id,
            render::cut(&one_line(&d.question), 80)
        ));
    }
    out
}

impl<A: Api> Bot<A> {
    pub fn new(api: A, cfg: &Config) -> Self {
        Self {
            api,
            store: cfg.store(),
            dir: cfg.dir.clone(),
            hook: cfg.on_answer.clone(),
            context: cfg.telegram.context,
            idle_minutes: cfg.telegram.idle_minutes,
            idle: Box::new(idle::System),
            now: Box::new(|| chrono::Local::now().timestamp_millis()),
            log: Box::new(|m| eprintln!("{m}")),
            poll_secs: POLL_SECS,
            idle_warned: false,
        }
    }

    fn state(&self) -> Result<State, ApiError> {
        state::load(&self.dir).map_err(local)
    }

    fn update(&self, f: impl FnOnce(&mut State)) -> Result<State, ApiError> {
        state::update(&self.dir, f).map_err(local)
    }

    pub fn turn(&mut self) -> Result<(), ApiError> {
        self.save_due()?;
        self.sync()?;
        self.send_new()?;
        self.poll()
    }

    fn away(&mut self) -> bool {
        let secs = self.idle.seconds();
        if secs.is_none() && self.idle_minutes > 0 && !self.idle_warned {
            self.idle_warned = true;
            (self.log)("telegram: cannot tell the idle time; counting you as away");
        }
        idle::away(secs, self.idle_minutes)
    }

    /// Sends, and once more as plain text when Telegram refuses the HTML. None when refused
    /// either way (logged), so the caller moves on instead of retrying forever.
    fn send(
        &mut self,
        chat: i64,
        m: &Msg,
        kb: &[Vec<render::Button>],
        reply_to: Option<i64>,
    ) -> Result<Option<(i64, bool)>, ApiError> {
        match self.api.send(chat, &m.text, m.html, kb, reply_to) {
            Ok(id) => Ok(Some((id, m.html))),
            Err(ApiError::Refused(_)) if m.html => {
                match self
                    .api
                    .send(chat, &render::plain(&m.text), false, kb, reply_to)
                {
                    Ok(id) => Ok(Some((id, false))),
                    Err(ApiError::Refused(e)) => {
                        (self.log)(&format!("telegram: {e}"));
                        Ok(None)
                    }
                    Err(e) => Err(e),
                }
            }
            Err(ApiError::Refused(e)) => {
                (self.log)(&format!("telegram: {e}"));
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    fn say(
        &mut self,
        chat: i64,
        text: &str,
        reply_to: Option<i64>,
    ) -> Result<Option<i64>, ApiError> {
        let m = Msg {
            text: text.into(),
            html: false,
        };
        Ok(self.send(chat, &m, &[], reply_to)?.map(|(id, _)| id))
    }

    /// Edits, and once more as plain text when Telegram refuses the HTML; a refusal either way
    /// is logged and the card moves on.
    fn edit(
        &mut self,
        chat: i64,
        mid: i64,
        m: &Msg,
        kb: &[Vec<render::Button>],
    ) -> Result<(), ApiError> {
        let r = match self.api.edit(chat, mid, &m.text, m.html, kb) {
            Err(ApiError::Refused(_)) if m.html => {
                self.api.edit(chat, mid, &render::plain(&m.text), false, kb)
            }
            r => r,
        };
        match r {
            Err(ApiError::Refused(e)) => {
                (self.log)(&format!("telegram: {e}"));
                Ok(())
            }
            r => r,
        }
    }

    fn message_of(&self, id: &str) -> Result<Option<i64>, ApiError> {
        Ok(self.state()?.messages.get(id).map(|s| s.message_id))
    }

    fn run_hook(&mut self, chat: i64, id: &str, event: &Event) -> Result<(), ApiError> {
        if let Err(w) = hook::run(self.hook.as_deref(), event) {
            let mid = self.message_of(id)?;
            self.say(chat, &format!("⚠ hook failed: {w}"), mid)?;
        }
        Ok(())
    }

    /// After an answer from the chat: the hook, then the card shows how it ended. A refusal
    /// because the card was answered meanwhile shows the stored answer; any other refusal is
    /// said in the chat.
    fn finish(
        &mut self,
        chat: i64,
        id: &str,
        r: crate::store::Result<Event>,
    ) -> Result<(), ApiError> {
        match r {
            Ok(event) => {
                self.run_hook(chat, id, &event)?;
                self.close_card(chat, id, false)
            }
            Err(e) => match self.store.outcome(id) {
                Ok(Outcome::Pending | Outcome::AskedBack(_)) => {
                    let mid = self.message_of(id)?;
                    self.say(chat, &e.0, mid)?;
                    Ok(())
                }
                _ => self.close_card(chat, id, true),
            },
        }
    }

    /// Edit a sent card to show how it ended, drop its buttons and any pending answer, mark it done.
    fn close_card(&mut self, chat: i64, id: &str, elsewhere: bool) -> Result<(), ApiError> {
        let Some(sent) = self.state()?.messages.get(id).cloned() else {
            return Ok(());
        };
        let status = match self.store.outcome(id) {
            Ok(o) => match render::outcome_line(&o, elsewhere) {
                Some(l) => l,
                None => return Ok(()),
            },
            Err(_) => render::CLOSED.to_string(),
        };
        let m = render::with_status(&sent.text, sent.html, &status);
        self.edit(chat, sent.message_id, &m, &[])?;
        self.update(|s| {
            if let Some(x) = s.messages.get_mut(id) {
                x.done = true;
            }
            s.pending.remove(id);
        })?;
        Ok(())
    }

    /// Save tapped answers whose undo window has run out.
    pub fn save_due(&mut self) -> Result<(), ApiError> {
        let st = self.state()?;
        let Some(chat) = st.chat_id else {
            return Ok(());
        };
        let now = (self.now)();
        for (id, p) in st.pending.iter().filter(|(_, p)| p.due_ms <= now) {
            let r = self.store.answer(id, p.option, &p.note);
            self.update(|s| {
                s.pending.remove(id);
            })?;
            self.finish(chat, id, r)?;
        }
        Ok(())
    }

    /// Finish sent cards that ended elsewhere; thread the session's new replies under the rest.
    pub fn sync(&mut self) -> Result<(), ApiError> {
        let st = self.state()?;
        let Some(chat) = st.chat_id else {
            return Ok(());
        };
        let open = self.store.list().0;
        for id in st.open() {
            match open.iter().find(|d| d.id == id) {
                Some(d) => self.forward(chat, &st.messages[&id], d)?,
                None => self.close_card(chat, &id, true)?,
            }
        }
        Ok(())
    }

    fn forward(&mut self, chat: i64, sent: &Sent, d: &Decision) -> Result<(), ApiError> {
        for (i, e) in d.discussion.iter().enumerate().skip(sent.discussion_seen) {
            if e.who != "you"
                && let Some(mid) = self.say(
                    chat,
                    &format!("{}: {}", e.who, e.text),
                    Some(sent.message_id),
                )?
            {
                self.update(|s| {
                    if let Some(x) = s.messages.get_mut(&d.id) {
                        x.replies.push(mid);
                    }
                })?;
            }
            self.update(|s| {
                if let Some(x) = s.messages.get_mut(&d.id) {
                    x.discussion_seen = i + 1;
                }
            })?;
        }
        Ok(())
    }

    /// While on and away: every open card not sent yet, in the store's order.
    pub fn send_new(&mut self) -> Result<(), ApiError> {
        let st = self.state()?;
        let Some(chat) = st.chat_id else {
            return Ok(());
        };
        if !st.enabled || !self.away() {
            return Ok(());
        }
        let now = (self.now)();
        for d in self.store.list().0 {
            if st.messages.contains_key(&d.id) {
                continue;
            }
            let m = render::card(&d, now, self.context);
            let kb: Keyboard = render::buttons(&d);
            let sent = self.send(chat, &m, &kb, None)?;
            let (message_id, html) = sent.unwrap_or((0, false));
            let text = if html || !m.html {
                m.text.clone()
            } else {
                render::plain(&m.text)
            };
            self.update(|s| {
                s.messages.insert(
                    d.id.clone(),
                    Sent {
                        message_id,
                        done: sent.is_none(),
                        discussion_seen: d.discussion.len(),
                        text,
                        html,
                        replies: vec![],
                    },
                );
            })?;
        }
        Ok(())
    }

    /// Long-poll for updates and handle each; the offset moves past an update once handled.
    pub fn poll(&mut self) -> Result<(), ApiError> {
        let offset = self.state()?.offset;
        for u in self.api.get_updates(offset, self.poll_secs)? {
            self.handle(&u)?;
            self.update(|s| s.offset = s.offset.max(u.id + 1))?;
        }
        Ok(())
    }

    /// Replaced in Task 8 by the real dispatcher.
    pub fn handle(&mut self, _u: &Update) -> Result<(), ApiError> {
        Ok(())
    }
}

#[cfg(test)]
#[allow(dead_code)] // helpers Task 8's tests use
mod tests {
    use super::*;
    use crate::config::Mode;
    use crate::store::New;
    use crate::telegram::api::Kind;
    use crate::telegram::fake::{Fake, FixedIdle, Out};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub const CHAT: i64 = 7;
    pub const LOG: &str = "cat >> 'EVENTS'";

    pub struct T {
        pub bot: Bot<Fake>,
        pub cfg: Config,
        pub idle: Rc<Cell<Option<u64>>>,
        pub now: Rc<Cell<i64>>,
        pub logs: Rc<RefCell<Vec<String>>>,
        pub events: PathBuf,
    }

    pub fn bot(
        t_cfg: &Config,
        idle: &Rc<Cell<Option<u64>>>,
        now: &Rc<Cell<i64>>,
        logs: &Rc<RefCell<Vec<String>>>,
    ) -> Bot<Fake> {
        let mut b = Bot::new(Fake::default(), t_cfg);
        b.idle = Box::new(FixedIdle(idle.clone()));
        let n = now.clone();
        b.now = Box::new(move || n.get());
        let l = logs.clone();
        b.log = Box::new(move |m| l.borrow_mut().push(m.to_string()));
        b
    }

    pub fn setup(hook: &str) -> T {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "decisions-bot-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let events = dir.with_extension("events");
        let _ = std::fs::remove_file(&events);
        let cfg = Config {
            dir: dir.clone(),
            mode: Mode::Always,
            wait: 50,
            on_answer: Some(hook.replace("EVENTS", &events.display().to_string())),
            mr_strip: String::new(),
            session: None,
            telegram: Default::default(),
        };
        state::update(&dir, |s| {
            s.chat_id = Some(CHAT);
            s.enabled = true;
        })
        .unwrap();
        let idle = Rc::new(Cell::new(Some(3600)));
        let now = Rc::new(Cell::new(chrono::Local::now().timestamp_millis()));
        let logs = Rc::new(RefCell::new(Vec::new()));
        T {
            bot: bot(&cfg, &idle, &now, &logs),
            cfg,
            idle,
            now,
            logs,
            events,
        }
    }

    pub fn add(t: &T, q: &str) -> String {
        t.bot
            .store
            .add(New {
                key: "ACME-1".into(),
                question: q.into(),
                options: vec!["Yes".into(), "No".into()],
                recommend: Some(1),
                ..New::default()
            })
            .unwrap()
    }

    pub fn st(t: &T) -> State {
        state::load(&t.bot.dir).unwrap()
    }

    pub fn msg(id: i64, text: &str, reply_to: Option<i64>) -> Update {
        Update {
            id,
            kind: Kind::Message {
                chat_id: CHAT,
                private: true,
                message_id: 1000 + id,
                text: text.into(),
                reply_to,
            },
        }
    }

    pub fn tap(id: i64, message_id: i64, data: &str) -> Update {
        Update {
            id,
            kind: Kind::Press {
                query_id: format!("q{id}"),
                chat_id: CHAT,
                message_id,
                data: data.into(),
            },
        }
    }

    pub fn events(t: &T) -> String {
        std::fs::read_to_string(&t.events).unwrap_or_default()
    }

    pub fn edit_of(t: &T, mid: i64) -> Out {
        t.bot
            .api
            .edits
            .iter()
            .rev()
            .find(|e| e.message_id == mid)
            .cloned()
            .expect("an edit")
    }

    #[test]
    fn sends_open_cards_only_when_on_and_away() {
        let mut t = setup(LOG);
        add(&t, "First?");
        add(&t, "Second?");
        t.idle.set(Some(10));
        t.bot.turn().unwrap();
        assert!(t.bot.api.sent.is_empty(), "at the computer");
        state::update(&t.bot.dir, |s| s.enabled = false).unwrap();
        t.idle.set(Some(3600));
        t.bot.turn().unwrap();
        assert!(t.bot.api.sent.is_empty(), "off");
        state::update(&t.bot.dir, |s| s.enabled = true).unwrap();
        t.bot.turn().unwrap();
        let firsts: Vec<bool> = t
            .bot
            .api
            .sent
            .iter()
            .map(|o| o.text.contains("First?"))
            .collect();
        assert_eq!(firsts, vec![true, false], "store order");
        let open = t.bot.store.list().0;
        assert_eq!(t.bot.api.sent[0].keyboard, render::buttons(&open[0]));
        assert_eq!(t.bot.api.sent[0].chat, CHAT);
        let s = st(&t);
        assert_eq!(s.messages["D-1"].message_id, 100);
        assert_eq!(s.messages["D-1"].text, t.bot.api.sent[0].text);
        assert!(s.messages["D-1"].html && !s.messages["D-1"].done);
    }

    #[test]
    fn coming_back_stops_sending_and_going_away_sends_only_new_cards() {
        let mut t = setup(LOG);
        add(&t, "First?");
        t.bot.turn().unwrap();
        t.idle.set(Some(0));
        add(&t, "Second?");
        t.bot.turn().unwrap();
        assert_eq!(t.bot.api.sent.len(), 1);
        t.idle.set(Some(3600));
        t.bot.turn().unwrap();
        t.bot.turn().unwrap();
        assert_eq!(t.bot.api.sent.len(), 2);
        assert!(t.bot.api.sent[1].text.contains("Second?"));
    }

    #[test]
    fn a_card_answered_in_the_deck_is_edited_once_and_done() {
        let mut t = setup(LOG);
        let id = add(&t, "First?");
        t.bot.turn().unwrap();
        t.bot.store.answer(&id, 2, "").unwrap();
        t.bot.turn().unwrap();
        let e = edit_of(&t, 100);
        assert!(e.text.contains("✓ 2 — No (in the deck)"), "{}", e.text);
        assert!(e.keyboard.is_empty());
        assert!(st(&t).messages[&id].done);
        t.bot.turn().unwrap();
        assert_eq!(t.bot.api.edits.len(), 1);
    }

    #[test]
    fn withdrawn_closed_and_deleted_cards_are_finished() {
        let mut t = setup(LOG);
        let a = add(&t, "A?");
        let b = add(&t, "B?");
        let c = add(&t, "C?");
        t.bot.turn().unwrap();
        t.bot.store.withdraw(&a, "not needed").unwrap();
        t.bot.store.answer(&b, 1, "").unwrap();
        t.bot.store.close(&b).unwrap();
        std::fs::remove_file(t.bot.dir.join(format!("{c}.md"))).unwrap();
        t.bot.turn().unwrap();
        assert!(edit_of(&t, 100).text.contains("withdrawn: not needed"));
        assert!(edit_of(&t, 101).text.contains("✓ 1 — Yes (in the deck)"));
        assert!(edit_of(&t, 102).text.contains("closed"));
        assert!(st(&t).open().is_empty());
        t.bot.turn().unwrap();
        assert_eq!(t.bot.api.edits.len(), 3, "each finished once");
    }

    #[test]
    fn the_sessions_reply_is_threaded_under_the_card_once() {
        let mut t = setup(LOG);
        let id = add(&t, "First?");
        t.bot.turn().unwrap();
        t.bot.store.ask(&id, "which run?").unwrap();
        t.bot.store.reply(&id, "the staging one").unwrap();
        t.bot.turn().unwrap();
        t.bot.turn().unwrap();
        let replies: Vec<&Out> = t
            .bot
            .api
            .sent
            .iter()
            .filter(|o| o.reply_to == Some(100))
            .collect();
        assert_eq!(replies.len(), 1);
        assert_eq!(replies[0].text, "session: the staging one");
        assert_eq!(st(&t).messages[&id].replies, vec![101]);
    }

    #[test]
    fn off_from_the_terminal_between_turns_is_kept() {
        let mut t = setup(LOG);
        add(&t, "First?");
        state::update(&t.bot.dir, |s| s.enabled = false).unwrap();
        t.bot.turn().unwrap();
        t.bot.turn().unwrap();
        assert!(!st(&t).enabled);
        assert!(t.bot.api.sent.is_empty());
    }

    #[test]
    fn a_card_telegram_refuses_as_html_goes_as_plain_text() {
        let mut t = setup(LOG);
        t.bot.api.refuse_html = true;
        let id = add(&t, "a <b> c?");
        t.bot.turn().unwrap();
        let o = &t.bot.api.sent[0];
        assert!(!o.html && o.text.contains("a <b> c?"), "{}", o.text);
        assert!(!st(&t).messages[&id].html);
    }

    #[test]
    fn unknown_idle_time_counts_as_away_and_warns_once() {
        let mut t = setup(LOG);
        t.idle.set(None);
        add(&t, "First?");
        t.bot.turn().unwrap();
        t.bot.turn().unwrap();
        assert_eq!(t.bot.api.sent.len(), 1);
        assert_eq!(
            t.logs
                .borrow()
                .iter()
                .filter(|l| l.contains("idle"))
                .count(),
            1
        );
    }

    #[test]
    fn nothing_happens_before_pairing() {
        let mut t = setup(LOG);
        state::update(&t.bot.dir, |s| s.chat_id = None).unwrap();
        add(&t, "First?");
        t.bot.api.updates.push(msg(1, "/off", None));
        t.bot.turn().unwrap();
        assert!(t.bot.api.sent.is_empty());
    }

    #[test]
    fn the_summary() {
        let mut t = setup(LOG);
        add(&t, "First?");
        t.bot.turn().unwrap();
        add(&t, "Second?");
        let s = summary(&st(&t), &t.bot.store.list().0, Some(30), 5);
        assert_eq!(
            s,
            "on · at the computer (idle 0m)\n2 open\nD-1 · First? · sent\nD-2 · Second? · not sent yet"
        );
        assert!(
            summary(&State::default(), &[], None, 5).starts_with("off · away (idle time unknown)")
        );
    }
}
