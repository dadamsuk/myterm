//! Throwaway checks for the Windows keeper design (SPEC.md 14.1, "To verify first").
//!
//!   serve    minimal keeper: per-user pipe with a SID-only DACL, one ConPTY session
//!   ping     connect to the pipe, print both ends' identity        (check 2)
//!   attach   console relay between stdio and the pipe              (check 4, run under ssh -t)
//!   repaint  does a ConPTY resize repaint the whole screen?        (check 3)
//!   harness  drive `ssh -t localhost <exe> attach` in a local ConPTY (check 4)
//!   whoami   print this process's identity

mod conpty;
mod ctrlc;
mod harness;
mod raw;
mod repaint;
mod win;

use anyhow::{anyhow, bail, Context, Result};
use conpty::Con;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient, ServerOptions};
use tokio::sync::broadcast;

pub fn opt(rest: &[String], name: &str) -> Option<String> {
    rest.iter().position(|a| a == name).and_then(|i| rest.get(i + 1)).cloned()
}

pub fn has(rest: &[String], name: &str) -> bool {
    rest.iter().any(|a| a == name)
}

/// "\r", "\n", "\t", "\xNN" and "\e" escapes in command-line key strings.
pub fn unescape(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            match b[i + 1] {
                b'r' => { out.push(b'\r'); i += 2; continue }
                b'n' => { out.push(b'\n'); i += 2; continue }
                b't' => { out.push(b'\t'); i += 2; continue }
                b'e' => { out.push(0x1b); i += 2; continue }
                b'x' if i + 3 < b.len() + 0 => {
                    if let Ok(v) = u8::from_str_radix(&s[i + 2..i + 4], 16) {
                        out.push(v);
                        i += 4;
                        continue;
                    }
                }
                _ => {}
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

fn default_pipe() -> String {
    format!(r"\\.\pipe\myterm-spike-{}", win::user_sid())
}

fn state_dir() -> std::path::PathBuf {
    let d = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".into())).join("myterm-spike");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().cloned().unwrap_or_default();
    let rest = if args.is_empty() { &args[..] } else { &args[1..] };
    let r = match cmd.as_str() {
        "serve" => serve(rest),
        "ping" => ping(rest),
        "attach" => attach(rest),
        "repaint" => repaint::run(rest),
        "harness" => harness::run(rest),
        "ctrlc" => ctrlc::run(rest),
        "raw" => raw::run(rest),
        "whoami" => {
            println!("{}", serde_json::to_string_pretty(&win::identity())?);
            Ok(())
        }
        _ => {
            eprintln!("usage: keeper-verify serve|ping|attach|repaint|harness|whoami [options]");
            std::process::exit(2)
        }
    };
    if let Err(e) = &r {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
    Ok(())
}

// ---------------------------------------------------------------- keeper

struct Session {
    con: Arc<Con>,
    out: broadcast::Sender<Vec<u8>>,
    size: Mutex<(u16, u16)>,
    child_pid: Option<u32>,
    exited: Mutex<Option<u32>>,
    trace: bool,
    nudge_gap_ms: u64,
}

impl Session {
    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.con.resize(cols, rows)?;
        *self.size.lock().unwrap() = (cols, rows);
        Ok(())
    }

    /// Forces ConPTY to repaint: resize to the client's size, or nudge by a row and back.
    fn repaint(&self, cols: u16, rows: u16) -> Result<()> {
        if *self.size.lock().unwrap() == (cols, rows) {
            self.resize(cols, rows + 1)?;
            std::thread::sleep(Duration::from_millis(self.nudge_gap_ms));
        }
        self.resize(cols, rows)
    }
}

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

fn log(msg: impl AsRef<str>) {
    let line = format!("[{}] {}\n", std::process::id(), msg.as_ref());
    eprint!("{line}");
    if let Some(f) = LOG.lock().unwrap().as_mut() {
        let _ = f.write_all(line.as_bytes());
    }
}

fn serve(rest: &[String]) -> Result<()> {
    let pipe = opt(rest, "--pipe").unwrap_or_else(default_pipe);
    let cmdline = opt(rest, "--cmd").unwrap_or_else(|| "pwsh -NoLogo -NoProfile".into());
    if has(rest, "--inbox") {
        std::env::set_var("KV_INBOX", "1");
    }
    *LOG.lock().unwrap() = std::fs::File::create(state_dir().join(format!("serve-{}.log", std::process::id()))).ok();

    // DACL: the owning user only. Optional explicit medium mandatory label, so a
    // keeper started from an elevated (high integrity) ssh logon can still be
    // opened by the medium-integrity desktop UI.
    let mut sddl = format!("D:P(A;;GA;;;{})", win::user_sid());
    if has(rest, "--medium-label") {
        sddl.push_str("S:(ML;;NW;;;ME)");
    }
    let sa = win::security_attributes(&sddl)?;
    log(format!("identity {}", win::identity()));
    log(format!("pipe {pipe} sddl {sddl}"));

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async move {
        // First instance doubles as the single-instance lock (pipe names are machine-global).
        let first = unsafe {
            ServerOptions::new()
                .first_pipe_instance(true)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(&pipe, sa as *mut _)
        };
        let mut server = match first {
            Ok(s) => s,
            Err(e) => {
                log(format!("cannot create pipe (keeper already running?): {e}"));
                std::process::exit(3);
            }
        };

        let flags = conpty::parse_flags(opt(rest, "--flags"), 0);
        let con = Arc::new(Con::spawn(&cmdline, 120, 30, flags)?);
        log(format!("conpty {} flags {flags}", conpty::which()));
        let (tx, _) = broadcast::channel::<Vec<u8>>(4096);
        let session = Arc::new(Session {
            child_pid: Some(con.pid),
            con: con.clone(),
            out: tx.clone(),
            size: Mutex::new((120, 30)),
            exited: Mutex::new(None),
            trace: has(rest, "--trace-input"),
            nudge_gap_ms: opt(rest, "--nudge-gap").and_then(|v| v.parse().ok()).unwrap_or(30),
        });
        let mut reader = con.output.try_clone()?;
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = tx.send(buf[..n].to_vec());
            }
        });
        let ws = session.clone();
        std::thread::spawn(move || {
            let code = ws.con.wait();
            log(format!("child exited {code}"));
            *ws.exited.lock().unwrap() = Some(code);
        });
        log(format!("session child pid {:?} cmd {cmdline}", session.child_pid));
        std::fs::write(
            state_dir().join("keeper.json"),
            json!({ "pipe": pipe, "pid": std::process::id(), "exe": std::env::current_exe()? }).to_string(),
        )?;

        loop {
            server.connect().await?;
            let conn = server;
            server = unsafe {
                ServerOptions::new().reject_remote_clients(true).create_with_security_attributes_raw(&pipe, sa as *mut _)?
            };
            let s = session.clone();
            tokio::spawn(async move {
                if let Err(e) = handle(conn, s).await {
                    log(format!("connection ended: {e:#}"));
                }
            });
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    })
}

