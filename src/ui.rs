//! Draws the state: the frame, the deck (front card and stack edges), the back, the status
//! line and the help. Returns where the card and edges landed (for `fx`) and how far the
//! back can scroll (for `app`).

use ratatui::Frame;
use ratatui::layout::{Alignment, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph, Wrap};

use crate::app::{FLIP_MS, InputKind, Side, State, UNDO_MS, age_label, band};
use crate::markdown;
use crate::queue::Decision;
use crate::theme;

pub const MIN_W: u16 = 50;
pub const MIN_H: u16 = 14;
const MAX_EDGES: usize = 4;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Areas {
    pub card: Rect,
    pub edges: Vec<(String, Rect)>,
    pub current: Option<String>,
    pub empty: Option<Rect>,
}

pub struct Drawn {
    pub areas: Areas,
    pub scroll_max: u16,
}

const KEYS: &[(&str, &str)] = &[
    ("J / K", "next / previous card"),
    ("j / k", "move between the options (back: scroll)"),
    ("g / G", "first / last option (back: top / bottom)"),
    ("1-9", "answer with that option"),
    ("⏎", "option under the cursor, else ★"),
    ("↑ / ↓", "same as j / k"),
    ("space", "flip: context and discussion"),
    ("^d / ^u", "page the back"),
    ("u", "undo the last answer"),
    ("n", "note on the card, or on the last answer"),
    ("last row", "something else: tell the agent in your words"),
    ("a", "ask the session back"),
    ("s", "skip to the end of the stack"),
    ("o", "open the link (MR, PR, ticket)"),
    ("r", "reload"),
    ("q / esc", "send pending answers and close"),
];

pub fn draw(f: &mut Frame, s: &State) -> Drawn {
    let area = f.area();
    f.render_widget(
        Block::new().style(Style::new().bg(theme::BG).fg(theme::TEXT)),
        area,
    );
    let mut drawn = Drawn {
        areas: Areas::default(),
        scroll_max: 0,
    };
    if area.width < MIN_W || area.height < MIN_H {
        let msg = Paragraph::new(format!("make the window larger ({MIN_W}×{MIN_H})"))
            .alignment(Alignment::Center)
            .style(Style::new().fg(theme::MUTED));
        f.render_widget(
            msg,
            Rect {
                y: area.y + area.height / 2,
                height: 1,
                ..area
            },
        );
        return drawn;
    }
    let frame = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::LAVENDER))
        .title(
            Line::from(header(s)).style(Style::new().fg(theme::BLUE).add_modifier(Modifier::BOLD)),
        )
        .title_bottom(Line::from(footer(s)).style(Style::new().fg(theme::MUTED)));
    let mut rows = frame.inner(area);
    f.render_widget(frame, area);
    if let Some(e) = &s.error {
        let line = Paragraph::new(format!(" decide failed: {e} · r retry"))
            .style(Style::new().fg(theme::RED));
        f.render_widget(line, Rect { height: 1, ..rows });
        rows.y += 1;
        rows.height -= 1;
    }
    if let Some(line) = status(s) {
        f.render_widget(
            Paragraph::new(line),
            Rect {
                y: rows.bottom() - 1,
                height: 1,
                ..rows
            },
        );
        rows.height -= 1;
    }
    let deck = rows.inner(Margin::new(1, 0));
    if !s.loaded {
        if s.error.is_none() {
            let msg = Paragraph::new("loading…")
                .alignment(Alignment::Center)
                .style(Style::new().fg(theme::MUTED));
            f.render_widget(
                msg,
                Rect {
                    y: deck.y + deck.height / 2,
                    height: 1,
                    ..deck
                },
            );
        }
    } else if s.queue.is_empty() && s.ghost.is_none() {
        drawn.areas.empty = Some(empty(f, deck));
    } else {
        deck_draw(f, s, deck, &mut drawn);
    }
    if s.help {
        help(f, area);
    }
    drawn
}

fn header(s: &State) -> String {
    let name = if s.demo {
        format!("{} · demo", s.title)
    } else {
        s.title.clone()
    };
    match s.queue.len() {
        _ if !s.loaded => format!(" {name} · loading… "),
        0 => format!(" {name} · nothing waiting "),
        n => format!(
            " {name} · {n} waiting · oldest {} ",
            age_label(s.oldest_ms())
        ),
    }
}

