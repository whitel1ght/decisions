//! Where the store is and how the MCP server behaves: the config file, then the environment.

use std::path::PathBuf;

use serde::Deserialize;

use crate::store::Store;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Always,
    OnRequest,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub dir: PathBuf,
    pub mode: Mode,
    pub wait: u64,
    pub on_answer: Option<String>,
    pub mr_strip: String,
    pub session: Option<String>,
}

#[derive(Deserialize, Default)]
struct FileConfig {
    mode: Option<String>,
    wait: Option<u64>,
    on_answer: Option<String>,
    mr_strip_prefix: Option<String>,
}

fn mode(v: &str, from: &str) -> Result<Mode, String> {
    match v {
        "always" => Ok(Mode::Always),
        "on-request" => Ok(Mode::OnRequest),
        other => Err(format!(
            "{from}: mode must be \"always\" or \"on-request\", not {other:?}"
        )),
    }
}

impl Config {
    pub fn store(&self) -> Store {
        Store::new(self.dir.clone(), self.mr_strip.clone())
    }
}

pub fn load() -> Result<Config, String> {
    load_from(&|k| std::env::var(k).ok())
}

pub fn load_from(env: &dyn Fn(&str) -> Option<String>) -> Result<Config, String> {
    let home = PathBuf::from(env("HOME").unwrap_or_default());
    let xdg = |var: &str, fallback: &str| {
        env(var)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(fallback))
    };
    let path = xdg("XDG_CONFIG_HOME", ".config").join("decisions/config.toml");
    let file: FileConfig = match std::fs::read_to_string(&path) {
        Ok(text) => toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(_) => FileConfig::default(),
    };
    let mut c = Config {
        dir: xdg("XDG_STATE_HOME", ".local/state").join("decisions"),
        mode: match &file.mode {
            Some(m) => mode(m, &path.display().to_string())?,
            None => Mode::Always,
        },
        wait: file.wait.unwrap_or(50),
        on_answer: file.on_answer.filter(|s| !s.trim().is_empty()),
        mr_strip: file.mr_strip_prefix.unwrap_or_default(),
        session: None,
    };
    if let Some(d) = env("DECISIONS_DIR").filter(|v| !v.is_empty()) {
        c.dir = PathBuf::from(d);
    }
    if let Some(m) = env("DECISIONS_MODE") {
        c.mode = mode(&m, "DECISIONS_MODE")?;
    }
    if let Some(w) = env("DECISIONS_WAIT") {
        c.wait = w
            .trim()
            .parse()
            .map_err(|_| format!("DECISIONS_WAIT: not a number of seconds: {w:?}"))?;
    }
    if let Some(s) = env("DECISIONS_SESSION").filter(|v| !v.trim().is_empty()) {
        c.session = Some(s);
    }
    if let Some(h) = env("DECISIONS_ON_ANSWER") {
        c.on_answer = Some(h).filter(|s| !s.trim().is_empty());
    }
    if let Some(p) = env("DECISIONS_MR_STRIP") {
        c.mr_strip = p;
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn with(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| m.get(k).cloned()
    }

    fn home() -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("decisions-config-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn defaults_follow_xdg_under_home() {
        let h = home();
        let c = load_from(&with(&[("HOME", h.to_str().unwrap())])).unwrap();
        assert_eq!(c.dir, h.join(".local/state/decisions"));
        assert_eq!(
            (
                c.mode,
                c.wait,
                c.on_answer.as_deref(),
                c.mr_strip.as_str(),
                c.session.as_deref()
            ),
            (Mode::Always, 50, None, "", None)
        );
    }

    #[test]
    fn the_file_then_the_environment() {
        let h = home();
        std::fs::create_dir_all(h.join(".config/decisions")).unwrap();
        std::fs::write(h.join(".config/decisions/config.toml"), "mode = \"on-request\"\nwait = 0\non_answer = \"cat >> /tmp/x\"\nmr_strip_prefix = \"acme-\"\n").unwrap();
        let c = load_from(&with(&[("HOME", h.to_str().unwrap())])).unwrap();
        assert_eq!(
            (c.mode, c.wait, c.on_answer.as_deref(), c.mr_strip.as_str()),
            (Mode::OnRequest, 0, Some("cat >> /tmp/x"), "acme-")
        );
        let c = load_from(&with(&[
            ("HOME", h.to_str().unwrap()),
            ("DECISIONS_DIR", "/q"),
            ("DECISIONS_MODE", "always"),
            ("DECISIONS_WAIT", "7"),
            ("DECISIONS_SESSION", "s1"),
            ("DECISIONS_ON_ANSWER", "true"),
            ("DECISIONS_MR_STRIP", ""),
        ]))
        .unwrap();
        assert_eq!(
            (
                c.dir.to_str().unwrap(),
                c.mode,
                c.wait,
                c.session.as_deref(),
                c.on_answer.as_deref(),
                c.mr_strip.as_str()
            ),
            ("/q", Mode::Always, 7, Some("s1"), Some("true"), "")
        );
    }

    #[test]
    fn xdg_dirs_win_over_home() {
        let h = home();
        let c = load_from(&with(&[
            ("HOME", h.to_str().unwrap()),
            ("XDG_STATE_HOME", "/st"),
        ]))
        .unwrap();
        assert_eq!(c.dir, std::path::PathBuf::from("/st/decisions"));
    }

    #[test]
    fn bad_values_are_errors_naming_the_source() {
        let h = home();
        assert!(
            load_from(&with(&[
                ("HOME", h.to_str().unwrap()),
                ("DECISIONS_MODE", "sometimes")
            ]))
            .unwrap_err()
            .contains("DECISIONS_MODE")
        );
        assert!(
            load_from(&with(&[
                ("HOME", h.to_str().unwrap()),
                ("DECISIONS_WAIT", "soon")
            ]))
            .unwrap_err()
            .contains("DECISIONS_WAIT")
        );
        std::fs::create_dir_all(h.join(".config/decisions")).unwrap();
        std::fs::write(h.join(".config/decisions/config.toml"), "wait = \"x\"\n").unwrap();
        assert!(
            load_from(&with(&[("HOME", h.to_str().unwrap())]))
                .unwrap_err()
                .contains("config.toml")
        );
    }
}
