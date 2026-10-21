//! How long since the last keyboard or mouse input, to tell whether the user is away.

use std::process::Command;

pub trait Idle {
    /// Seconds since the last input; None when it cannot be told.
    fn seconds(&mut self) -> Option<u64>;
}

pub struct System;

impl Idle for System {
    fn seconds(&mut self) -> Option<u64> {
        if cfg!(target_os = "macos") {
            let out = Command::new("ioreg")
                .args(["-c", "IOHIDSystem"])
                .output()
                .ok()?;
            parse_ioreg(&String::from_utf8_lossy(&out.stdout))
        } else {
            let out = Command::new("xprintidle").output().ok()?;
            if !out.status.success() {
                return None;
            }
            parse_xprintidle(&String::from_utf8_lossy(&out.stdout))
        }
    }
}

pub fn parse_ioreg(out: &str) -> Option<u64> {
    out.lines()
        .find_map(|l| {
            l.split_once("\"HIDIdleTime\" = ")?
                .1
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map(|ns| ns / 1_000_000_000)
}

pub fn parse_xprintidle(out: &str) -> Option<u64> {
    out.trim().parse::<u64>().ok().map(|ms| ms / 1000)
}

/// Away after `minutes` without input; always away with 0 or when idle time is unknown.
pub fn away(idle_secs: Option<u64>, minutes: u64) -> bool {
    minutes == 0 || idle_secs.is_none_or(|s| s >= minutes * 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_ioreg_nanoseconds() {
        let out = "    | | |   \"HIDIdleTime\" = 288550863125\n    | | |   \"Other\" = 1\n";
        assert_eq!(parse_ioreg(out), Some(288));
        assert_eq!(parse_ioreg("nothing here"), None);
    }

    #[test]
    fn reads_xprintidle_milliseconds() {
        assert_eq!(parse_xprintidle("61500\n"), Some(61));
        assert_eq!(parse_xprintidle("x"), None);
    }

    #[test]
    fn away_after_the_threshold_or_when_unknown_or_always() {
        assert!(!away(Some(299), 5));
        assert!(away(Some(300), 5));
        assert!(away(None, 5));
        assert!(away(Some(0), 0));
    }

    #[test]
    fn the_system_source_does_not_panic() {
        let _ = System.seconds();
    }
}