fn footer(s: &State) -> String {
    let keys = match (&s.input, s.help, s.side, &s.pending) {
        (Some(i), ..) => match i.kind {
            InputKind::Note => "⏎ send with note · esc no note",
            InputKind::CardNote { .. } => "⏎ attach the note (empty removes it) · esc cancel",
            InputKind::Tell { .. } => "⏎ answer in your own words · esc cancel",
            InputKind::Ask { .. } => "⏎ ask · esc cancel",
        },
        (None, true, ..) => "any key closes help",
        (None, false, Side::Back, _) => {
            "j/k scroll · ^d/^u page · J/K cards · space front · 1-9 answer · ? help"
        }
        (None, false, Side::Front, Some(_)) => "u undo · n note · J/K cards · 1-9 answer · ? help",
        (None, false, Side::Front, None) => {
            "J/K cards · j/k options · 1-9 answer · ⏎ pick · space flip · ? help"
        }
    };
    format!(" {keys} ")
}

fn status(s: &State) -> Option<Line<'static>> {
    let bold = |c: Color| Style::new().fg(c).add_modifier(Modifier::BOLD);
    if let Some(i) = &s.input {
        let prompt = match &i.kind {
            InputKind::Note => " note › ".to_string(),
            InputKind::Ask { id } => format!(" ask {id} › "),
            InputKind::CardNote { id } => format!(" note for {id} › "),
            InputKind::Tell { id } => format!(" tell {id} › "),
        };
        return Some(Line::from(vec![
            Span::styled(prompt, bold(theme::LAVENDER)),
            Span::styled(i.text.clone(), Style::new().fg(theme::TEXT)),
            Span::styled("▏", Style::new().fg(theme::LAVENDER)),
        ]));
    }
    if s.quitting {
        return Some(Line::styled(
            " sending… · q again to quit anyway",
            Style::new().fg(theme::YELLOW),
        ));
    }
    if let Some(p) = &s.pending {
        let choice = match &p.custom {
            Some(text) => format!("✎ {text}"),
            None => {
                let text = p
                    .decision
                    .options
                    .iter()
                    .find(|o| o.n == p.n)
                    .map_or("", |o| o.text.as_str());
                format!("{}  {text}", p.n)
            }
        };
        let left = (p.due - s.now).clamp(0, UNDO_MS);
        let filled = ((left * 10 + UNDO_MS - 1) / UNDO_MS) as usize;
        let bar = format!(
            "{}{} {}s",
            "█".repeat(filled),
            "░".repeat(10 - filled),
            (left + 999) / 1000
        );
        return Some(Line::from(vec![
            Span::styled(
                format!(" ✓ {} → {choice}", p.decision.id),
                bold(theme::GREEN),
            ),
            Span::styled("   u undo · n note  ", Style::new().fg(theme::MUTED)),
            Span::styled(bar, Style::new().fg(theme::GREEN)),
        ]));
    }
    s.toast
        .as_ref()
        .map(|(m, _)| Line::styled(format!(" {m}"), Style::new().fg(theme::YELLOW)))
}

fn deck_draw(f: &mut Frame, s: &State, deck: Rect, drawn: &mut Drawn) {
    let ahead = s.queue.len().saturating_sub(s.cur + 1);
    let n_edges = ahead.min(MAX_EDGES) as u16;
    let card = Rect {
        height: deck.height.saturating_sub(n_edges),
        ..deck
    };
    drawn.areas.card = card;
    drawn.areas.current = s.current().map(|d| d.id.clone());
    for i in 1..=n_edges {
        let d = &s.queue[s.cur + usize::from(i)];
        let rect = Rect {
            x: deck.x + i,
            y: card.bottom() + i - 1,
            width: deck.width.saturating_sub(2 * i),
            height: 1,
        };
        let more = (i == n_edges && ahead > MAX_EDGES).then(|| ahead - MAX_EDGES);
        edge(f, s, d, rect, more);
        drawn.areas.edges.push((d.id.clone(), rect));
    }
    f.render_widget(Clear, card);
    f.render_widget(Block::new().style(Style::new().bg(theme::BG)), card);
    if let Some(d) = s.current() {
        let (rect, side) = flipping(s, card);
        match side {
            Side::Front => front(f, s, d, rect),
            Side::Back => drawn.scroll_max = back(f, s, d, rect),
        }
    }
    if let Some((d, until)) = &s.ghost
        && s.now < *until
    {
        f.render_widget(Clear, card);
        front(f, s, d, card);
    }
}

