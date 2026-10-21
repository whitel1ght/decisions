//! The service around the bot's turns (backoff, a fresh token after a 401, `last_error`,
//! stopping) and pairing a chat.

use std::hash::{BuildHasher, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::telegram::api::{Api, ApiError, Kind};
use crate::telegram::bot::Bot;
use crate::telegram::state;

/// Seconds to wait after an error, and whether to fetch the token again first.
pub fn wait_after(e: &ApiError, backoff: &mut u64) -> (u64, bool) {
    match e {
        ApiError::Retry { after: Some(s), .. } => (*s, false),
        ApiError::Unauthorized(_) => (60, true),
        ApiError::Forbidden(_) => (60, false),
        _ => {
            let w = *backoff;
            *backoff = (*backoff * 2).min(60);
            (w, false)
        }
    }
}

fn pause(secs: u64, stop: &AtomicBool, sleep: &mut dyn FnMut(u64)) {
    for _ in 0..secs {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        sleep(1);
    }
}

/// Turns until `stop`. Errors are logged when they change, kept in `last_error` until the next
/// success, and waited out; a 401 (or `connected == false`) gets a fresh client from `connect`.
pub fn serve<A: Api>(
    mut bot: Bot<A>,
    connected: bool,
    mut connect: impl FnMut() -> Result<A, String>,
    stop: &AtomicBool,
    sleep: &mut dyn FnMut(u64),
) {
    let mut backoff = 1;
    let mut failing = true;
    let mut logged: Option<String> = None;
    let mut need_connect = !connected;
    let mut note = |bot: &mut Bot<A>, msg: &str| {
        if logged.as_deref() != Some(msg) {
            (bot.log)(&format!("telegram: {msg}"));
            logged = Some(msg.to_string());
        }
        let _ = state::update(&bot.dir, |s| s.last_error = Some(msg.to_string()));
    };
    while !stop.load(Ordering::Relaxed) {
        if need_connect {
            match connect() {
                Ok(api) => {
                    bot.api = api;
                    need_connect = false;
                }
                Err(m) => {
                    note(&mut bot, &m);
                    pause(60, stop, sleep);
                }
            }
            continue;
        }
        match bot.turn() {
            Ok(()) => {
                backoff = 1;
                if failing {
                    failing = false;
                    let _ = state::update(&bot.dir, |s| s.last_error = None);
                }
            }
            Err(e) => {
                failing = true;
                note(&mut bot, &e.to_string());
                let (secs, reconnect) = wait_after(&e, &mut backoff);
                pause(secs, stop, sleep);
                need_connect = reconnect;
            }
        }
    }
}

/// Six random digits.
pub fn code() -> String {
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0),
    );
    format!("{:06}", h.finish() % 1_000_000)
}

