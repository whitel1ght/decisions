//! decisions: the /run-day decision queue as a deck of cards, for tmux prefix y.

use std::path::PathBuf;
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::app::{self, Command, Event, State};
use crate::demo::Demo;
use crate::fx::Fx;
use crate::keys;
use crate::queue::Backend;
use crate::ui::{self, Areas};
use crossterm::event::{self, DisableBracketedPaste, EnableBracketedPaste, Event as TermEvent};
use crossterm::execute;
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

const RELOAD_EVERY: Duration = Duration::from_secs(30);
const DEMO_RELOAD_EVERY: Duration = Duration::from_secs(2);
/// Opens a link in the browser.
pub const OPENER: &str = if cfg!(target_os = "macos") {
    "open"
} else {
    "xdg-open"
};
/// The stats screen looks back this many days.
const STATS_DAYS: u32 = 14;
const SEND_WAIT: Duration = Duration::from_secs(10);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Runs the app's commands: `decide` calls on threads, results back as events.
struct Runner {
    decide: Arc<dyn Backend>,
    tx: mpsc::Sender<Event>,
    in_flight: usize,
    loading: bool,
    reload_again: bool,
}

impl Runner {
    /// Returns true for Quit.
    fn run(&mut self, cmd: Command) -> bool {
        match cmd {
            Command::Reload => self.reload(),
            Command::LoadStats => {
                let (decide, tx) = (self.decide.clone(), self.tx.clone());
                thread::spawn(move || {
                    let _ = tx.send(Event::StatsLoaded(decide.stats(STATS_DAYS).map(Box::new)));
                });
            }
            Command::Answer { id, n, note } => {
                self.send(id, false, move |d, id| d.answer(id, n, note.as_deref()))
            }
            Command::Ask { id, text } => self.send(id, true, move |d, id| d.ask(id, &text)),
            Command::Tell { id, text } => self.send(id, false, move |d, id| d.tell(id, &text)),
            Command::Seen { id } => self.send(id, false, |d, id| d.seen(id)),
            Command::LoadSessions => self.background(|d| Event::SessionsLoaded {
                sessions: d.sessions(),
                last: d.last_target(),
            }),
            Command::LoadFiles(cwd) => self.background(move |d| Event::FilesLoaded {
                files: d.files(&cwd),
                cwd,
            }),
            Command::SaveTarget(owner) => self.decide.save_target(&owner),
            Command::PasteImage => self.background(|d| Event::ImagePasted(d.paste_image(now_ms()))),
            Command::SendPrompt {
                session,
                source,
                text,
            } => {
                self.in_flight += 1;
                self.background(move |d| Event::PromptSent(d.prompt(&session, &source, &text)));
            }
            Command::OpenUrl(url) => {
                let _ = std::process::Command::new(OPENER)
                    .arg(url)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
            Command::Quit => return true,
        }
        false
    }

    /// Runs `call` on a thread; its event comes back through the loop.
    fn background(&self, call: impl FnOnce(&dyn Backend) -> Event + Send + 'static) {
        let (decide, tx) = (self.decide.clone(), self.tx.clone());
        thread::spawn(move || {
            let _ = tx.send(call(&*decide));
        });
    }

    fn reload(&mut self) {
        if self.loading {
            self.reload_again = true;
            return;
        }
        self.loading = true;
        let (decide, tx) = (self.decide.clone(), self.tx.clone());
        thread::spawn(move || {
            let _ = tx.send(Event::Loaded(decide.list()));
        });
    }

    fn loaded(&mut self) {
        self.loading = false;
        if std::mem::take(&mut self.reload_again) {
            self.reload();
        }
    }

    fn send(
        &mut self,
        id: String,
        ask: bool,
        call: impl FnOnce(&dyn Backend, &str) -> Result<Option<String>, String> + Send + 'static,
    ) {
        self.in_flight += 1;
        let (decide, tx) = (self.decide.clone(), self.tx.clone());
        thread::spawn(move || {
            let result = call(&*decide, &id);
            let _ = tx.send(Event::Sent { id, ask, result });
        });
    }
}

fn spawn_keys(tx: mpsc::Sender<Event>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if !event::poll(Duration::from_millis(100)).unwrap_or(false) {
                continue;
            }
            let sent = match event::read() {
                Ok(TermEvent::Key(k)) => keys::map(k).map_or(Ok(()), |k| tx.send(Event::Key(k))),
                Ok(TermEvent::Paste(text)) => tx.send(Event::Paste(text)),
                Ok(TermEvent::Resize(..)) => tx.send(Event::Tick),
                Ok(_) => Ok(()),
                Err(_) => break,
            };
            if sent.is_err() {
                break;
            }
        }
    });
}

