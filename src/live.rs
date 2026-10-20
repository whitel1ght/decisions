//! The deck's backend for the real queue: the store in process, plus the on_answer hook.

use crate::config::Config;
use crate::hook;
use crate::queue::{Backend, Decision};
use crate::store::{Event, Store};

pub struct Live {
    store: Store,
    on_answer: Option<String>,
}

impl Live {
    pub fn new(cfg: &Config) -> Self {
        Self {
            store: cfg.store(),
            on_answer: cfg.on_answer.clone(),
        }
    }

    fn after(&self, r: crate::store::Result<Event>) -> Result<Option<String>, String> {
        let event = r.map_err(|e| e.0)?;
        Ok(hook::run(self.on_answer.as_deref(), &event).err())
    }
}

impl Backend for Live {
    fn list(&self) -> Result<Vec<Decision>, String> {
        Ok(self.store.list().0)
    }

    fn answer(&self, id: &str, n: u8, note: Option<&str>) -> Result<Option<String>, String> {
        self.after(self.store.answer(id, n, note.unwrap_or("")))
    }

    fn ask(&self, id: &str, text: &str) -> Result<Option<String>, String> {
        self.after(self.store.ask(id, text))
    }

    fn tell(&self, id: &str, text: &str) -> Result<Option<String>, String> {
        self.after(self.store.tell(id, text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Mode};
    use crate::store::New;

    fn cfg(hook: Option<&str>) -> Config {
        let dir = std::env::temp_dir().join(format!(
            "decisions-live-{}-{}",
            std::process::id(),
            hook.map_or(0, str::len)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Config {
            dir,
            mode: Mode::Always,
            wait: 50,
            on_answer: hook.map(String::from),
            mr_strip: String::new(),
            session: None,
        }
    }

    #[test]
    fn answers_go_to_the_store_and_the_hook() {
        let c = cfg(Some("exit 0"));
        let id = c
            .store()
            .add(New {
                question: "Ship?".into(),
                options: vec!["yes".into(), "no".into()],
                ..New::default()
            })
            .unwrap();
        let live = Live::new(&c);
        assert_eq!(live.list().unwrap().len(), 1);
        assert_eq!(live.answer(&id, 1, None), Ok(None));
        assert!(live.list().unwrap().is_empty());
    }

    #[test]
    fn a_failing_hook_is_a_warning_not_an_error() {
        let c = cfg(Some("echo broken >&2; exit 9"));
        let id = c
            .store()
            .add(New {
                question: "Ship?".into(),
                options: vec!["yes".into(), "no".into()],
                ..New::default()
            })
            .unwrap();
        let w = Live::new(&c).tell(&id, "close it").unwrap().unwrap();
        assert!(w.contains("broken"), "{w}");
        assert!(c.store().list().0.is_empty(), "the answer stands");
    }
}
