//! Check 3: after a resize, does ConPTY repaint enough that a client that
//! saw none of the earlier output ends up with the same screen?
//!
//! "truth" is fed every byte, resized at the moment each resize was issued.
//! "client" is a blank terminal at the final size fed only what came after.

use crate::conpty::{self, Con};
use crate::{opt, unescape};
use anyhow::Result;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Pty {
    pub con: Arc<Con>,
    pub out: Arc<Mutex<Vec<u8>>>,
    last: Arc<Mutex<Instant>>,
    /// (byte offset, cols, rows) for every size, starting with the initial one.
    pub sizes: Vec<(usize, u16, u16)>,
}

impl Pty {
    pub fn spawn(cmdline: &str, cols: u16, rows: u16, flags: u32) -> Result<Pty> {
        let con = Arc::new(Con::spawn(cmdline, cols, rows, flags)?);
        let mut reader = con.output.try_clone()?;
        let out = Arc::new(Mutex::new(Vec::new()));
        let last = Arc::new(Mutex::new(Instant::now()));
        let (o, l) = (out.clone(), last.clone());
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                o.lock().unwrap().extend_from_slice(&buf[..n]);
                *l.lock().unwrap() = Instant::now();
            }
        });
        Ok(Pty { con, out, last, sizes: vec![(0, cols, rows)] })
    }

    /// Waits until there has been no output for `quiet`, up to `max`.
    pub fn settle(&self, quiet: Duration, max: Duration) {
        let start = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(50));
            let idle = self.last.lock().unwrap().elapsed();
            if (idle >= quiet && start.elapsed() >= quiet) || start.elapsed() >= max {
                return;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.out.lock().unwrap().len()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let at = self.len();
        self.con.resize(cols, rows)?;
        self.sizes.push((at, cols, rows));
        Ok(())
    }

    pub fn send(&mut self, b: &[u8]) -> Result<()> {
        (&self.con.input).write_all(b)?;
        Ok(())
    }

    /// Screen as seen by a client that has watched from byte `from` onwards.
    pub fn screen(&self, from: usize) -> vt100::Parser {
        let out = self.out.lock().unwrap();
        let (_, c0, r0) = *self.sizes.iter().rev().find(|s| s.0 <= from).unwrap_or(&self.sizes[0]);
        let mut p = vt100::Parser::new(r0, c0, 0);
        let mut pos = from;
        for &(at, c, r) in self.sizes.iter().filter(|s| s.0 > from) {
            p.process(&out[pos..at]);
            p.set_size(r, c);
            pos = at;
        }
        p.process(&out[pos..]);
        p
    }
}

pub fn rows_of(p: &vt100::Parser) -> Vec<String> {
    let s = p.screen();
    let (_, cols) = s.size();
    s.rows(0, cols).map(|r| r.trim_end().to_string()).collect()
}

/// Cells whose text, colours or attributes differ.
pub fn cell_diffs(a: &vt100::Parser, b: &vt100::Parser) -> usize {
    let (sa, sb) = (a.screen(), b.screen());
    let (rows, cols) = sa.size();
    let mut n = 0;
    for r in 0..rows {
        for c in 0..cols {
            let (x, y) = (sa.cell(r, c), sb.cell(r, c));
            let same = match (x, y) {
                (Some(x), Some(y)) => {
                    x.contents() == y.contents()
                        && x.fgcolor() == y.fgcolor()
                        && x.bgcolor() == y.bgcolor()
                        && x.bold() == y.bold()
                        && x.italic() == y.italic()
                        && x.underline() == y.underline()
                        && x.inverse() == y.inverse()
                }
                (None, None) => true,
                _ => false,
            };
            if !same {
                n += 1;
            }
        }
    }
    n
}

pub fn show(title: &str, p: &vt100::Parser) {
    println!("--- {title} (cursor {:?}) ---", p.screen().cursor_position());
    for (i, r) in rows_of(p).iter().enumerate() {
        println!("{i:2}|{r}");
    }
}

pub fn run(rest: &[String]) -> Result<()> {
    let cmdline = opt(rest, "--cmd").unwrap_or_else(|| "pwsh -NoLogo -NoProfile".into());
    let keys = opt(rest, "--keys");
    let mode = opt(rest, "--mode").unwrap_or_else(|| "nudge:30".into());
    let start_wait: u64 = opt(rest, "--wait").and_then(|v| v.parse().ok()).unwrap_or(3000);
    let flags = conpty::parse_flags(opt(rest, "--flags"), 0);
    let t0 = Instant::now();
    let mut pty = Pty::spawn(&cmdline, 120, 30, flags)?;
    let mut first_ms = None;
    while t0.elapsed() < Duration::from_secs(15) {
        if pty.len() > 0 && first_ms.is_none() {
            first_ms = Some(t0.elapsed().as_millis());
        }
        if rows_of(&pty.screen(0)).iter().any(|r| r.contains('>')) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    println!("first_output_ms={first_ms:?} prompt_ms={}", t0.elapsed().as_millis());
    pty.settle(Duration::from_millis(start_wait), Duration::from_secs(30));
    if let Some(k) = keys {
        for part in k.split("||") {
            pty.send(&unescape(part))?;
            pty.settle(Duration::from_millis(1500), Duration::from_secs(20));
        }
    }
    let mark = pty.len();
    let before = pty.screen(0);
    match mode.split_once(':') {
        Some(("nudge", gap)) => {
            pty.resize(120, 31)?;
            std::thread::sleep(Duration::from_millis(gap.parse().unwrap_or(30)));
            pty.resize(120, 30)?;
        }
        Some(("size", wh)) => {
            let (c, r) = wh.split_once('x').expect("size:COLSxROWS");
            pty.resize(c.parse()?, r.parse()?)?;
        }
        _ => anyhow::bail!("--mode nudge:<ms> | size:<cols>x<rows>"),
    }
    pty.settle(Duration::from_millis(1500), Duration::from_secs(10));
    let truth = pty.screen(0);
    let client = pty.screen(mark);
    let (t, c) = (rows_of(&truth), rows_of(&client));
    let diff: Vec<usize> = (0..t.len().max(c.len())).filter(|&i| t.get(i) != c.get(i)).collect();
    let cells = cell_diffs(&truth, &client);
    let cursor_ok = truth.screen().cursor_position() == client.screen().cursor_position();
    let nonblank = t.iter().filter(|r| !r.is_empty()).count();
    println!(
        "conpty={} flags={flags} cmd={cmdline:?} mode={mode} total_bytes={mark} repaint_bytes={} rows_differing={} cells_differing={cells} cursor_match={cursor_ok} nonblank_rows={nonblank}",
        conpty::which(),
        pty.len() - mark,
        diff.len()
    );
    if rest.iter().any(|a| a == "--show") || !diff.is_empty() || !cursor_ok {
        show("before resize", &before);
        show("truth", &truth);
        show("client (post-resize bytes only)", &client);
    }
    println!("{}", if diff.is_empty() && cells == 0 && cursor_ok { "PASS" } else { "FAIL" });
    pty.con.kill();
    std::process::exit(0)
}
