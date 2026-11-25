//! Runs a target in a ConPTY and prints the terminal modes its output turns
//! on and off (DEC private modes), then optionally sends a mouse drag.

use crate::opt;
use crate::repaint::Pty;
use anyhow::Result;
use std::time::Duration;

pub fn modes(b: &[u8]) -> Vec<String> {
    let s = String::from_utf8_lossy(b);
    let mut out = Vec::new();
    let mut rest = &s[..];
    while let Some(i) = rest.find("\x1b[?") {
        let t = &rest[i + 3..];
        let end = t.find(|c: char| !(c.is_ascii_digit() || c == ';')).unwrap_or(t.len());
        if let Some(f) = t[end..].chars().next() {
            if f == 'h' || f == 'l' {
                out.push(format!("?{}{f}", &t[..end]));
            }
        }
        rest = &t[end..];
    }
    out
}

pub fn run(rest: &[String]) -> Result<()> {
    let target = opt(rest, "--target").expect("--target");
    println!("conpty: {}", crate::conpty::which());
    let mut p = Pty::spawn(&target, 100, 30, 0)?;
    p.settle(Duration::from_millis(2000), Duration::from_secs(15));
    let out = p.out.lock().unwrap().clone();
    println!("modes: {:?}", modes(&out));
    if let Some(send) = opt(rest, "--send") {
        p.send(&crate::unescape(&send))?;
        p.settle(Duration::from_millis(1500), Duration::from_secs(15));
        crate::repaint::show("after send", &p.screen(0));
    }
    p.con.kill();
    Ok(())
}