async fn handle(conn: tokio::net::windows::named_pipe::NamedPipeServer, s: Arc<Session>) -> Result<()> {
    let (rd, mut wr) = tokio::io::split(conn);
    let mut rd = BufReader::new(rd);
    let mut line = String::new();
    rd.read_line(&mut line).await?;
    let req: Value = serde_json::from_str(&line).context("request")?;
    log(format!("request {}", line.trim()));
    match req["op"].as_str() {
        Some("info") => {
            let (cols, rows) = *s.size.lock().unwrap();
            let reply = json!({
                "keeper": win::identity(),
                "session": { "child_pid": s.child_pid, "cols": cols, "rows": rows, "exited": *s.exited.lock().unwrap() },
            });
            wr.write_all(format!("{reply}\n").as_bytes()).await?;
        }
        Some("attach") => {
            let cols = req["cols"].as_u64().unwrap_or(80) as u16;
            let rows = req["rows"].as_u64().unwrap_or(24) as u16;
            let mut sub = s.out.subscribe();
            s.repaint(cols, rows)?;
            let out = async {
                loop {
                    match sub.recv().await {
                        Ok(b) => wr.write_all(&b).await?,
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(_) => break,
                    }
                }
                Ok::<_, anyhow::Error>(())
            };
            let inp = async {
                let mut hdr = [0u8; 5];
                loop {
                    rd.read_exact(&mut hdr).await?;
                    let len = u32::from_le_bytes(hdr[1..5].try_into().unwrap()) as usize;
                    let mut payload = vec![0u8; len];
                    rd.read_exact(&mut payload).await?;
                    match hdr[0] {
                        0 => {
                            if s.trace {
                                log(format!("input {:?}", String::from_utf8_lossy(&payload)));
                            }
                            (&s.con.input).write_all(&payload)?;
                        }
                        1 if len == 4 => {
                            let cols = u16::from_le_bytes([payload[0], payload[1]]);
                            let rows = u16::from_le_bytes([payload[2], payload[3]]);
                            log(format!("resize {cols}x{rows}"));
                            s.resize(cols, rows)?;
                        }
                        t => bail!("bad frame type {t}"),
                    }
                }
                #[allow(unreachable_code)]
                Ok::<_, anyhow::Error>(())
            };
            tokio::select! { r = out => r?, r = inp => r? }
        }
        other => bail!("unknown op {other:?}"),
    }
    Ok(())
}

