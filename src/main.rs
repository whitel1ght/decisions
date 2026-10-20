//! decisions: the /run-day decision queue as a deck of cards, for tmux prefix y.

use std::env;
use std::path::PathBuf;
use std::process::{ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{self, Event as TermEvent};
use decisions::app::{self, Command, Event, State};
use decisions::demo::Demo;
use decisions::fx::Fx;
use decisions::keys;
use decisions::queue::{Backend, Decide};
use decisions::ui::{self, Areas};
use notify_debouncer_mini::notify::{RecommendedWatcher, RecursiveMode};
use notify_debouncer_mini::{DebounceEventResult, Debouncer, new_debouncer};

const USAGE: &str = "decisions — answer what /run-day is waiting on (tmux prefix y)

  decisions        the queue as a deck of cards; ? lists the keys
  decisions --demo a made-up queue held in memory, to try the screen and the keys;
                   nothing reaches decide or the /run-day session

Reads and answers through `decide` ($DECIDE, else decide on PATH) and watches
$RUN_DAY_STATE_DIR/decisions (default ~/.local/state/run-day/decisions).
An answer waits 5 s for undo; q sends it at once and closes once it has landed
(q again closes without waiting).";

const RELOAD_EVERY: Duration = Duration::from_secs(30);
const DEMO_RELOAD_EVERY: Duration = Duration::from_secs(2);
const SEND_WAIT: Duration = Duration::from_secs(10);

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn decisions_dir() -> PathBuf {
    let state = env::var_os("RUN_DAY_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".local/state/run-day")
        });
    state.join("decisions")
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
            Command::Answer { id, n, note } => {
                self.send(id, false, move |d, id| d.answer(id, n, note.as_deref()))
            }
            Command::Ask { id, text } => self.send(id, true, move |d, id| d.ask(id, &text)),
            Command::Tell { id, text } => self.send(id, false, move |d, id| d.tell(id, &text)),
            Command::OpenUrl(url) => {
                let _ = std::process::Command::new("open")
                    .arg(url)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
            }
            Command::Quit => return true,
        }
        false
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
        call: impl FnOnce(&dyn Backend, &str) -> Result<(), String> + Send + 'static,
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
fn watch(tx: mpsc::Sender<Event>) -> Option<Debouncer<RecommendedWatcher>> {
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
        .watch(&decisions_dir(), RecursiveMode::NonRecursive)
        .ok()?;
    Some(debouncer)
}

fn main() -> ExitCode {
    let demo = match env::args().nth(1).as_deref() {
        Some("-h" | "--help") => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Some("--demo") => true,
        Some(other) => {
            eprintln!("decisions: unknown option {other}");
            return ExitCode::from(2);
        }
        None => false,
    };
    let (backend, reload_every): (Arc<dyn Backend>, Duration) = if demo {
        (Arc::new(Demo::new()), DEMO_RELOAD_EVERY)
    } else {
        (Arc::new(Decide::from_env()), RELOAD_EVERY)
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
    spawn_keys(tx.clone(), stop.clone());
    let _watcher = if demo { None } else { watch(tx.clone()) };

    let mut s = State::new(now_ms());
    s.demo = demo;
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
                Event::Sent { .. } => runner.in_flight -= 1,
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
            Ok(ev @ Event::Sent { .. }) => {
                runner.in_flight -= 1;
                app::update(&mut s, ev, now_ms());
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    ratatui::restore();

    let mut failed: Vec<_> = s.failed.iter().collect();
    failed.sort_by(|a, b| a.0.cmp(b.0));
    for (id, f) in &failed {
        eprintln!("decisions: {id} → {}: {}", f.n, f.error);
    }
    if runner.in_flight > 0 {
        eprintln!(
            "decisions: {} answer(s) still sending after {}s; check `decide list`",
            runner.in_flight,
            SEND_WAIT.as_secs()
        );
    }
    if failed.is_empty() && runner.in_flight == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
