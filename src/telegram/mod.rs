//! `decisions telegram`: the open decisions in the user's Telegram chat while they are away.

pub mod api;
pub mod bot;
#[cfg(test)]
pub mod fake;
pub mod render;
pub mod reply;
pub mod service;
pub mod state;

use std::io::Write;
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use clap::Subcommand;

use crate::config::{Config, Telegram};
use crate::idle::{Idle, System};
use crate::telegram::api::{Api, Http};

#[derive(Subcommand)]
pub enum TgCmd {
    /// The service: sends cards while you're away and takes answers from the chat
    Run,
    /// Link your private Telegram chat to the bot
    Pair,
    /// Send cards to Telegram while you're away
    On,
    /// Stop sending new cards (cards already sent still take answers)
    Off,
    /// On or off, paired, running, away, what was sent, the last error
    Status,
    /// Print a launchd agent (macOS) or systemd user unit (Linux) that runs the service
    Setup,
}

fn refused(e: impl std::fmt::Display) -> ExitCode {
    eprintln!("decisions: {e}");
    ExitCode::from(1)
}

/// The bot token: `DECISIONS_TELEGRAM_TOKEN`, else what `token_cmd` prints. Errors never
/// include the command's output.
pub fn token(t: &Telegram) -> Result<String, String> {
    if let Some(tok) = &t.token {
        return Ok(tok.clone());
    }
    let Some(cmd) = &t.token_cmd else {
        return Err(
            "no bot token: set DECISIONS_TELEGRAM_TOKEN, or token_cmd under [telegram] in config.toml"
                .into(),
        );
    };
    let out = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("token_cmd did not start: {e}"))?;
    let tok = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || tok.is_empty() {
        return Err(format!(
            "token_cmd failed ({}) or printed nothing",
            out.status
        ));
    }
    Ok(tok)
}

/// The variables the service needs from this shell: PATH and the DECISIONS_ ones, never the token.
fn passed_on(vars: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = vars
        .into_iter()
        .filter(|(k, _)| {
            (k == "PATH" || k.starts_with("DECISIONS_")) && k != "DECISIONS_TELEGRAM_TOKEN"
        })
        .collect();
    v.sort();
    v
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// A launchd agent (macOS) or systemd user unit (Linux) running `<exe> telegram run`.
pub fn unit(macos: bool, exe: &str, home: &str, env: &[(String, String)]) -> String {
    if macos {
        let vars: String = env
            .iter()
            .map(|(k, v)| format!("    <key>{}</key><string>{}</string>\n", xml(k), xml(v)))
            .collect();
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!-- Save as ~/Library/LaunchAgents/decisions.telegram.plist, then run:
     launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/decisions.telegram.plist -->
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>decisions.telegram</string>
  <key>ProgramArguments</key>
  <array><string>{exe}</string><string>telegram</string><string>run</string></array>
  <key>EnvironmentVariables</key>
  <dict>
{vars}  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>30</integer>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
            exe = xml(exe),
            log = xml(&format!("{home}/Library/Logs/decisions-telegram.log")),
        )
    } else {
        let vars: String = env
            .iter()
            .map(|(k, v)| format!("Environment={}\n", quoted(&format!("{k}={v}"))))
            .collect();
        format!(
            "# Save as ~/.config/systemd/user/decisions-telegram.service, then run:\n\
             #   systemctl --user daemon-reload && systemctl --user enable --now decisions-telegram\n\
             [Unit]\nDescription=decisions: Telegram bot\n\n\
             [Service]\nExecStart={} telegram run\n{vars}Restart=always\nRestartSec=30\n\n\
             [Install]\nWantedBy=default.target\n",
            quoted(exe)
        )
    }
}

fn now_ms() -> i64 {
    chrono::Local::now().timestamp_millis()
}