// ---------------------------------------------------------------- clients

async fn connect(pipe: &str) -> Result<NamedPipeClient> {
    for _ in 0..100 {
        match ClientOptions::new().open(pipe) {
            Ok(c) => return Ok(c),
            Err(e) if e.raw_os_error() == Some(231) => tokio::time::sleep(Duration::from_millis(50)).await,
            Err(e) => return Err(anyhow!("open {pipe}: {e}")),
        }
    }
    bail!("pipe busy")
}

fn ping(rest: &[String]) -> Result<()> {
    let pipe = opt(rest, "--pipe").unwrap_or_else(default_pipe);
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let mut c = connect(&pipe).await?;
        c.write_all(format!("{}\n", json!({ "op": "info", "client": win::identity() })).as_bytes()).await?;
        let mut line = String::new();
        BufReader::new(c).read_line(&mut line).await?;
        let reply: Value = serde_json::from_str(&line)?;
        println!("{}", serde_json::to_string_pretty(&json!({ "client": win::identity(), "reply": reply }))?);
        Ok(())
    })
}

mod console {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::WriteFile;
    use windows_sys::Win32::System::Console::*;

    pub fn stdin() -> HANDLE { unsafe { GetStdHandle(STD_INPUT_HANDLE) } }
    pub fn stdout() -> HANDLE { unsafe { GetStdHandle(STD_OUTPUT_HANDLE) } }

    /// Raw VT input, VT output, UTF-8 both ways. Returns the old modes.
    pub fn raw() -> (u32, u32) {
        unsafe {
            SetConsoleCP(65001);
            SetConsoleOutputCP(65001);
            let (mut i, mut o) = (0u32, 0u32);
            GetConsoleMode(stdin(), &mut i);
            GetConsoleMode(stdout(), &mut o);
            SetConsoleMode(
                stdin(),
                (i & !(ENABLE_PROCESSED_INPUT | ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT | ENABLE_QUICK_EDIT_MODE))
                    | ENABLE_VIRTUAL_TERMINAL_INPUT
                    | ENABLE_WINDOW_INPUT
                    | ENABLE_EXTENDED_FLAGS,
            );
            SetConsoleMode(stdout(), o | ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN);
            (i, o)
        }
    }

    pub fn restore((i, o): (u32, u32)) {
        unsafe {
            SetConsoleMode(stdin(), i);
            SetConsoleMode(stdout(), o);
        }
    }

    pub fn size() -> (u16, u16) {
        unsafe {
            let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
            if GetConsoleScreenBufferInfo(stdout(), &mut info) == 0 {
                return (80, 24);
            }
            let w = info.srWindow;
            ((w.Right - w.Left + 1) as u16, (w.Bottom - w.Top + 1) as u16)
        }
    }

    pub fn write(b: &[u8]) {
        let mut off = 0;
        while off < b.len() {
            let mut n = 0u32;
            let ok = unsafe { WriteFile(stdout(), b[off..].as_ptr(), (b.len() - off) as u32, &mut n, std::ptr::null_mut()) };
            if ok == 0 || n == 0 {
                return;
            }
            off += n as usize;
        }
    }

    /// True while `b` ends inside an escape sequence (lone ESC, ESC [ params, ESC O).
    pub fn incomplete_escape(b: &[u8]) -> bool {
        let Some(i) = b.iter().rposition(|&c| c == 0x1b) else { return false };
        let t = &b[i..];
        match t.get(1) {
            None => true,
            Some(b'[') => !t[2..].iter().any(|c| (0x40..=0x7e).contains(c)),
            Some(b'O') => t.len() == 2,
            _ => false,
        }
    }

