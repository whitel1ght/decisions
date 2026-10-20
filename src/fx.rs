//! The animations: tachyonfx effects started from the app's `Anim` events and run over the
//! drawn frame. The flip and the thrown card's ghost are drawn by `ui` instead.

use std::time::Duration;

use ratatui::buffer::Buffer;
use ratatui::layout::{Margin, Rect};
use tachyonfx::{CellFilter, Effect, EffectManager, Interpolation, Motion, fx};

use crate::app::{Anim, Band, GHOST_MS};
use crate::theme;
use crate::ui::Areas;

#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum FxKey {
    #[default]
    Card,
    Edge(String),
    Age(String),
}

#[derive(Default)]
pub struct Fx {
    mgr: EffectManager<FxKey>,
    idle: Option<Effect>,
}

fn border() -> CellFilter {
    CellFilter::Outer(Margin::new(1, 1))
}

fn idle() -> Effect {
    fx::repeating(fx::ping_pong(fx::hsl_shift_fg(
        [0.0, 0.0, 12.0],
        (1400, Interpolation::SineInOut),
    )))
    .with_filter(border())
}

impl Fx {
    pub fn start(&mut self, anim: &Anim, areas: &Areas) {
        let card = areas.card;
        let edge = |id: &str| areas.edges.iter().find(|(e, _)| e == id).map(|(_, r)| *r);
        match anim {
            Anim::Deal => {
                self.card(fx::coalesce((250, Interpolation::QuadOut)), card);
                for (i, (id, r)) in areas.edges.iter().enumerate() {
                    let slide = fx::slide_in(
                        Motion::DownToUp,
                        3,
                        0,
                        theme::BG,
                        (220, Interpolation::QuadOut),
                    );
                    self.mgr.add_unique_effect(
                        FxKey::Edge(id.clone()),
                        fx::delay(60 * (i as u32 + 1), slide).with_area(*r),
                    );
                }
            }
            Anim::Next => self.card(
                fx::slide_in(
                    Motion::DownToUp,
                    4,
                    0,
                    theme::BG,
                    (180, Interpolation::QuadOut),
                ),
                card,
            ),
            Anim::Prev => self.card(
                fx::slide_in(
                    Motion::UpToDown,
                    4,
                    0,
                    theme::BG,
                    (180, Interpolation::QuadOut),
                ),
                card,
            ),
            Anim::Throw => self.card(
                fx::sequence(&[
                    fx::dissolve((GHOST_MS as u32, Interpolation::QuadIn)),
                    fx::coalesce((200, Interpolation::QuadOut)),
                ]),
                card,
            ),
            Anim::Leave(_) => self.card(fx::coalesce((200, Interpolation::QuadOut)), card),
            Anim::Arrive(id) => {
                if let Some(r) = edge(id) {
                    let glow = fx::fade_from_fg(theme::LAVENDER, (600, Interpolation::QuadOut));
                    self.mgr
                        .add_unique_effect(FxKey::Edge(id.clone()), glow.with_area(r));
                }
            }
            Anim::AgeBand { id, from, to } => {
                let target = if areas.current.as_deref() == Some(id.as_str()) {
                    Some(card)
                } else {
                    edge(id)
                };
                if let Some(r) = target {
                    let fade =
                        fx::fade_from_fg(theme::band_color(*from), (1000, Interpolation::Linear));
                    let effect = if *to == Band::Stale {
                        fx::sequence(&[
                            fade,
                            fx::ping_pong(fx::hsl_shift_fg(
                                [0.0, 0.0, 20.0],
                                (300, Interpolation::SineInOut),
                            )),
                        ])
                    } else {
                        fade
                    };
                    self.mgr.add_unique_effect(
                        FxKey::Age(id.clone()),
                        effect.with_filter(border()).with_area(r),
                    );
                }
            }
        }
    }

    fn card(&mut self, effect: Effect, area: Rect) {
        self.mgr
            .add_unique_effect(FxKey::Card, effect.with_area(area));
    }

    pub fn process(&mut self, elapsed: Duration, buf: &mut Buffer, areas: &Areas) {
        let area = buf.area;
        self.mgr.process_effects(elapsed.into(), buf, area);
        match areas.empty {
            Some(r) => {
                let _ = self
                    .idle
                    .get_or_insert_with(idle)
                    .process(elapsed.into(), buf, r);
            }
            None => self.idle = None,
        }
    }

    pub fn running(&self) -> bool {
        self.mgr.is_running()
    }