/// Waits for `/start <code>` from a private chat until `deadline_ms`; saves that chat (dropping
/// what was sent to an earlier one) and confirms there. Updates seen are consumed.
pub fn pair<A: Api>(
    api: &mut A,
    dir: &Path,
    code: &str,
    deadline_ms: i64,
    now: &dyn Fn() -> i64,
    sleep: &mut dyn FnMut(u64),
) -> Result<i64, String> {
    let mut offset = state::load(dir)?.offset;
    let want = format!("/start {code}");
    while now() < deadline_ms {
        let updates = match api.get_updates(offset, 5) {
            Ok(u) => u,
            Err(ApiError::Retry { .. }) => {
                sleep(1);
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        for u in updates {
            offset = offset.max(u.id + 1);
            if let Kind::Message {
                chat_id,
                private: true,
                text,
                ..
            } = &u.kind
                && text.trim() == want
            {
                let chat = *chat_id;
                state::update(dir, |s| {
                    s.chat_id = Some(chat);
                    s.enabled = true;
                    s.messages.clear();
                    s.pending.clear();
                    s.offset = offset;
                    s.last_error = None;
                })?;
                api.send(
                    chat,
                    "Paired. You'll get decisions here while you're away.",
                    false,
                    &[],
                    None,
                )
                .map_err(|e| e.to_string())?;
                return Ok(chat);
            }
        }
        state::update(dir, |s| s.offset = offset)?;
    }
    Err("pairing timed out: no /start with the code within 5 minutes".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Mode};
    use crate::telegram::api::Update;
    use crate::telegram::fake::{Fake, FixedIdle};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn cfg() -> Config {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "decisions-service-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        state::update(&dir, |s| {
            s.chat_id = Some(7);
            s.enabled = true;
        })
        .unwrap();
        Config {
            dir,
            mode: Mode::Always,
            wait: 50,
            on_answer: None,
            mr_strip: String::new(),
            session: None,
            telegram: Default::default(),
        }
    }

    fn bot(c: &Config, api: Fake, logs: &Rc<RefCell<Vec<String>>>) -> Bot<Fake> {
        let mut b = Bot::new(api, c);
        b.idle = Box::new(FixedIdle(Rc::new(Cell::new(Some(3600)))));
        let l = logs.clone();
        b.log = Box::new(move |m| l.borrow_mut().push(m.to_string()));
        b
    }

    fn retry(m: &str) -> ApiError {
        ApiError::Retry {
            after: None,
            msg: m.into(),
        }
    }

    #[test]
    fn how_long_to_wait() {
        let mut b = 1;
        assert_eq!(
            wait_after(
                &ApiError::Retry {
                    after: Some(3),
                    msg: "x".into()
                },
                &mut b
            ),
            (3, false)
        );
        let backs: Vec<u64> = (0..8).map(|_| wait_after(&retry("x"), &mut b).0).collect();
        assert_eq!(backs, vec![1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(
            wait_after(&ApiError::Unauthorized("x".into()), &mut b),
            (60, true)
        );
        assert_eq!(
            wait_after(&ApiError::Forbidden("x".into()), &mut b),
            (60, false)
        );
    }

    #[test]
    fn errors_back_off_are_recorded_once_and_cleared() {
        let c = cfg();
        let stop = Arc::new(AtomicBool::new(false));
        let mut api = Fake::default();
        api.fail.extend([retry("net down"), retry("net down")]);
        api.stop = Some((1, stop.clone()));
        let logs = Rc::new(RefCell::new(Vec::new()));
        let mut slept = 0;
        let mut seen = None;
        let dir = c.dir.clone();
        serve(
            bot(&c, api, &logs),
            true,
            || Err("unused".into()),
            &stop,
            &mut |_| {
                slept += 1;
                seen = state::load(&dir).unwrap().last_error;
            },
        );
        assert_eq!(slept, 3, "1 s, then 2 s");
        assert_eq!(seen.as_deref(), Some("net down"));
        assert_eq!(
            state::load(&c.dir).unwrap().last_error,
            None,
            "cleared by the next success"
        );
        assert_eq!(
            logs.borrow()
                .iter()
                .filter(|l| l.contains("net down"))
                .count(),
            1
        );
    }

    #[test]
    fn a_401_fetches_the_token_again_until_it_works() {
        let c = cfg();
        let stop = Arc::new(AtomicBool::new(false));
        let mut api = Fake::default();
        api.fail.push_back(ApiError::Unauthorized(
            "getUpdates: 401 Unauthorized".into(),
        ));
        let logs = Rc::new(RefCell::new(Vec::new()));
        let mut tries = 0;
        let mut slept = 0;
        let s2 = stop.clone();
        serve(
            bot(&c, api, &logs),
            true,
            || {
                tries += 1;
                if tries == 1 {
                    return Err("token_cmd failed".into());
                }
                let mut f = Fake::default();
                f.stop = Some((1, s2.clone()));
                Ok(f)
            },
            &stop,
            &mut |_| slept += 1,
        );
        assert_eq!((tries, slept), (2, 120));
        let l = logs.borrow().join("\n");
        assert!(l.contains("401") && l.contains("token_cmd failed"), "{l}");
    }

    #[test]
    fn starting_without_a_token_connects_first() {
        let c = cfg();
        let stop = Arc::new(AtomicBool::new(false));
        let logs = Rc::new(RefCell::new(Vec::new()));
        let mut tries = 0;
        let s2 = stop.clone();
        serve(
            bot(&c, Fake::default(), &logs),
            false,
            || {
                tries += 1;
                let mut f = Fake::default();
                f.stop = Some((1, s2.clone()));
                Ok(f)
            },
            &stop,
            &mut |_| {},
        );
        assert_eq!(tries, 1);
    }

    #[test]
    fn stop_ends_a_pause() {
        let c = cfg();
        let stop = AtomicBool::new(false);
        let mut api = Fake::default();
        api.fail.push_back(ApiError::Retry {
            after: Some(30),
            msg: "slow".into(),
        });
        let logs = Rc::new(RefCell::new(Vec::new()));
        let mut slept = 0;
        serve(
            bot(&c, api, &logs),
            true,
            || Err("unused".into()),
            &stop,
            &mut |_| {
                slept += 1;
                stop.store(true, Ordering::Relaxed);
            },
        );
        assert_eq!(slept, 1);
    }

    fn ticks() -> impl Fn() -> i64 {
        let c = Cell::new(0);
        move || {
            c.set(c.get() + 1000);
            c.get()
        }
    }

    fn text(id: i64, chat: i64, private: bool, t: &str) -> Update {
        Update {
            id,
            kind: Kind::Message {
                chat_id: chat,
                private,
                message_id: id,
                text: t.into(),
                reply_to: None,
            },
        }
    }

    #[test]
    fn pairs_the_private_chat_that_sends_the_code() {
        let c = cfg();
        state::update(&c.dir, |s| {
            s.chat_id = None;
            s.messages.insert("D-1".into(), Default::default());
        })
        .unwrap();
        let mut api = Fake::default();
        api.updates = vec![
            text(1, 9, false, "/start 123456"),
            text(2, 8, true, "/start 999999"),
            text(3, 7, true, " /start 123456 "),
        ];
        assert_eq!(
            pair(&mut api, &c.dir, "123456", 300_000, &ticks(), &mut |_| {}),
            Ok(7)
        );
        let s = state::load(&c.dir).unwrap();
        assert_eq!(
            (s.chat_id, s.enabled, s.offset, s.messages.len()),
            (Some(7), true, 4, 0)
        );
        assert_eq!(api.sent[0].chat, 7);
        assert!(api.sent[0].text.starts_with("Paired"));
    }

    #[test]
    fn pairing_times_out_and_keeps_the_offset() {
        let c = cfg();
        let mut api = Fake::default();
        api.updates = vec![text(1, 9, false, "/start 123456")];
        let r = pair(&mut api, &c.dir, "123456", 10_000, &ticks(), &mut |_| {});
        assert!(r.unwrap_err().contains("timed out"));
        assert_eq!(state::load(&c.dir).unwrap().offset, 2);
    }

    #[test]
    fn pairing_rides_out_network_trouble() {
        let c = cfg();
        let mut api = Fake::default();
        api.fail.push_back(retry("net down"));
        api.updates = vec![text(1, 7, true, "/start 123456")];
        assert_eq!(
            pair(&mut api, &c.dir, "123456", 300_000, &ticks(), &mut |_| {}),
            Ok(7)
        );
    }

    #[test]
    fn codes_are_six_digits() {
        for _ in 0..20 {
            let c = code();
            assert!(c.len() == 6 && c.bytes().all(|b| b.is_ascii_digit()), "{c}");
        }
    }
}