/// Reload when anything in the decisions folder changes. None when it cannot be watched
/// (missing folder): the 30 s re-read still picks changes up.
fn watch(dir: PathBuf, tx: mpsc::Sender<Event>) -> Option<Debouncer<RecommendedWatcher>> {
    let mut debouncer = new_debouncer(
        Duration::from_millis(150),
        move |res: DebounceEventResult| {
            if res.is_ok() {
                let _ = tx.send(Event::Changed);
            }
        },
    )
    .ok()?;
    debouncer
        .watcher()
        .watch(&dir, RecursiveMode::NonRecursive)
        .ok()?;
    Some(debouncer)
}

pub fn run(cfg: &crate::config::Config, demo: bool, prompt: bool) -> ExitCode {
    let (backend, reload_every): (Arc<dyn Backend>, Duration) = if demo {
        (Arc::new(Demo::new(cfg.icons)), DEMO_RELOAD_EVERY)
    } else {
        (Arc::new(crate::live::Live::new(cfg)), RELOAD_EVERY)
    };

    let (tx, rx) = mpsc::channel::<Event>();
    let signalled = Arc::new(AtomicBool::new(false));
    for sig in [
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
        signal_hook::consts::SIGINT,
    ] {
        let _ = signal_hook::flag::register(sig, signalled.clone());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut terminal = ratatui::init();
    // A paste comes in one piece, so a dragged file's path can attach the image it names.
    let _ = execute!(std::io::stdout(), EnableBracketedPaste);
    spawn_keys(tx.clone(), stop.clone());
    let _watcher = if demo {
        None
    } else {
        // Watch even before the first question: a missing folder could not be watched.
        let _ = std::fs::create_dir_all(&cfg.dir);
        watch(cfg.dir.clone(), tx.clone())
    };

    let mut s = State::new(now_ms());
    s.demo = demo;
    s.title = cfg.title.clone();
    s.icons = cfg.icons;
    let mut fx = Fx::default();
    let mut areas = Areas::default();
    let mut runner = Runner {
        decide: backend,
        tx,
        in_flight: 0,
        loading: false,
        reload_again: false,
    };
    runner.reload();
    if prompt {
        let mut first = Vec::new();
        app::open_prompt(&mut s, &mut first);
        for cmd in first {
            runner.run(cmd);
        }
    }

    let mut last_frame = Instant::now();
    let mut next_tick = Instant::now() + Duration::from_secs(1);
    let mut next_reload = Instant::now() + reload_every;
    'main: loop {
        let elapsed = last_frame.elapsed();
        last_frame = Instant::now();
        let _ = terminal.draw(|f| {
            let drawn = ui::draw(f, &s);
            s.scroll_max = drawn.scroll_max;
            areas = drawn.areas;
            fx.process(elapsed, f.buffer_mut(), &areas);
        });
        for anim in s.anims.drain(..) {
            fx.start(&anim, &areas);
        }

        let frame = if fx.running() || s.animating() {
            Duration::from_millis(16)
        } else if areas.empty.is_some() {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(1)
        };
        let wait = frame.min(next_tick.saturating_duration_since(Instant::now()));
        let mut events = Vec::new();
        match rx.recv_timeout(wait) {
            Ok(ev) => events.push(ev),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if signalled.swap(false, Ordering::Relaxed) {
            events.push(Event::Signal);
        }
        let due = Instant::now() >= next_tick;
        if due {
            next_tick = Instant::now() + Duration::from_secs(1);
        }
        if due || s.animating() {
            events.push(Event::Tick);
        }
        if Instant::now() >= next_reload {
            next_reload = Instant::now() + reload_every;
            runner.reload();
        }
        for ev in events {
            match &ev {
                Event::Key(_) => fx.finish(),
                Event::Loaded(_) => runner.loaded(),
                Event::Sent { .. } | Event::PromptSent(_) => runner.in_flight -= 1,
                _ => {}
            }
            for cmd in app::update(&mut s, ev, now_ms()) {
                if runner.run(cmd) {
                    break 'main;
                }
            }
        }
    }

    stop.store(true, Ordering::Relaxed);
    let deadline = Instant::now() + SEND_WAIT;
    while runner.in_flight > 0 {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(ev @ (Event::Sent { .. } | Event::PromptSent(_))) => {
                runner.in_flight -= 1;
                app::update(&mut s, ev, now_ms());
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = execute!(std::io::stdout(), DisableBracketedPaste);
    ratatui::restore();

    let mut failed: Vec<_> = s.failed.iter().collect();
    failed.sort_by(|a, b| a.0.cmp(b.0));
    for (id, f) in &failed {
        eprintln!("decisions: {id} → {}: {}", f.n, f.error);
    }
    for (text, error) in &s.prompt.lost {
        eprintln!("decisions: message not sent ({error}): {text}");
    }
    if runner.in_flight > 0 {
        eprintln!(
            "decisions: {} answer(s) still sending after {}s; check `decide list`",
            runner.in_flight,
            SEND_WAIT.as_secs()
        );
    }
    if failed.is_empty() && s.prompt.lost.is_empty() && runner.in_flight == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_opener_fits_the_platform() {
        let want = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        assert_eq!(super::OPENER, want);
    }
}
