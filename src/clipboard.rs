//! The deck's `c`: the first clipboard tool found gets the text on stdin; with none, an OSC 52
//! escape asks the terminal (tmux with `set-clipboard on` passes it on).

use std::io::{ErrorKind, Write};
use std::process::{Command, Stdio};

const TOOLS: [(&str, &[&str]); 3] = [
    ("pbcopy", &[]),
    ("wl-copy", &[]),
    ("xclip", &["-selection", "clipboard"]),
];

pub fn copy(text: &str) -> Result<(), String> {
    for (tool, args) in TOOLS {
        let mut child = match Command::new(tool)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(format!("{tool}: {e}")),
        };
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|e| format!("{tool}: {e}"))?;
        }
        let status = child.wait().map_err(|e| format!("{tool}: {e}"))?;
        return if status.success() {
            Ok(())
        } else {
            Err(format!("{tool} exited with {status}"))
        };
    }
    std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/tty")
        .and_then(|mut tty| tty.write_all(osc52(text).as_bytes()))
        .map_err(|e| format!("no clipboard tool, and the terminal refused OSC 52: {e}"))
}

pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                ABC[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_encodes_base64() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
        assert_eq!(
            osc52("kubectl get pods"),
            "\x1b]52;c;a3ViZWN0bCBnZXQgcG9kcw==\x07"
        );
        assert_eq!(osc52("abc"), "\x1b]52;c;YWJj\x07");
    }
}