fn serve(cfg: &Config) -> ExitCode {
    let st = match state::load(&cfg.dir) {
        Ok(s) => s,
        Err(e) => return refused(e),
    };
    if st.chat_id.is_none() {
        return refused("not paired: run `decisions telegram pair` first");
    }
    if cfg.telegram.token.is_none() && cfg.telegram.token_cmd.is_none() {
        return refused(token(&cfg.telegram).unwrap_err());
    }
    let _lock = match state::run_lock(&cfg.dir) {
        Ok(Some(l)) => l,
        Ok(None) => return refused("another `decisions telegram run` or `pair` is running"),
        Err(e) => return refused(e),
    };
    let stop = Arc::new(AtomicBool::new(false));
    for sig in [signal_hook::consts::SIGTERM, signal_hook::consts::SIGINT] {
        let _ = signal_hook::flag::register(sig, stop.clone());
    }
    let first = token(&cfg.telegram);
    let connected = first.is_ok();
    if let Err(e) = &first {
        eprintln!("telegram: {e}");
    }
    let api = Http::new(&cfg.telegram.api, first.as_deref().unwrap_or(""));
    let t = cfg.telegram.clone();
    eprintln!("telegram: running");
    service::serve(
        bot::Bot::new(api, cfg),
        connected,
        || token(&t).map(|tok| Http::new(&t.api, &tok)),
        &stop,
        &mut |s| std::thread::sleep(Duration::from_secs(s)),
    );
    ExitCode::SUCCESS
}

fn pair(cfg: &Config) -> ExitCode {
    let tok = match token(&cfg.telegram) {
        Ok(t) => t,
        Err(e) => return refused(e),
    };
    let mut api = Http::new(&cfg.telegram.api, &tok);
    let name = match api.get_me() {
        Ok(n) => n,
        Err(e) => return refused(format!("the bot token does not work: {e}")),
    };
    let _lock = match state::run_lock(&cfg.dir) {
        Ok(Some(l)) => l,
        Ok(None) => {
            return refused(
                "the service is running: stop it first (see `decisions telegram setup`), then pair",
            );
        }
        Err(e) => return refused(e),
    };
    let code = service::code();
    println!("Send /start {code} to @{name} within 5 minutes.");
    let _ = std::io::stdout().flush();
    match service::pair(
        &mut api,
        &cfg.dir,
        &code,
        now_ms() + 300_000,
        &now_ms,
        &mut |s| std::thread::sleep(Duration::from_secs(s)),
    ) {
        Ok(_) => {
            println!(
                "Paired. Start the service: `decisions telegram run`, or install it with `decisions telegram setup`."
            );
            ExitCode::SUCCESS
        }
        Err(e) => refused(e),
    }
}

fn status(cfg: &Config) -> ExitCode {
    let st = match state::load(&cfg.dir) {
        Ok(s) => s,
        Err(e) => return refused(e),
    };
    let running = matches!(state::run_lock(&cfg.dir), Ok(None));
    let open = cfg.store().list().0;
    println!(
        "{}",
        bot::summary(&st, &open, System.seconds(), cfg.telegram.idle_minutes)
    );
    println!(
        "chat: {}",
        if st.chat_id.is_some() {
            "paired"
        } else {
            "not paired (decisions telegram pair)"
        }
    );
    println!(
        "service: {}",
        if running { "running" } else { "not running" }
    );
    if let Some(e) = &st.last_error {
        println!("last error: {e}");
    }
    ExitCode::SUCCESS
}

