//! `decisions --demo`: a made-up queue held in memory, to try the screen, the keys and the
//! animations without `decide`, the run-day state or the session. Answers and asks behave
//! like the real ones, but only here; one card always fails to answer and one fails to ask
//! back, so the error paths can be seen too.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::queue::{Backend, Decision, Entry, Opt};

/// The card whose answer always fails (failed-send badge, retry, quit warning).
pub const FAILS_ANSWER: &str = "D-3";
/// The card whose ask back always fails (failed-ask badge, retry with the text kept).
pub const FAILS_ASK: &str = "D-4";
pub const ARRIVE_EVERY_MS: i64 = 20_000;
pub const REPLY_AFTER_MS: i64 = 4_000;
const MIN: i64 = 60_000;

type Clock = Box<dyn Fn() -> i64 + Send + Sync>;

pub struct Demo {
    clock: Clock,
    state: Mutex<DemoState>,
}

struct DemoState {
    open: Vec<Decision>,
    arrivals: Vec<Decision>,
    replies: Vec<(i64, String)>,
}

fn iso(ms: i64) -> String {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn card(
    id: &str,
    key: &str,
    mr: Option<(&str, u32)>,
    blocks: &str,
    asked: i64,
    question: &str,
    options: &[&str],
    star: Option<u8>,
    context: &str,
) -> Decision {
    let (mr, mr_short) = match mr {
        Some((repo, iid)) => (
            format!("https://gitlab.com/acme/acme-{repo}/-/merge_requests/{iid}"),
            format!("{repo}!{iid}"),
        ),
        None => (String::new(), String::new()),
    };
    Decision {
        id: id.into(),
        key: key.into(),
        mr,
        mr_short,
        question: question.into(),
        asked: iso(asked),
        blocks: blocks.into(),
        question_back: false,
        options: options
            .iter()
            .zip(1u8..)
            .map(|(text, n)| Opt {
                n,
                text: (*text).into(),
                recommended: star == Some(n),
            })
            .collect(),
        recommended: star,
        context: context.into(),
        discussion: Vec::new(),
    }
}

fn entry(who: &str, text: &str) -> Entry {
    Entry {
        at: "demo".into(),
        who: who.into(),
        text: text.into(),
    }
}

const LONG_CONTEXT: &str = "### What the reviewer asked

@alex asked for two changes on backend!870 before approving:

- name the two DMS save paths the fallback metric still misses
- add a test for the retry path, so a second save is counted once

### What the agent did

The fix is on the branch and the full unit suite passes (2,349 tests). The reply below is
drafted and waits for you:

```
Both paths now go through FallbackMeter.record(); the retry path is covered by
DmsSaveRetrySpec. Thanks for catching the second one.
```

### Why push now

- the MR has one approval already, and this is the last open thread
- QA wants it in today's build, which is cut at 15:00
- nothing else on the branch changed since the review

### Risks

- the metric name changed from `dms.fallback.saves` to `dms.fallback.save_count`; the
  Grafana panel needs the new name (a one-line change, noted in the description)
- a long link to check the line cutting: https://gitlab.com/acme/acme-backend/-/merge_requests/870/diffs#note_4026396373_and_some_more_text_to_make_it_long

### If you pick \"Push only\"

The reply stays as a draft on the MR, and the session reminds you at the end of the day.

### If you pick \"Leave it for now\"

The branch stays as it is; the decision comes back tomorrow morning.

Scroll with j/k, ctrl-d/ctrl-u, g and G; space or esc flips back.";

impl Demo {
    pub fn new() -> Self {
        Self::with_clock(Box::new(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64)
        }))
    }

    pub fn with_clock(clock: Clock) -> Self {
        let now = clock();
        let mut d1 = card(
            "D-1",
            "ACME-1004",
            None,
            "reply",
            now - 250 * MIN,
            "Reply to the client's question about the export columns?",
            &[
                "Send the drafted reply",
                "Change it first (add a note with n)",
                "I'll answer myself",
            ],
            Some(1),
            "The session asked you something back on this one (the green ↩ reply badge).\n\nDrafted reply:\n\n> The export keeps the column order you set; hidden columns stay out of the file.",
        );
        d1.question_back = true;
        let mut d2 = card(
            "D-2",
            "ACME-1003",
            Some(("dashboard", 940)),
            "merge",
            now - 125 * MIN,
            "Merge dashboard!940 now that QA passed?",
            &["Merge", "Wait for a second approval"],
            Some(1),
            "QA passed on staging. One approval so far; the repo asks for one.",
        );
        d2.discussion = vec![
            entry("you", "Which QA run passed?"),
            entry(
                "session",
                "The staging run from this morning, all 42 checks green.",
            ),
        ];
        Self {
            state: Mutex::new(DemoState {
                open: vec![
                    d1,
                    d2,
                    card(
                        FAILS_ANSWER,
                        "ACME-1006",
                        None,
                        "",
                        now - 45 * MIN,
                        "Close ACME-1006 as a duplicate of ACME-1003?",
                        &["Close it", "Keep it open"],
                        None,
                        "In the demo, answering this card always fails: you see the red send-failed badge, ⏎ retries, and quitting with it pending keeps the window open.",
                    ),
                    card(
                        FAILS_ASK,
                        "ACME-1002",
                        None,
                        "dispatch",
                        now - 20 * MIN,
                        "Start ACME-1002 now, or after the review round?",
                        &["Start now", "After the review round", "Not today"],
                        None,
                        "No ★ here: ⏎ does nothing until you move to an option with j/k.\n\nIn the demo, asking back (a) on this card fails, to show the failed-ask badge; a again offers your text for a retry.",
                    ),
                    card(
                        "D-5",
                        "ACME-1007",
                        None,
                        "",
                        now - 14 * MIN - 30_000,
                        "Rebase the long-running branch onto main?",
                        &["Rebase", "Merge main in", "Leave it"],
                        Some(1),
                        "This card turns from blue to yellow about half a minute after the demo starts.",
                    ),
                    card(
                        "D-6",
                        "",
                        None,
                        "",
                        now - 7 * MIN,
                        "Which of these should go into tomorrow's plan?",
                        &[
                            "All four",
                            "Only the reviews",
                            "Only ACME-1005 and ACME-1006",
                            "None of them",
                        ],
                        Some(2),
                        "No ticket and no MR: o says there is none.",
                    ),
                    card(
                        "D-7",
                        "ACME-1008",
                        Some(("backend", 871)),
                        "review",
                        now - 3 * MIN,
                        "Re-review @sam's MR now that they replied?",
                        &["Re-review now", "Later today"],
                        Some(1),
                        "Sam answered both threads and pushed one commit.",
                    ),
                    card(
                        "D-8",
                        "ACME-1001",
                        Some(("backend", 870)),
                        "reply",
                        now - MIN,
                        "Push the review fix and post the reply to @alex?",
                        &["Push and post", "Push only", "Leave it for now"],
                        Some(1),
                        LONG_CONTEXT,
                    ),
                ],
                arrivals: vec![
                    card(
                        "D-9",
                        "ACME-1009",
                        None,
                        "",
                        now + ARRIVE_EVERY_MS,
                        "New ticket ACME-1009 assigned to you: add it to today's plan?",
                        &["Add it", "Not today"],
                        Some(1),
                        "A new decision arrived while you were looking.",
                    ),
                    card(
                        "D-10",
                        "ACME-1008",
                        Some(("backend", 871)),
                        "",
                        now + 2 * ARRIVE_EVERY_MS,
                        "The pipeline failed on backend!871: dispatch a fix agent?",
                        &["Dispatch it", "I'll look myself"],
                        Some(1),
                        "Two unit tests fail in RetrieveV2ItemSpec.",
                    ),
                    card(
                        "D-11",
                        "ACME-1003",
                        None,
                        "",
                        now + 3 * ARRIVE_EVERY_MS,
                        "Withdraw the older question about ACME-1003?",
                        &["Withdraw it", "Keep both"],
                        None,
                        "D-2 asks the same thing.",
                    ),
                ],
                replies: Vec::new(),
            }),
            clock,
        }
    }

    fn settle(&self, s: &mut DemoState) {
        let now = (self.clock)();
        while s
            .arrivals
            .first()
            .is_some_and(|d| d.asked_ms().unwrap_or(i64::MAX) <= now)
        {
            let d = s.arrivals.remove(0);
            s.open.push(d);
        }
        let (due, later): (Vec<_>, Vec<_>) = std::mem::take(&mut s.replies)
            .into_iter()
            .partition(|(at, _)| *at <= now);
        s.replies = later;
        for (_, id) in due {
            if let Some(d) = s.open.iter_mut().find(|d| d.id == id) {
                d.discussion.push(entry(
                    "session",
                    "(demo) Good question: the context on the back has the details.",
                ));
            }
        }
    }
}