    pub fn finish(&mut self) {
        self.mgr = EffectManager::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{self, Event, State};
    use crate::queue::{Decision, decode};
    use crate::ui;
    use ratatui::{Terminal, backend::TestBackend};

    const T0: i64 = 1_792_483_448_000;

    fn state(n: usize) -> State {
        let base = decode(include_str!("../tests/fixtures/list.json")).unwrap();
        let list: Vec<Decision> = (1..=n)
            .map(|i| {
                let mut d = base[usize::from(i > 1)].clone();
                d.id = format!("D-{i}");
                d
            })
            .collect();
        let mut s = State::new(T0);
        app::update(&mut s, Event::Loaded(Ok(list)), T0);
        s.anims.clear();
        s
    }

    fn symbols(buf: &Buffer) -> String {
        let a = buf.area;
        (0..a.height)
            .map(|y| {
                (0..a.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    /// Draws `s`, starts `anim`, runs 50 ms frames until it is done; returns the last frame,
    /// a plain frame without effects, and how many frames it took.
    fn run(s: &State, anim: &Anim) -> (Buffer, Buffer, u32) {
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut fx = Fx::default();
        let mut areas = Areas::default();
        t.draw(|f| areas = ui::draw(f, s).areas).unwrap();
        fx.start(anim, &areas);
        let mut frames = 0;
        while fx.running() && frames < 100 {
            t.draw(|f| {
                let a = ui::draw(f, s).areas;
                fx.process(Duration::from_millis(50), f.buffer_mut(), &a);
            })
            .unwrap();
            frames += 1;
        }
        let mut plain = Terminal::new(TestBackend::new(80, 24)).unwrap();
        plain
            .draw(|f| {
                ui::draw(f, s);
            })
            .unwrap();
        (
            t.backend().buffer().clone(),
            plain.backend().buffer().clone(),
            frames,
        )
    }

    #[test]
    fn every_effect_runs_and_ends_on_the_plain_frame() {
        let s = state(3);
        let anims = [
            Anim::Deal,
            Anim::Next,
            Anim::Prev,
            Anim::Throw,
            Anim::Leave("D-9".into()),
            Anim::Arrive("D-2".into()),
            Anim::AgeBand {
                id: "D-1".into(),
                from: Band::Fresh,
                to: Band::Warm,
            },
            Anim::AgeBand {
                id: "D-2".into(),
                from: Band::Old,
                to: Band::Stale,
            },
        ];
        for anim in &anims {
            let (animated, plain, frames) = run(&s, anim);
            assert!(frames > 0, "{anim:?} never started");
            assert!(frames < 100, "{anim:?} never finished");
            assert_eq!(symbols(&animated), symbols(&plain), "{anim:?}");
        }
    }

    #[test]
    fn an_arrival_off_screen_starts_nothing() {
        let s = state(2);
        let (_, _, frames) = run(&s, &Anim::Arrive("D-40".into()));
        assert_eq!(frames, 0);
    }

    #[test]
    fn finish_ends_every_effect_at_once() {
        let s = state(3);
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut areas = Areas::default();
        t.draw(|f| areas = ui::draw(f, &s).areas).unwrap();
        let mut fx = Fx::default();
        fx.start(&Anim::Deal, &areas);
        assert!(fx.running());
        fx.finish();
        assert!(!fx.running());
    }

    #[test]
    fn the_idle_shimmer_runs_only_while_the_queue_is_empty() {
        let s = state(0);
        let mut t = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let mut fx = Fx::default();
        t.draw(|f| {
            let a = ui::draw(f, &s).areas;
            fx.process(Duration::from_millis(50), f.buffer_mut(), &a);
        })
        .unwrap();
        assert!(fx.idle.is_some());
        assert!(!fx.running(), "the shimmer does not keep the frame rate up");
        let s = state(1);
        t.draw(|f| {
            let a = ui::draw(f, &s).areas;
            fx.process(Duration::from_millis(50), f.buffer_mut(), &a);
        })
        .unwrap();
        assert!(fx.idle.is_none());
    }

    #[test]
    fn effects_survive_a_smaller_frame() {
        let s = state(5);
        let mut big = Terminal::new(TestBackend::new(120, 40)).unwrap();
        let mut areas = Areas::default();
        big.draw(|f| areas = ui::draw(f, &s).areas).unwrap();
        let mut fx = Fx::default();
        for anim in [Anim::Deal, Anim::Throw, Anim::Arrive("D-3".into())] {
            fx.start(&anim, &areas);
        }
        for (w, h) in [(60, 16), (30, 8)] {
            let mut small = Terminal::new(TestBackend::new(w, h)).unwrap();
            for _ in 0..10 {
                small
                    .draw(|f| {
                        let a = ui::draw(f, &s).areas;
                        fx.process(Duration::from_millis(50), f.buffer_mut(), &a);
                    })
                    .unwrap();
            }
        }
    }
}