pub fn run(cmd: TgCmd, cfg: &Config) -> ExitCode {
    match cmd {
        TgCmd::Run => serve(cfg),
        TgCmd::Pair => pair(cfg),
        TgCmd::On | TgCmd::Off => {
            let on = matches!(cmd, TgCmd::On);
            match state::update(&cfg.dir, |s| s.enabled = on) {
                Ok(st) => {
                    println!("telegram: {}", if on { "on" } else { "off" });
                    if st.chat_id.is_none() {
                        eprintln!("decisions: not paired yet: run `decisions telegram pair`");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => refused(e),
            }
        }
        TgCmd::Status => status(cfg),
        TgCmd::Setup => {
            let exe = std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| "decisions".into());
            let home = std::env::var("HOME").unwrap_or_default();
            print!(
                "{}",
                unit(
                    cfg!(target_os = "macos"),
                    &exe,
                    &home,
                    &passed_on(std::env::vars())
                )
            );
            ExitCode::SUCCESS
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Telegram;

    #[test]
    fn the_token_comes_from_the_environment_first_then_the_command() {
        let t = Telegram {
            token: Some("env".into()),
            token_cmd: Some("echo cmd".into()),
            ..Telegram::default()
        };
        assert_eq!(token(&t).unwrap(), "env");
        let t = Telegram {
            token_cmd: Some("printf ' tok \\n'".into()),
            ..Telegram::default()
        };
        assert_eq!(token(&t).unwrap(), "tok");
        let t = Telegram {
            token_cmd: Some("printf SECRET; exit 1".into()),
            ..Telegram::default()
        };
        let e = token(&t).unwrap_err();
        assert!(!e.contains("SECRET"), "{e}");
        let t = Telegram {
            token_cmd: Some("true".into()),
            ..Telegram::default()
        };
        assert!(token(&t).is_err(), "prints nothing");
        assert!(
            token(&Telegram::default())
                .unwrap_err()
                .contains("DECISIONS_TELEGRAM_TOKEN")
        );
    }

    #[test]
    fn a_launchd_agent() {
        let env = vec![("DECISIONS_DIR".to_string(), "/a&b".to_string())];
        let u = unit(true, "/bin/decisions", "/Users/me", &env);
        assert!(
            u.contains(
                "<string>/bin/decisions</string><string>telegram</string><string>run</string>"
            ),
            "{u}"
        );
        assert!(
            u.contains("<key>DECISIONS_DIR</key><string>/a&amp;b</string>"),
            "{u}"
        );
        assert!(u.contains("/Users/me/Library/Logs/decisions-telegram.log"));
        assert!(u.contains("<key>KeepAlive</key><true/>"));
        let comment = u.split("<!--").nth(1).unwrap().split("-->").next().unwrap();
        assert!(!comment.contains("--"), "XML comments cannot hold --");
    }

    #[test]
    fn a_systemd_unit() {
        let env = vec![("DECISIONS_DIR".to_string(), "/a b".to_string())];
        let u = unit(false, "/opt/my bin/decisions", "/home/me", &env);
        assert!(
            u.contains("ExecStart=\"/opt/my bin/decisions\" telegram run"),
            "{u}"
        );
        assert!(u.contains("Environment=\"DECISIONS_DIR=/a b\""), "{u}");
        assert!(u.contains("Restart=always"));
    }

    #[test]
    fn the_environment_passed_on_never_has_the_token() {
        let vars = vec![
            ("DECISIONS_TELEGRAM_TOKEN".to_string(), "x".to_string()),
            ("DECISIONS_DIR".to_string(), "/d".to_string()),
            ("PATH".to_string(), "/bin".to_string()),
            ("HOME".to_string(), "/h".to_string()),
        ];
        assert_eq!(
            passed_on(vars),
            vec![
                ("DECISIONS_DIR".to_string(), "/d".to_string()),
                ("PATH".to_string(), "/bin".to_string())
            ]
        );
    }

    #[test]
    fn the_command_line() {
        use clap::Parser;
        for args in [
            ["decisions", "telegram", "run"],
            ["decisions", "telegram", "status"],
            ["decisions", "telegram", "off"],
        ] {
            assert!(crate::cli::Cli::try_parse_from(args).is_ok(), "{args:?}");
        }
        assert!(crate::cli::Cli::try_parse_from(["decisions", "telegram", "nope"]).is_err());
    }
}
