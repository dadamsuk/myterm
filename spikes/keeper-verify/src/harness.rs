//! Check 4: run `ssh -t <host> <exe> attach` inside a local ConPTY (standing in
//! for myterm's xterm.js) against a keeper running pwsh, type at it, resize it,
//! and read the screen back.

use crate::opt;
use crate::repaint::{rows_of, show, Pty};
use anyhow::Result;
use std::time::Duration;

fn wait(p: &Pty) {
    p.settle(Duration::from_millis(1200), Duration::from_secs(15));
}

fn check(name: &str, ok: bool, results: &mut Vec<(String, bool)>) {
    println!("{} {name}", if ok { "PASS" } else { "FAIL" });
    results.push((name.into(), ok));
}

pub fn run(rest: &[String]) -> Result<()> {
    let host = opt(rest, "--host").unwrap_or_else(|| "localhost".into());
    let exe = opt(rest, "--exe").unwrap_or_else(|| std::env::current_exe().unwrap().display().to_string());
    let ssh_opts = opt(rest, "--ssh-opts").unwrap_or_default();
    let target = opt(rest, "--target").unwrap_or_else(|| format!("ssh -t {ssh_opts} {host} {exe} attach"));
    println!("target: {target}");
    let mut p = Pty::spawn(&target, 100, 30, crate::conpty::parse_flags(opt(rest, "--flags"), 0))?;
    p.settle(Duration::from_millis(2500), Duration::from_secs(20));
    show("after attach", &p.screen(0));
    let mut results = Vec::new();
    if let Some(script) = opt(rest, "--script") {
        for line in std::fs::read_to_string(script)?.lines() {
            p.send(format!("{line}\r").as_bytes())?;
            p.settle(Duration::from_millis(2500), Duration::from_secs(30));
        }
        show("script screen", &p.screen(0));
        p.con.kill();
        std::process::exit(0);
    }

    // UTF-8 round trip, both directions.
    let text = "héllo ✓ 日本 🙂";
    p.send(format!("echo '{text}'\r").as_bytes())?;
    wait(&p);
    let rows = rows_of(&p.screen(0));
    check("utf-8 echoed and printed", rows.iter().any(|r| r.trim() == text), &mut results);

    // Cursor keys and history: Up recalls the last line.
    p.send(b"\x1b[A")?;
    wait(&p);
    let rows = rows_of(&p.screen(0));
    let recalled = rows.iter().rev().find(|r| !r.is_empty()).map(|r| r.contains("echo 'héllo")).unwrap_or(false);
    check("up arrow recalls history", recalled, &mut results);

    // Ctrl+C reaches the shell as a key (PSReadLine cancels the line).
    p.send(b"\x03")?;
    wait(&p);
    p.send(b"'after-ctrl-c'\r")?;
    wait(&p);
    let rows = rows_of(&p.screen(0));
    check("ctrl+c then new command runs", rows.iter().any(|r| r.trim() == "after-ctrl-c"), &mut results);

    // Tab completion.
    p.send(b"Get-ChildI\t")?;
    wait(&p);
    let rows = rows_of(&p.screen(0));
    check("tab completes", rows.iter().any(|r| r.contains("Get-ChildItem")), &mut results);
    p.send(b"\x1b")?; // clear the line (PSReadLine Escape = RevertLine)
    std::thread::sleep(Duration::from_millis(300));
    p.send(b"\x03")?;
    wait(&p);

    // Resize propagates: local pty -> sshd ConPTY -> attach -> keeper ConPTY -> pwsh.
    for (c, r) in [(90u16, 35u16), (132, 40)] {
        p.resize(c, r)?;
        wait(&p);
        p.send(b"\"size=$($Host.UI.RawUI.WindowSize.Width)x$($Host.UI.RawUI.WindowSize.Height)\"\r")?;
        wait(&p);
        let rows = rows_of(&p.screen(0));
        let want = format!("size={c}x{r}");
        let got = rows.iter().rev().find(|r| r.starts_with("size=")).cloned().unwrap_or_default();
        check(&format!("resize to {c}x{r} reaches pwsh (got {got:?})"), got.trim() == want, &mut results);
    }

    // A wide full-screen-ish redraw: long line wrapping at the new width.
    p.send(format!("'{}'\r", "x".repeat(200)).as_bytes())?;
    wait(&p);

    show("final screen", &p.screen(0));
    p.con.kill();
    let failed = results.iter().filter(|r| !r.1).count();
    println!("{} of {} passed", results.len() - failed, results.len());
    std::process::exit(if failed == 0 { 0 } else { 1 })
}