impl Default for Demo {
    fn default() -> Self {
        Self::new()
    }
}

impl Backend for Demo {
    fn list(&self) -> Result<Vec<Decision>, String> {
        let mut s = self
            .state
            .lock()
            .map_err(|_| "demo: state poisoned".to_string())?;
        self.settle(&mut s);
        Ok(s.open.clone())
    }

    fn tell(&self, id: &str, _text: &str) -> Result<(), String> {
        self.answer(id, 0, None)
    }

    fn answer(&self, id: &str, _n: u8, _note: Option<&str>) -> Result<(), String> {
        if id == FAILS_ANSWER {
            return Err("demo: answering this card always fails".into());
        }
        let mut s = self
            .state
            .lock()
            .map_err(|_| "demo: state poisoned".to_string())?;
        let before = s.open.len();
        s.open.retain(|d| d.id != id);
        if s.open.len() == before {
            Err(format!("demo: {id} is not open"))
        } else {
            Ok(())
        }
    }

    fn ask(&self, id: &str, text: &str) -> Result<(), String> {
        if id == FAILS_ASK {
            return Err("demo: asking back on this card always fails".into());
        }
        let mut s = self
            .state
            .lock()
            .map_err(|_| "demo: state poisoned".to_string())?;
        let due = (self.clock)() + REPLY_AFTER_MS;
        let d = s
            .open
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| format!("demo: {id} is not open"))?;
        d.discussion.push(entry("you", text));
        s.replies.push((due, id.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Band, band};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    const T0: i64 = 1_792_483_448_000;
    const SEC: i64 = 1_000;

    fn demo() -> (Demo, Arc<AtomicI64>) {
        let now = Arc::new(AtomicI64::new(T0));
        let clock = now.clone();
        (
            Demo::with_clock(Box::new(move || clock.load(Ordering::Relaxed))),
            now,
        )
    }

    fn ids(list: &[Decision]) -> Vec<&str> {
        list.iter().map(|d| d.id.as_str()).collect()
    }

    fn age(d: &Decision, now: i64) -> i64 {
        now - d.asked_ms().unwrap()
    }

    #[test]
    fn starts_with_eight_cards_oldest_first_covering_every_age_band() {
        let (demo, _) = demo();
        let list = demo.list().unwrap();
        assert_eq!(
            ids(&list),
            ["D-1", "D-2", "D-3", "D-4", "D-5", "D-6", "D-7", "D-8"]
        );
        let bands: Vec<Band> = list.iter().map(|d| band(age(d, T0))).collect();
        for b in [Band::Fresh, Band::Warm, Band::Old, Band::Stale] {
            assert!(bands.contains(&b), "{b:?} missing");
        }
        assert!(list.windows(2).all(|w| w[0].asked_ms() <= w[1].asked_ms()));
    }

    #[test]
    fn the_cards_cover_what_the_screen_can_show() {
        let (demo, _) = demo();
        let list = demo.list().unwrap();
        assert!(list.iter().any(|d| d.question_back), "a ↩ reply card");
        assert!(
            list.iter().any(|d| d.recommended.is_none()),
            "a card without ★"
        );
        assert!(
            list.iter().any(|d| d.options.len() == 4 && d.mr.is_empty()),
            "4 options, no MR"
        );
        assert!(
            list.iter().any(|d| !d.discussion.is_empty()),
            "a discussion"
        );
        assert!(
            list.iter().any(|d| d.context.lines().count() > 30),
            "a long context to scroll"
        );
    }

    #[test]
    fn an_answer_takes_the_card_off_the_queue() {
        let (demo, _) = demo();
        demo.answer("D-1", 1, Some("note")).unwrap();
        assert!(!ids(&demo.list().unwrap()).contains(&"D-1"));
        assert_eq!(
            demo.answer("D-1", 1, None).unwrap_err(),
            "demo: D-1 is not open"
        );
    }

    #[test]
    fn answering_the_failing_card_always_fails() {
        let (demo, _) = demo();
        let err = demo.answer(FAILS_ANSWER, 1, None).unwrap_err();
        assert!(err.starts_with("demo:"), "{err}");
        assert!(ids(&demo.list().unwrap()).contains(&FAILS_ANSWER));
    }

    #[test]
    fn asking_back_fails_on_one_card_and_gets_a_reply_on_the_others() {
        let (demo, now) = demo();
        assert!(demo.ask(FAILS_ASK, "why?").is_err());
        demo.ask("D-1", "why?").unwrap();
        let d1 = |demo: &Demo| {
            demo.list()
                .unwrap()
                .into_iter()
                .find(|d| d.id == "D-1")
                .unwrap()
        };
        let disc = d1(&demo).discussion;
        assert_eq!(disc.last().unwrap().who, "you");
        assert_eq!(disc.last().unwrap().text, "why?");
        now.fetch_add(REPLY_AFTER_MS, Ordering::Relaxed);
        assert_eq!(d1(&demo).discussion.last().unwrap().who, "session");
    }

    #[test]
    fn new_decisions_arrive_every_twenty_seconds_three_times() {
        let (demo, now) = demo();
        assert_eq!(demo.list().unwrap().len(), 8);
        now.fetch_add(ARRIVE_EVERY_MS, Ordering::Relaxed);
        assert_eq!(ids(&demo.list().unwrap()).last(), Some(&"D-9"));
        now.fetch_add(10 * ARRIVE_EVERY_MS, Ordering::Relaxed);
        let list = demo.list().unwrap();
        assert_eq!(list.len(), 11);
        assert_eq!(ids(&list)[8..], ["D-9", "D-10", "D-11"]);
    }

    #[test]
    fn one_card_turns_yellow_half_a_minute_in() {
        let (demo, _) = demo();
        let list = demo.list().unwrap();
        let soon = list.iter().filter(|d| {
            band(age(d, T0)) == Band::Fresh && band(age(d, T0 + 30 * SEC)) == Band::Warm
        });
        assert_eq!(soon.count(), 1);
    }

    #[test]
    fn telling_takes_the_card_off_except_on_the_failing_one() {
        let (demo, _) = demo();
        demo.tell("D-1", "close it").unwrap();
        assert!(!ids(&demo.list().unwrap()).contains(&"D-1"));
        assert!(demo.tell(FAILS_ANSWER, "close it").is_err());
    }
}