fn edge(f: &mut Frame, s: &State, d: &Decision, rect: Rect, more: Option<usize>) {
    let age = s.age_ms(d);
    let mut label = format!(" {} · {} ", d.id, age_label(age));
    if let Some(m) = more {
        label.push_str(&format!("· +{m} more "));
    }
    let fill = usize::from(rect.width).saturating_sub(label.chars().count() + 3);
    let line = format!("╰{}{label}─╯", "─".repeat(fill));
    f.render_widget(
        Paragraph::new(line).style(Style::new().fg(theme::band_color(band(age)))),
        rect,
    );
}

fn card_block(color: Color) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(color))
        .padding(Padding::new(1, 1, 1, 0))
        .style(Style::new().bg(theme::BG))
}

fn front(f: &mut Frame, s: &State, d: &Decision, rect: Rect) {
    let age = s.age_ms(d);
    let mut title = vec![
        Span::raw(" "),
        Span::styled(
            d.id.clone(),
            Style::new().fg(theme::TEXT).add_modifier(Modifier::BOLD),
        ),
    ];
    for part in [
        d.source.clone(),
        d.key.to_lowercase(),
        d.mr_short.clone(),
        d.blocks.clone(),
    ] {
        if !part.is_empty() {
            title.push(Span::styled(
                format!(" · {part}"),
                Style::new().fg(theme::MUTED),
            ));
        }
    }
    title.push(Span::raw(" "));
    if d.question_back {
        title.push(Span::styled(
            "↩ reply ",
            Style::new().fg(theme::GREEN).add_modifier(Modifier::BOLD),
        ));
    }
    let block = card_block(theme::band_color(band(age)))
        .title(Line::from(title))
        .title_top(Line::from(format!(" ⏱ {} ", age_label(age))).right_aligned());
    let mut lines = vec![
        Line::styled(
            d.question.clone(),
            Style::new().fg(theme::TEXT).add_modifier(Modifier::BOLD),
        ),
        Line::default(),
    ];
    if let Some(note) = s.notes.get(&d.id) {
        lines.push(Line::styled(
            format!("✎ note: {note}"),
            Style::new().fg(theme::LAVENDER),
        ));
        lines.push(Line::default());
    }
    if let Some(fl) = s.failed.get(&d.id) {
        let what = fl
            .custom
            .as_ref()
            .map_or_else(|| fl.n.to_string(), |text| format!("✎ {text}"));
        lines.push(Line::styled(
            format!("send failed: {} · ⏎ retries {what}", fl.error),
            Style::new().fg(theme::RED),
        ));
        lines.push(Line::default());
    }
    if let Some(af) = s.ask_failed.get(&d.id) {
        lines.push(Line::styled(
            format!("ask failed: {} · a retries", af.error),
            Style::new().fg(theme::RED),
        ));
        lines.push(Line::default());
    }
    for (i, o) in d.options.iter().enumerate() {
        let mut style = if o.recommended {
            Style::new()
                .fg(theme::LAVENDER)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme::TEXT)
        };
        if s.cursor == Some(i) {
            style = style.bg(theme::SELECT);
        }
        let star = if o.recommended { "★" } else { " " };
        lines.push(Line::styled(format!("{star} {}  {}", o.n, o.text), style));
    }
    let slot = d.options.len() + 1;
    let number = if slot <= 9 {
        slot.to_string()
    } else {
        " ".into()
    };
    let mut style = Style::new().fg(theme::MUTED);
    if s.cursor == Some(d.options.len()) {
        style = style.fg(theme::TEXT).bg(theme::SELECT);
    }
    lines.push(Line::styled(
        format!("✎ {number}  Something else… tell the agent"),
        style,
    ));
    lines.push(Line::default());
    lines.push(Line::styled(
        format!(
            "space: context · {} lines · discussion · {}",
            d.context.lines().count(),
            d.discussion.len()
        ),
        Style::new().fg(theme::MUTED),
    ));
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(block),
        rect,
    );
}

