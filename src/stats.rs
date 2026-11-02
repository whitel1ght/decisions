//! How the queue has been used: per-day counts and time to answer over a window of days.
//! Pure over loaded cards, so the CLI and the deck show the same numbers.

use std::collections::HashMap;

use chrono::{DateTime, Duration, FixedOffset};
use serde::Serialize;

use crate::store::Card;

const TOP_BLOCKS: usize = 5;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DayStats {
    pub day: String,
    pub asked: usize,
    pub answered: usize,
    pub withdrawn: usize,
    pub open: usize,
    /// Median seconds from asked to answered, over this day's answered questions.
    pub median_secs: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    pub window_days: u32,
    /// Oldest first; every day of the window, quiet ones included.
    pub days: Vec<DayStats>,
    pub answered: usize,
    pub median_secs: Option<i64>,
    pub p90_secs: Option<i64>,
    /// Answer cards filed in the window, counted apart from questions.
    pub answer_cards: usize,
    /// The most common `blocks` values among the window's questions, most first.
    pub top_blocks: Vec<(String, usize)>,
    /// Cards in the window per harness (the text before ` · ` in `source`), most first.
    pub sources: Vec<(String, usize)>,
    /// Open questions now, whatever day they were asked.
    pub open_now: usize,
    pub oldest_open_secs: Option<i64>,
}

fn median(sorted: &[i64]) -> Option<i64> {
    let n = sorted.len();
    match n {
        0 => None,
        _ if n % 2 == 1 => Some(sorted[n / 2]),
        _ => Some((sorted[n / 2 - 1] + sorted[n / 2]) / 2),
    }
}

/// Nearest-rank percentile of an ascending slice.
fn percentile(sorted: &[i64], p: usize) -> Option<i64> {
    let n = sorted.len();
    (n > 0).then(|| sorted[(p * n).div_ceil(100).max(1) - 1])
}