    /// Waits up to `ms` for more console input.
    pub fn more_input(ms: u32) -> bool {
        unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(stdin(), ms) == 0 }
    }

    pub enum Event {
        Text(Vec<u8>),
        Resize(u16, u16),
    }

    /// Blocking: the next batch of console input, as UTF-8 text or a resize.
    /// `pending` carries a lone high surrogate across batches.
    pub fn read(pending: &mut Option<u16>) -> Vec<Event> {
        let mut recs: [INPUT_RECORD; 128] = unsafe { std::mem::zeroed() };
        let mut n = 0u32;
        if unsafe { ReadConsoleInputW(stdin(), recs.as_mut_ptr(), recs.len() as u32, &mut n) } == 0 {
            return Vec::new();
        }
        let mut events = Vec::new();
        let mut units: Vec<u16> = pending.take().into_iter().collect();
        let flush = |units: &mut Vec<u16>, events: &mut Vec<Event>, pending: &mut Option<u16>| {
            if let Some(&last) = units.last() {
                if (0xD800..0xDC00).contains(&last) {
                    *pending = units.pop();
                }
            }
            if !units.is_empty() {
                events.push(Event::Text(String::from_utf16_lossy(units).into_bytes()));
                units.clear();
            }
        };
        for r in &recs[..n as usize] {
            match r.EventType as u32 {
                KEY_EVENT => {
                    let k = unsafe { r.Event.KeyEvent };
                    let ch = unsafe { k.uChar.UnicodeChar };
                    if let Ok(path) = std::env::var("KV_TRACE") {
                        use std::io::Write;
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                            let _ = writeln!(f, "down={} vk=0x{:02x} scan=0x{:02x} ch=U+{:04X} state=0x{:x} rep={}", k.bKeyDown, k.wVirtualKeyCode, k.wVirtualScanCode, ch, k.dwControlKeyState, k.wRepeatCount);
                        }
                    }
                    // Characters not on the keyboard layout arrive as an Alt+numpad
                    // sequence whose character rides on the Alt key-up.
                    let alt_up = k.bKeyDown == 0 && k.wVirtualKeyCode == 0x12;
                    if (k.bKeyDown != 0 || alt_up) && ch != 0 {
                        for _ in 0..k.wRepeatCount.max(1) {
                            units.push(ch);
                        }
                    }
                }
                WINDOW_BUFFER_SIZE_EVENT => {
                    flush(&mut units, &mut events, pending);
                    // The record's own size: querying the console here can still
                    // return the old one. Under ConPTY buffer and window match.
                    let d = unsafe { r.Event.WindowBufferSizeEvent.dwSize };
                    if let Ok(path) = std::env::var("KV_TRACE") {
                        use std::io::Write;
                        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                            let _ = writeln!(f, "bufsize event {}x{} console {:?}", d.X, d.Y, size());
                        }
                    }
                    events.push(Event::Resize(d.X as u16, d.Y as u16));
                }
                _ => {}
            }
        }
        flush(&mut units, &mut events, pending);
        events
    }
}

fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(5 + payload.len());
    f.push(kind);
    f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    f.extend_from_slice(payload);
    f
}

fn attach(rest: &[String]) -> Result<()> {
    let pipe = opt(rest, "--pipe").unwrap_or_else(default_pipe);
    if let Some(t) = opt(rest, "--trace") {
        std::env::set_var("KV_TRACE", t);
    }
    let old = console::raw();
    let (cols, rows) = console::size();
    let rt = tokio::runtime::Runtime::new()?;
    let r = rt.block_on(async {
        let mut c = connect(&pipe).await?;
        c.write_all(format!("{}\n", json!({ "op": "attach", "cols": cols, "rows": rows })).as_bytes()).await?;
        let (mut rd, mut wr) = tokio::io::split(c);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        std::thread::spawn(move || {
            let mut pending = None;
            let mut last = (cols, rows);
            let mut held: Vec<u8> = Vec::new();
            loop {
                let events = console::read(&mut pending);
                let mut frames = Vec::new();
                for ev in events {
                    match ev {
                        console::Event::Text(t) => held.extend_from_slice(&t),
                        other => {
                            if !held.is_empty() {
                                frames.push(frame(0, &std::mem::take(&mut held)));
                            }
                            frames.push(match other {
                                console::Event::Resize(c, r) => {
                                    if (c, r) == last {
                                        continue;
                                    }
                                    last = (c, r);
                                    let mut p = c.to_le_bytes().to_vec();
                                    p.extend_from_slice(&r.to_le_bytes());
                                    frame(1, &p)
                                }
                                console::Event::Text(_) => unreachable!(),
                            });
                        }
                    }
                }
                // Keep an escape sequence in one frame: sshd's ConPTY can split
                // it across reads, and a lone ESC reaches the shell as Escape.
                if !(console::incomplete_escape(&held) && console::more_input(20)) && !held.is_empty() {
                    frames.push(frame(0, &std::mem::take(&mut held)));
                }
                for f in frames {
                    if tx.send(f).is_err() {
                        return;
                    }
                }
            }
        });
        let up = async {
            while let Some(f) = rx.recv().await {
                wr.write_all(&f).await?;
            }
            Ok::<_, anyhow::Error>(())
        };
        let down = async {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = rd.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                console::write(&buf[..n]);
            }
            Ok::<_, anyhow::Error>(())
        };
        tokio::select! { r = up => r, r = down => r }
    });
    console::restore(old);
    r?;
    // The input thread is blocked in ReadConsoleInputW; just leave.
    std::process::exit(0)
}