/// Draws the back; returns how far it can scroll.
fn back(f: &mut Frame, s: &State, d: &Decision, rect: Rect) -> u16 {
    let width = rect.width.saturating_sub(4);
    let mut lines = markdown::render(&d.context, width);
    lines.push(Line::default());
    lines.push(Line::styled(
        format!("── discussion · {} ──", d.discussion.len()),
        Style::new().fg(theme::MUTED),
    ));
    for e in &d.discussion {
        lines.push(Line::from(vec![
            Span::styled(
                e.who.clone(),
                Style::new().fg(theme::BLUE).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!(" · {}", e.at), Style::new().fg(theme::MUTED)),
        ]));
        lines.extend(markdown::render(&e.text, width));
    }
    let total = lines.len() as u16;
    let max = total.saturating_sub(rect.height.saturating_sub(2));
    let top = s.scroll.min(max);
    let block = card_block(theme::band_color(band(s.age_ms(d))))
        .padding(Padding::horizontal(1))
        .title(
            Line::from(format!(" {} · context ", d.id))
                .style(Style::new().fg(theme::TEXT).add_modifier(Modifier::BOLD)),
        )
        .title_top(Line::from(format!(" {}/{} ", top + 1, total.max(1))).right_aligned());
    f.render_widget(Paragraph::new(lines).scroll((top, 0)).block(block), rect);
    max
}

fn flipping(s: &State, card: Rect) -> (Rect, Side) {
    let Some(t) = s.flip else {
        return (card, s.side);
    };
    let p = ((s.now - t) as f32 / FLIP_MS as f32).clamp(0.0, 1.0);
    let w = ((f32::from(card.width) * (1.0 - 2.0 * p).abs()).round() as u16)
        .clamp(2, card.width.max(2));
    let side = match (p < 0.5, s.side) {
        (true, Side::Front) => Side::Back,
        (true, Side::Back) => Side::Front,
        (false, side) => side,
    };
    (
        Rect {
            x: card.x + (card.width.saturating_sub(w)) / 2,
            width: w,
            ..card
        },
        side,
    )
}

fn empty(f: &mut Frame, deck: Rect) -> Rect {
    let (w, h) = (deck.width.min(44), deck.height.min(4));
    let r = Rect {
        x: deck.x + (deck.width - w) / 2,
        y: deck.y + (deck.height - h) / 2,
        width: w,
        height: h,
    };
    let text = vec![
        Line::styled(
            "nothing waiting",
            Style::new().fg(theme::TEXT).add_modifier(Modifier::BOLD),
        ),
        Line::styled("new decisions appear here", Style::new().fg(theme::MUTED)),
    ];
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::MUTED));
    f.render_widget(
        Paragraph::new(text)
            .alignment(Alignment::Center)
            .block(block),
        r,
    );
    r
}