fn ranked(counts: HashMap<String, usize>) -> Vec<(String, usize)> {
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

/// Statistics over the `days` days ending with `now`'s day, by the day each card was asked in
/// `now`'s offset.
pub fn compute(cards: &[Card], now: DateTime<FixedOffset>, days: u32) -> Stats {
    let days = days.max(1);
    let today = now.date_naive();
    let first = today - Duration::days(i64::from(days) - 1);
    let mut rows: Vec<DayStats> = (0..i64::from(days))
        .map(|i| DayStats {
            day: (first + Duration::days(i)).to_string(),
            asked: 0,
            answered: 0,
            withdrawn: 0,
            open: 0,
            median_secs: None,
        })
        .collect();
    let mut per_day: Vec<Vec<i64>> = vec![Vec::new(); rows.len()];
    let mut all = Vec::new();
    let (mut blocks, mut sources) = (HashMap::new(), HashMap::new());
    let (mut answer_cards, mut open_now) = (0, 0);
    let mut oldest_open = None;

    for c in cards {
        if !c.answer_card && c.status == "open" {
            open_now += 1;
            let age = (now - c.asked).num_seconds().max(0);
            oldest_open = oldest_open.max(Some(age));
        }
        let day = c.asked.with_timezone(now.offset()).date_naive();
        if day < first || day > today {
            continue;
        }
        let harness = c.source.split(" · ").next().unwrap_or("").trim();
        if !harness.is_empty() {
            *sources.entry(harness.to_string()).or_insert(0) += 1;
        }
        if c.answer_card {
            answer_cards += 1;
            continue;
        }
        let i = (day - first).num_days() as usize;
        let row = &mut rows[i];
        row.asked += 1;
        match c.status.as_str() {
            "open" => row.open += 1,
            "withdrawn" => row.withdrawn += 1,
            "answered" => {
                row.answered += 1;
                if let Some(at) = c.answered {
                    let secs = (at - c.asked).num_seconds().max(0);
                    per_day[i].push(secs);
                    all.push(secs);
                }
            }
            _ => {}
        }
        if !c.blocks.is_empty() {
            *blocks.entry(c.blocks.clone()).or_insert(0) += 1;
        }
    }

    for (row, secs) in rows.iter_mut().zip(per_day.iter_mut()) {
        secs.sort_unstable();
        row.median_secs = median(secs);
    }
    all.sort_unstable();
    let mut top_blocks = ranked(blocks);
    top_blocks.truncate(TOP_BLOCKS);
    Stats {
        window_days: days,
        answered: rows.iter().map(|r| r.answered).sum(),
        days: rows,
        median_secs: median(&all),
        p90_secs: percentile(&all, 90),
        answer_cards,
        top_blocks,
        sources: ranked(sources),
        open_now,
        oldest_open_secs: oldest_open,
    }
}

/// `45s`, `12m`, `3h 05m`, `2d 4h`.
pub fn human(secs: i64) -> String {
    let (m, h, d) = (secs / 60, secs / 3600, secs / 86400);
    if secs < 60 {
        format!("{secs}s")
    } else if h == 0 {
        format!("{m}m")
    } else if d == 0 {
        format!("{h}h {:02}m", m % 60)
    } else {
        format!("{d}d {}h", h % 24)
    }
}

/// The plain-text report `decisions stats` prints.
pub fn lines(s: &Stats) -> Vec<String> {
    let opt = |v: Option<i64>| v.map_or("-".into(), human);
    let mut out = vec![format!(
        "{:<10}  {:>5}  {:>8}  {:>9}  {:>4}  {:>8}",
        "day", "asked", "answered", "withdrawn", "open", "median"
    )];
    for d in &s.days {
        out.push(format!(
            "{:<10}  {:>5}  {:>8}  {:>9}  {:>4}  {:>8}",
            d.day,
            d.asked,
            d.answered,
            d.withdrawn,
            d.open,
            opt(d.median_secs)
        ));
    }
    out.push(String::new());
    out.push(format!(
        "time to answer ({} answered): median {}, p90 {}",
        s.answered,
        opt(s.median_secs),
        opt(s.p90_secs)
    ));
    out.push(format!(
        "open now: {}, oldest {}",
        s.open_now,
        opt(s.oldest_open_secs)
    ));
    out.push(format!("answer cards: {}", s.answer_cards));
    let join = |v: &[(String, usize)]| {
        if v.is_empty() {
            "-".to_string()
        } else {
            v.iter()
                .map(|(k, n)| format!("{k} {n}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    out.push(format!("blocks: {}", join(&s.top_blocks)));
    out.push(format!("sources: {}", join(&s.sources)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    fn card(asked: &str, status: &str, took_mins: Option<i64>) -> Card {
        let asked = t(asked);
        Card {
            asked,
            status: status.into(),
            answer_card: false,
            blocks: String::new(),
            source: "claude-code · projects".into(),
            answered: took_mins.map(|m| asked + Duration::minutes(m)),
        }
    }

    const NOW: &str = "2026-11-02T12:00:00+03:00";

    #[test]
    fn counts_per_day() {
        let cards = [
            card("2026-11-02T09:00:00+03:00", "answered", Some(10)),
            card("2026-11-02T10:00:00+03:00", "open", None),
            card("2026-11-01T09:00:00+03:00", "withdrawn", None),
            card("2026-11-01T11:00:00+03:00", "answered", Some(30)),
        ];
        let s = compute(&cards, t(NOW), 3);
        assert_eq!(s.days.len(), 3);
        assert_eq!(s.days[0].day, "2026-10-31");
        assert_eq!(s.days[0].asked, 0);
        let (d1, d2) = (&s.days[1], &s.days[2]);
        assert_eq!((d1.asked, d1.answered, d1.withdrawn, d1.open), (2, 1, 1, 0));
        assert_eq!((d2.asked, d2.answered, d2.withdrawn, d2.open), (2, 1, 0, 1));
        assert_eq!(d1.median_secs, Some(30 * 60));
        assert_eq!(s.open_now, 1);
        assert_eq!(s.oldest_open_secs, Some(2 * 3600));
    }

    #[test]
    fn median_and_p90_odd_and_even() {
        let odd: Vec<_> = [5, 1, 9]
            .iter()
            .map(|m| card("2026-11-02T09:00:00+03:00", "answered", Some(*m)))
            .collect();
        let s = compute(&odd, t(NOW), 1);
        assert_eq!(s.median_secs, Some(5 * 60));
        assert_eq!(s.p90_secs, Some(9 * 60));
        let even: Vec<_> = [1, 2, 3, 10]
            .iter()
            .map(|m| card("2026-11-02T09:00:00+03:00", "answered", Some(*m)))
            .collect();
        let s = compute(&even, t(NOW), 1);
        assert_eq!(s.median_secs, Some(150));
        assert_eq!(s.p90_secs, Some(10 * 60));
    }

    #[test]
    fn p90_is_nearest_rank() {
        let secs: Vec<i64> = (1..=20).collect();
        assert_eq!(percentile(&secs, 90), Some(18));
        assert_eq!(percentile(&[], 90), None);
    }

    #[test]
    fn answer_cards_are_apart_from_timing_and_counts() {
        let mut a = card("2026-11-02T09:00:00+03:00", "seen", Some(500));
        a.answer_card = true;
        a.blocks = "merge".into();
        let q = card("2026-11-02T09:00:00+03:00", "answered", Some(2));
        let mut open_answer = card("2026-10-01T09:00:00+03:00", "open", None);
        open_answer.answer_card = true;
        let s = compute(&[a, q, open_answer], t(NOW), 1);
        assert_eq!(s.answer_cards, 1);
        assert_eq!(s.days[0].asked, 1);
        assert_eq!(s.median_secs, Some(120));
        assert_eq!(s.open_now, 0);
        assert!(s.top_blocks.is_empty());
    }

    #[test]
    fn empty_store() {
        let s = compute(&[], t(NOW), 14);
        assert_eq!(s.days.len(), 14);
        assert!(
            s.days
                .iter()
                .all(|d| d.asked == 0 && d.median_secs.is_none())
        );
        assert_eq!(
            (s.median_secs, s.p90_secs, s.oldest_open_secs),
            (None, None, None)
        );
        assert_eq!(s.open_now, 0);
        assert!(s.sources.is_empty());
        assert_eq!(lines(&s).len(), 14 + 1 + 6);
    }

    #[test]
    fn window_cut_off() {
        let cards = [
            card("2026-10-31T23:59:00+03:00", "answered", Some(1)),
            card("2026-10-30T23:59:00+03:00", "answered", Some(1)),
            card("2026-11-03T00:01:00+03:00", "answered", Some(1)),
        ];
        let s = compute(&cards, t(NOW), 3);
        assert_eq!(s.days.iter().map(|d| d.asked).sum::<usize>(), 1);
        assert_eq!(s.answered, 1);
        // a card outside the window that is still open counts as open now
        let old = card("2026-09-01T09:00:00+03:00", "open", None);
        let s = compute(&[old], t(NOW), 3);
        assert_eq!(s.open_now, 1);
        assert_eq!(s.days.iter().map(|d| d.asked).sum::<usize>(), 0);
    }

    #[test]
    fn days_are_read_in_the_offset_of_now() {
        // 22:30 UTC on the 1st is already the 2nd at +03:00
        let c = card("2026-11-01T22:30:00+00:00", "open", None);
        let s = compute(&[c], t(NOW), 1);
        assert_eq!(s.days[0].asked, 1);
    }

    #[test]
    fn top_blocks_and_sources() {
        let mk = |blocks: &str, source: &str| Card {
            blocks: blocks.into(),
            source: source.into(),
            ..card("2026-11-02T09:00:00+03:00", "open", None)
        };
        let mut cards = Vec::new();
        for (b, n) in [("a", 1), ("b", 3), ("c", 2), ("d", 1), ("e", 1), ("f", 1)] {
            for _ in 0..n {
                cards.push(mk(b, "pi · repo"));
            }
        }
        cards.push(mk("", "claude-code · projects"));
        cards.push(mk("", ""));
        let s = compute(&cards, t(NOW), 1);
        assert_eq!(s.top_blocks.len(), 5);
        assert_eq!(s.top_blocks[0], ("b".into(), 3));
        assert_eq!(s.top_blocks[1], ("c".into(), 2));
        assert_eq!(s.top_blocks[2].0, "a");
        assert_eq!(s.sources, vec![("pi".into(), 9), ("claude-code".into(), 1)]);
    }

    #[test]
    fn human_durations() {
        assert_eq!(human(45), "45s");
        assert_eq!(human(12 * 60), "12m");
        assert_eq!(human(3 * 3600 + 5 * 60), "3h 05m");
        assert_eq!(human(2 * 86400 + 4 * 3600), "2d 4h");
    }
}