fn help(f: &mut Frame, area: Rect) {
    let (w, h) = (52.min(area.width), (KEYS.len() as u16 + 2).min(area.height));
    let r = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    f.render_widget(Clear, r);
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(k, what)| {
            Line::from(vec![
                Span::styled(
                    format!(" {k:<9}"),
                    Style::new()
                        .fg(theme::LAVENDER)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(*what, Style::new().fg(theme::TEXT)),
            ])
        })
        .collect();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme::LAVENDER))
        .title(" keys ")
        .style(Style::new().bg(theme::BG));
    f.render_widget(Paragraph::new(lines).block(block), r);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{self, Event, Key, State};
    use crate::queue::{Decision, decode};
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

    const T0: i64 = 1_792_483_448_000;
    const MIN: i64 = 60_000;

    fn fixture() -> Vec<Decision> {
        decode(include_str!("../tests/fixtures/list.json")).unwrap()
    }

    fn many(n: usize) -> Vec<Decision> {
        (1..=n)
            .map(|i| {
                let mut d = fixture()[usize::from(i > 1)].clone();
                d.id = format!("D-{i}");
                d
            })
            .collect()
    }

    fn state(list: Vec<Decision>, now: i64) -> State {
        let mut s = State::new(now);
        app::update(&mut s, Event::Loaded(Ok(list)), now);
        s.anims.clear();
        s
    }

    fn render(s: &State, w: u16, h: u16) -> (Buffer, Drawn) {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        let mut out = None;
        t.draw(|f| out = Some(draw(f, s))).unwrap();
        (t.backend().buffer().clone(), out.unwrap())
    }

    fn rows(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn screen(buf: &Buffer) -> String {
        rows(buf).join("\n")
    }

    fn find(buf: &Buffer, needle: &str) -> (u16, u16) {
        for (y, row) in rows(buf).iter().enumerate() {
            if let Some(byte) = row.find(needle) {
                return (row[..byte].chars().count() as u16, y as u16);
            }
        }
        panic!("{needle:?} not on screen:\n{}", screen(buf));
    }

    #[test]
    fn the_front_shows_header_card_title_question_and_options() {
        let s = state(fixture(), T0 + 13 * MIN);
        let all = screen(&render(&s, 80, 24).0);
        for want in [
            "decisions · 2 waiting · oldest 13m",
            "D-1 · acme-1001 · backend!870 · reply",
            "⏱ 13m",
            "Push the fix and post the reply?",
            "★ 1  Push and post",
            "  2  Push only",
            "space: context · 10 lines · discussion · 0",
            "J/K cards · j/k options · 1-9 answer · ⏎ pick · space flip · ? help",
        ] {
            assert!(all.contains(want), "{want:?} missing:\n{all}");
        }
    }

    #[test]
    fn the_star_is_lavender_and_the_cursor_row_is_highlighted() {
        let mut s = state(fixture(), T0);
        s.cursor = Some(1);
        let (buf, _) = render(&s, 80, 24);
        let (x, y) = find(&buf, "★ 1  Push and post");
        assert_eq!(buf[(x, y)].fg, theme::LAVENDER);
        assert!(buf[(x, y)].modifier.contains(Modifier::BOLD));
        let (x, y) = find(&buf, "2  Push only");
        assert_eq!(buf[(x, y)].bg, theme::SELECT);
    }

    #[test]
    fn the_card_border_takes_the_colour_of_its_age() {
        for (mins, colour) in [
            (5, theme::BLUE),
            (20, theme::YELLOW),
            (120, theme::ORANGE),
            (240, theme::RED),
        ] {
            let s = state(fixture(), T0 + mins * MIN);
            let (buf, d) = render(&s, 80, 24);
            let c = d.areas.card;
            assert_eq!(buf[(c.x, c.y)].symbol(), "╭");
            assert_eq!(buf[(c.x, c.y)].fg, colour, "{mins}m");
        }
    }

    #[test]
    fn the_stack_shows_the_next_cards_and_how_many_more() {
        let s = state(many(7), T0 + 3 * MIN);
        let (buf, d) = render(&s, 80, 30);
        let ids: Vec<&str> = d.areas.edges.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["D-2", "D-3", "D-4", "D-5"]);
        assert_eq!(d.areas.current.as_deref(), Some("D-1"));
        let all = screen(&buf);
        assert!(all.contains("D-2 · 3m"), "{all}");
        assert!(all.contains("D-5 · 3m · +2 more"), "{all}");
        assert!(!all.contains("D-6"), "{all}");
    }

    #[test]
    fn the_back_shows_context_and_discussion() {
        let mut s = state(fixture(), T0);
        app::update(&mut s, Event::Key(Key::Char('J')), T0);
        app::update(&mut s, Event::Key(Key::Space), T0);
        s.flip = None;
        let all = screen(&render(&s, 60, 16).0);
        for want in [
            "D-2 · context",
            "Short context.",
            "── discussion · 2 ──",
            "you · 11:04",
            "Which QA run?",
            "1/",
            "j/k scroll",
        ] {
            assert!(all.contains(want), "{want:?} missing:\n{all}");
        }
    }

    #[test]
    fn a_long_back_reports_how_far_it_scrolls_and_clamps() {
        let mut s = state(fixture(), T0);
        s.side = Side::Back;
        let (_, d) = render(&s, 60, 14);
        assert!(d.scroll_max > 0);
        s.scroll = 200;
        let (buf, again) = render(&s, 60, 14);
        assert_eq!(again.scroll_max, d.scroll_max);
        assert!(screen(&buf).contains(&format!("{}/", d.scroll_max + 1)));
    }

    #[test]
    fn badges_for_a_question_back_and_a_failed_send() {
        let mut list = fixture();
        list[0].question_back = true;
        let mut s = state(list, T0);
        s.failed.insert(
            "D-1".into(),
            app::Failed {
                n: 2,
                note: None,
                custom: None,
                error: "boom".into(),
            },
        );
        let all = screen(&render(&s, 80, 24).0);
        assert!(all.contains("↩ reply"), "{all}");
        assert!(all.contains("send failed: boom · ⏎ retries 2"), "{all}");
    }

    #[test]
    fn the_status_line_shows_the_undo_bar_then_the_note_input() {
        let mut s = state(fixture(), T0);
        app::update(&mut s, Event::Key(Key::Char('2')), T0);
        app::update(&mut s, Event::Tick, T0 + 1_500);
        let all = screen(&render(&s, 80, 24).0);
        for want in ["✓ D-1 → 2  Push only", "u undo · n note", "4s"] {
            assert!(all.contains(want), "{want:?} missing:\n{all}");
        }
        app::update(&mut s, Event::Key(Key::Char('n')), T0 + 1_500);
        app::update(&mut s, Event::Key(Key::Char('h')), T0 + 1_500);
        assert!(screen(&render(&s, 80, 24).0).contains("note › h"));
    }

    #[test]
    fn the_ask_input_names_the_card() {
        let mut s = state(fixture(), T0);
        app::update(&mut s, Event::Key(Key::Char('a')), T0);
        assert!(screen(&render(&s, 80, 24).0).contains("ask D-1 › "));
    }

    #[test]
    fn a_read_error_shows_under_the_header() {
        let mut s = state(fixture(), T0);
        s.error = Some("decide not runnable".into());
        assert!(
            screen(&render(&s, 80, 24).0).contains("decide failed: decide not runnable · r retry")
        );
    }

    #[test]
    fn an_empty_queue_shows_a_calm_card() {
        let s = state(vec![], T0);
        let (buf, d) = render(&s, 80, 24);
        let all = screen(&buf);
        assert!(
            all.contains("nothing waiting") && all.contains("new decisions appear here"),
            "{all}"
        );
        assert!(all.contains("decisions · nothing waiting"), "{all}");
        assert!(d.areas.empty.is_some());
    }

    #[test]
    fn too_small_asks_for_a_bigger_window() {
        let s = state(fixture(), T0);
        assert!(screen(&render(&s, 40, 10).0).contains("make the window larger (50×14)"));
    }

    #[test]
    fn help_lists_the_keys() {
        let mut s = state(fixture(), T0);
        s.help = true;
        let all = screen(&render(&s, 80, 24).0);
        assert!(
            all.contains("undo the last answer") && all.contains("ask the session back"),
            "{all}"
        );
    }

    #[test]
    fn a_flip_squashes_the_card_then_opens_the_other_side() {
        let mut s = state(fixture(), T0);
        app::update(&mut s, Event::Key(Key::Space), T0);
        s.now = T0 + app::FLIP_MS / 2;
        let (buf, d) = render(&s, 80, 24);
        let c = d.areas.card;
        let row: String = (c.x..c.right()).map(|x| buf[(x, c.y)].symbol()).collect();
        assert_eq!(row.trim().chars().count(), 2, "{row:?}");
        s.now = T0 + app::FLIP_MS * 5 / 6;
        let all = screen(&render(&s, 80, 24).0);
        assert!(all.contains("D-1 · context"), "{all}");
    }

    #[test]
    fn a_thrown_card_stays_drawn_until_its_ghost_expires() {
        let mut s = state(fixture(), T0);
        app::update(&mut s, Event::Key(Key::Char('1')), T0);
        assert!(screen(&render(&s, 80, 24).0).contains("Push the fix and post the reply?"));
        app::update(&mut s, Event::Tick, T0 + app::GHOST_MS);
        let all = screen(&render(&s, 80, 24).0);
        assert!(!all.contains("Push the fix"), "{all}");
        assert!(all.contains("Merge dashboard!940 now?"), "{all}");
    }

    #[test]
    fn a_card_without_options_still_renders() {
        let mut list = fixture();
        list[0].options.clear();
        list[0].recommended = None;
        let s = state(list, T0);
        assert!(screen(&render(&s, 80, 24).0).contains("Push the fix and post the reply?"));
    }

    #[test]
    fn many_cards_in_the_minimum_window_do_not_panic() {
        let mut s = state(many(30), T0);
        s.error = Some("x".into());
        app::update(&mut s, Event::Key(Key::Char('1')), T0);
        render(&s, 50, 14);
    }

    #[test]
    fn before_the_first_load_it_says_loading_not_nothing_waiting() {
        let s = State::new(T0);
        let (buf, d) = render(&s, 80, 24);
        let all = screen(&buf);
        assert!(all.contains("loading…"), "{all}");
        assert!(!all.contains("nothing waiting"), "{all}");
        assert!(d.areas.empty.is_none());
    }

    #[test]
    fn a_failed_ask_shows_on_its_card() {
        let mut s = state(fixture(), T0);
        s.ask_failed.insert(
            "D-1".into(),
            app::AskFailed {
                text: "why".into(),
                error: "boom".into(),
            },
        );
        let all = screen(&render(&s, 80, 24).0);
        assert!(all.contains("ask failed: boom · a retries"), "{all}");
    }

    #[test]
    fn quitting_says_what_it_waits_for() {
        let mut s = state(fixture(), T0);
        app::update(&mut s, Event::Key(Key::Char('1')), T0);
        app::update(&mut s, Event::Key(Key::Char('q')), T0);
        let all = screen(&render(&s, 80, 24).0);
        assert!(all.contains("sending… · q again to quit anyway"), "{all}");
    }

    #[test]
    fn the_header_carries_the_configured_title() {
        let mut s = state(fixture(), T0);
        s.title = "frictions".into();
        let all = screen(&render(&s, 80, 24).0);
        assert!(all.contains(" frictions · 2 waiting"), "{all}");
        s.demo = true;
        let all = screen(&render(&s, 80, 24).0);
        assert!(all.contains(" frictions · demo · 2 waiting"), "{all}");
    }

    #[test]
    fn the_demo_says_so_in_the_header() {
        let mut s = state(fixture(), T0);
        s.demo = true;
        let all = screen(&render(&s, 80, 24).0);
        assert!(all.contains("decisions · demo · 2 waiting"), "{all}");
    }

    #[test]
    fn the_tell_slot_is_the_last_row_and_takes_the_cursor() {
        let mut s = state(fixture(), T0);
        let all = screen(&render(&s, 80, 24).0);
        assert!(all.contains("✎ 4  Something else… tell the agent"), "{all}");
        s.cursor = Some(3);
        let (buf, _) = render(&s, 80, 24);
        let (x, y) = find(&buf, "✎ 4  Something else");
        assert_eq!(buf[(x, y)].bg, theme::SELECT);
    }

    #[test]
    fn a_card_note_shows_under_the_question() {
        let mut s = state(fixture(), T0);
        s.notes.insert("D-1".into(), "use staging".into());
        assert!(screen(&render(&s, 80, 24).0).contains("✎ note: use staging"));
    }

    #[test]
    fn the_tell_and_card_note_lines_name_the_card() {
        let mut s = state(fixture(), T0);
        app::update(&mut s, Event::Key(Key::Char('4')), T0);
        assert!(screen(&render(&s, 80, 24).0).contains("tell D-1 › "));
        let mut s = state(fixture(), T0);
        app::update(&mut s, Event::Key(Key::Char('n')), T0);
        assert!(screen(&render(&s, 80, 24).0).contains("note for D-1 › "));
    }

    #[test]
    fn the_undo_bar_shows_an_answer_in_your_own_words() {
        let mut s = state(fixture(), T0);
        app::update(&mut s, Event::Key(Key::Char('4')), T0);
        for c in "close it".chars() {
            app::update(
                &mut s,
                Event::Key(if c == ' ' { Key::Space } else { Key::Char(c) }),
                T0,
            );
        }
        app::update(&mut s, Event::Key(Key::Enter), T0);
        assert!(screen(&render(&s, 80, 24).0).contains("✓ D-1 → ✎ close it"));
    }
    #[test]
    fn the_card_names_who_asked() {
        let mut list = fixture();
        list[0].source = "claude-code · repo".into();
        let all = screen(&render(&state(list, T0), 100, 24).0);
        assert!(
            all.contains("D-1 · claude-code · repo · acme-1001 · backend!870 · reply"),
            "{all}"
        );
    }
}
