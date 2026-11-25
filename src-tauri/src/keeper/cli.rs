//! `myterm-cli` (also `myterm.exe --cli` in debug builds, which are console
//! programs): the stdio side of the keeper, for ssh and for the UI's PTYs.
//!
//!   myterm-cli channel        relay stdin/stdout to the keeper's control pipe,
//!                             starting the keeper first if it is not running
//!   myterm-cli attach <id>    attach this console to a session
//!   myterm-cli start          start the keeper if needed, then exit
//!   myterm-cli request JSON   send one request, print the reply (diagnostics)
//!   myterm-cli version

use super::server::{local_dir, pipe_name, update_info};
use super::{frame, resize_frame, FRAME_INPUT, VERSION};
use anyhow::{anyhow, bail, Result};
use serde_json::json;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PIPE_BUSY: i32 = 231;

pub async fn open_pipe(pipe: &str) -> Result<NamedPipeClient> {
    let start = Instant::now();
    loop {
        match ClientOptions::new().open(pipe) {
            Ok(c) => return Ok(c),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && start.elapsed() < Duration::from_secs(5) => {
                tokio::time::sleep(Duration::from_millis(30)).await
            }
            Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND) => bail!("the keeper is not running"),
            Err(e) => bail!("can't open the keeper's pipe: {e}"),
        }
    }
}

pub fn main(args: &[String]) -> ! {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let r = match args.first().map(String::as_str) {
        Some("channel") => rt.block_on(channel()),
        Some("attach") => match args.get(1) {
            Some(id) => rt.block_on(attach(id)),
            None => Err(anyhow!("usage: myterm-cli attach <session id>")),
        },
        Some("start") => rt.block_on(ensure_keeper()),
        Some("request") => rt.block_on(request(args.get(1).map(String::as_str).unwrap_or("{}"))),
        Some("version") => {
            println!("{}", json!({ "version": VERSION, "proto": super::PROTO }));
            Ok(())
        }
        _ => Err(anyhow!("usage: myterm-cli channel | attach <id> | start | version")),
    };
    match r {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("myterm-cli: {e:#}");
            std::process::exit(1)
        }
    }
}

// ---------------------------------------------------------------- starting the keeper

/// The exe the keeper runs from: a per-version copy of myterm.exe under
/// %LOCALAPPDATA%\myterm\keeper, so an installer can replace myterm.exe
/// while a keeper is running.
pub fn keeper_exe() -> Result<PathBuf> {
    let me = std::env::current_exe()?;
    let is_cli = me.file_stem().is_some_and(|s| s.to_string_lossy().to_ascii_lowercase().starts_with("myterm-cli"));
    let src = if !is_cli {
        me
    } else if me.with_file_name("myterm.exe").is_file() {
        me.with_file_name("myterm.exe")
    } else {
        // Running from a copy (cli_copy): the app recorded where it is.
        recorded_app().unwrap_or_else(|| me.with_file_name("myterm.exe"))
    };
    if !src.is_file() {
        bail!("{} not found", src.display());
    }
    update_info(json!({ "app": src.to_string_lossy() }));
    let dir = local_dir().join("keeper");
    std::fs::create_dir_all(&dir)?;
    let dst = dir.join(format!("myterm-keeper-{VERSION}.exe"));
    let stamp = |p: &PathBuf| std::fs::metadata(p).ok().map(|m| (m.len(), m.modified().ok()));
    if stamp(&src) != stamp(&dst) && std::fs::copy(&src, &dst).is_err() && !dst.is_file() {
        // In use or not writable: run from the original.
        return Ok(src);
    }
    Ok(dst)
}

fn info() -> serde_json::Value {
    std::fs::read_to_string(local_dir().join("keeper.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn recorded_app() -> Option<PathBuf> {
    Some(PathBuf::from(info()["app"].as_str()?))
}

/// Set by the tray's Quit (server.rs), cleared by the next local start.
fn quit_from_tray() -> bool {
    info()["quit_from_tray"].as_u64().is_some()
}

/// The myterm-cli that ssh commands run: a copy of `src` under
/// %LOCALAPPDATA%\myterm\cli named for this build, so a channel held open by
/// another machine never locks the shipped (or freshly built) myterm-cli.exe.
/// Copies from older builds are removed once nothing is running them.
pub fn cli_copy(src: &std::path::Path) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let Ok(m) = std::fs::metadata(src) else { return src.to_path_buf() };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (m.len(), m.modified().ok()).hash(&mut h);
    let dir = local_dir().join("cli");
    let dst = dir.join(format!("myterm-cli-{VERSION}-{:08x}.exe", h.finish() as u32));
    if dst.is_file() {
        return dst;
    }
    let tmp = dst.with_extension("tmp");
    let copied = std::fs::create_dir_all(&dir).is_ok()
        && std::fs::copy(src, &tmp).is_ok()
        && std::fs::rename(&tmp, &dst).is_ok();
    if !copied {
        let _ = std::fs::remove_file(&tmp);
        return src.to_path_buf();
    }
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path() != dst {
                let _ = std::fs::remove_file(e.path()); // fails while still running: next time
            }
        }
    }
    dst
}

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Starts the keeper as a detached child (from the desktop logon).
pub fn spawn_detached(exe: &PathBuf) -> Result<()> {
    let run = |flags: u32| {
        Command::new(exe)
            .arg("--keeper")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // Out of any job the UI was started in, where the job allows it.
    run(base | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| run(base))?;
    Ok(())
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn powershell(script: &str) -> Result<()> {
    let st = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .stdin(Stdio::null())
        .stdout(Stdio::null()) // stdout may be the channel's protocol stream
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()?;
    if !st.success() {
        bail!("powershell exited {:?}", st.code());
    }
    Ok(())
}

/// A per-user scheduled task that runs only while the user is logged on at
/// the desktop, so the keeper gets the desktop logon and its credentials.
fn start_by_task(exe: &PathBuf) -> Result<()> {
    powershell(&format!(
        "$ErrorActionPreference = 'Stop'
$a = New-ScheduledTaskAction -Execute {exe} -Argument '--keeper'
$p = New-ScheduledTaskPrincipal -UserId ([Security.Principal.WindowsIdentity]::GetCurrent().Name) -LogonType Interactive -RunLevel Limited
$s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit 0 -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -MultipleInstances IgnoreNew
Register-ScheduledTask -TaskName 'myterm-keeper' -Action $a -Principal $p -Settings $s -Description 'Keeps myterm terminal sessions running' -Force | Out-Null
Start-ScheduledTask -TaskName 'myterm-keeper'",
        exe = ps_quote(&exe.to_string_lossy())
    ))
}

/// WMI starts the process outside the ssh session's job, so it survives the
/// disconnect, but in a network logon without Credential Manager.
fn start_by_wmi(exe: &PathBuf) -> Result<()> {
    let cl = format!("\"{}\" --keeper", exe.display());
    powershell(&format!(
        "$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{{ CommandLine = {} }}
if ($r.ReturnValue -ne 0) {{ exit 1 }}",
        ps_quote(&cl)
    ))
}

async fn wait_for_pipe(pipe: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if ClientOptions::new().open(pipe).is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    false
}

/// Starts the keeper unless it is running. Under ssh a child of this process
/// would die with the connection, so it is started by the task scheduler or WMI.
pub async fn ensure_keeper() -> Result<()> {
    let pipe = pipe_name();
    if ClientOptions::new().open(&pipe).is_ok() {
        return Ok(());
    }
    let over_ssh = std::env::var_os("SSH_CONNECTION").is_some();
    if over_ssh && quit_from_tray() {
        bail!("the keeper was quit from the tray there; start myterm on that machine to start it again");
    }
    let exe = keeper_exe()?;
    if !over_ssh {
        // Started here (the app, or myterm-cli at this desktop): a tray quit no longer holds.
        update_info(json!({ "quit_from_tray": null }));
        spawn_detached(&exe)?;
        if wait_for_pipe(&pipe, Duration::from_secs(10)).await {
            return Ok(());
        }
        bail!("the keeper did not start");
    }
    if start_by_task(&exe).is_ok() && wait_for_pipe(&pipe, Duration::from_secs(8)).await {
        return Ok(());
    }
    // Nobody is logged on at the desktop (an Interactive task can't run then).
    start_by_wmi(&exe)?;
    if wait_for_pipe(&pipe, Duration::from_secs(10)).await {
        return Ok(());
    }
    bail!("the keeper did not start")
}

async fn request(json: &str) -> Result<()> {
    let mut pipe = open_pipe(&pipe_name()).await?;
    let req: serde_json::Value = serde_json::from_str(json)?;
    pipe.write_all(format!("{req}
").as_bytes()).await?;
    let reply = read_reply(&mut pipe).await?;
    println!("{}", serde_json::to_string_pretty(&reply)?);
    Ok(())
}

// ---------------------------------------------------------------- channel

/// Relays the control protocol between stdio (ssh -T) and the pipe.
async fn channel() -> Result<()> {
    ensure_keeper().await?;
    let pipe = open_pipe(&pipe_name()).await?;
    let (mut pr, mut pw) = tokio::io::split(pipe);
    let up = async {
        let mut stdin = tokio::io::stdin();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = stdin.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            pw.write_all(&buf[..n]).await?;
            pw.flush().await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let down = async {
        let mut stdout = tokio::io::stdout();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = pr.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            stdout.write_all(&buf[..n]).await?;
            stdout.flush().await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    tokio::pin!(down);
    tokio::select! {
        r = &mut down => r,
        r = up => {
            // stdin closed: let replies already on their way through.
            let _ = tokio::time::timeout(Duration::from_secs(5), &mut down).await;
            r
        }
    }
}

// ---------------------------------------------------------------- attach

mod console {
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::WriteFile;
    use windows_sys::Win32::System::Console::*;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    pub fn stdin() -> HANDLE {
        unsafe { GetStdHandle(STD_INPUT_HANDLE) }
    }
    pub fn stdout() -> HANDLE {
        unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }
    }

    /// Raw VT input, VT output, UTF-8 both ways. Returns the old state.
    pub fn raw() -> (u32, u32, u32, u32) {
        unsafe {
            let (icp, ocp) = (GetConsoleCP(), GetConsoleOutputCP());
            SetConsoleCP(65001);
            SetConsoleOutputCP(65001);
            let (mut i, mut o) = (0u32, 0u32);
            GetConsoleMode(stdin(), &mut i);
            GetConsoleMode(stdout(), &mut o);
            SetConsoleMode(
                stdin(),
                // No ENABLE_MOUSE_INPUT: with it, the ConPTY this runs in turns on
                // mouse tracking in the terminal (?1003;1006h), so a drag no
                // longer selects text there, and the mouse records were dropped.
                (i & !(ENABLE_PROCESSED_INPUT
                    | ENABLE_LINE_INPUT
                    | ENABLE_ECHO_INPUT
                    | ENABLE_QUICK_EDIT_MODE
                    | ENABLE_MOUSE_INPUT))
                    | ENABLE_VIRTUAL_TERMINAL_INPUT
                    | ENABLE_WINDOW_INPUT
                    | ENABLE_EXTENDED_FLAGS,
            );
            SetConsoleMode(stdout(), o | ENABLE_VIRTUAL_TERMINAL_PROCESSING | DISABLE_NEWLINE_AUTO_RETURN);
            (i, o, icp, ocp)
        }
    }

    pub fn restore((i, o, icp, ocp): (u32, u32, u32, u32)) {
        unsafe {
            SetConsoleMode(stdin(), i);
            SetConsoleMode(stdout(), o);
            SetConsoleCP(icp);
            SetConsoleOutputCP(ocp);
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

    pub fn write(b: &[u8]) -> bool {
        let mut off = 0;
        while off < b.len() {
            let mut n = 0u32;
            let ok = unsafe { WriteFile(stdout(), b[off..].as_ptr(), (b.len() - off) as u32, &mut n, std::ptr::null_mut()) };
            if ok == 0 || n == 0 {
                return false;
            }
            off += n as usize;
        }
        true
    }

    /// Waits up to `ms` for more console input.
    pub fn more_input(ms: u32) -> bool {
        unsafe { WaitForSingleObject(stdin(), ms) == 0 }
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

    pub enum Event {
        Text(Vec<u8>),
        Resize(u16, u16),
    }

    /// Blocking: the next batch of console input. `pending` carries a lone
    /// high surrogate to the next batch.
    pub fn read(pending: &mut Option<u16>) -> Option<Vec<Event>> {
        let mut recs: [INPUT_RECORD; 128] = unsafe { std::mem::zeroed() };
        let mut n = 0u32;
        if unsafe { ReadConsoleInputW(stdin(), recs.as_mut_ptr(), recs.len() as u32, &mut n) } == 0 {
            return None;
        }
        let mut events = Vec::new();
        let mut units: Vec<u16> = pending.take().into_iter().collect();
        let flush = |units: &mut Vec<u16>, events: &mut Vec<Event>, pending: &mut Option<u16>| {
            if units.last().is_some_and(|&u| (0xD800..0xDC00).contains(&u)) {
                *pending = units.pop();
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
                    // Characters that are not on the keyboard layout come as an
                    // Alt+numpad sequence whose character is on the Alt key-up.
                    let alt_up = k.bKeyDown == 0 && k.wVirtualKeyCode == 0x12;
                    if (k.bKeyDown != 0 || alt_up) && ch != 0 {
                        for _ in 0..k.wRepeatCount.max(1) {
                            units.push(ch);
                        }
                    }
                }
                WINDOW_BUFFER_SIZE_EVENT => {
                    flush(&mut units, &mut events, pending);
                    // The record's size: asking the console now can still give the old one.
                    let d = unsafe { r.Event.WindowBufferSizeEvent.dwSize };
                    events.push(Event::Resize(d.X as u16, d.Y as u16));
                }
                _ => {}
            }
        }
        flush(&mut units, &mut events, pending);
        Some(events)
    }
}

/// Reads the one-line JSON reply byte by byte, so no stream bytes are consumed.
async fn read_reply(p: &mut NamedPipeClient) -> Result<serde_json::Value> {
    let mut line = Vec::new();
    let mut b = [0u8; 1];
    loop {
        if p.read(&mut b).await? == 0 {
            bail!("the keeper closed the connection");
        }
        if b[0] == b'\n' {
            break;
        }
        line.push(b[0]);
    }
    Ok(serde_json::from_slice(&line)?)
}

async fn attach(id: &str) -> Result<()> {
    let mut pipe = open_pipe(&pipe_name()).await?;
    let (cols, rows) = console::size();
    pipe.write_all(format!("{}\n", json!({ "op": "attach", "id": id, "cols": cols, "rows": rows })).as_bytes()).await?;
    let reply = read_reply(&mut pipe).await?;
    if reply["ok"].as_bool() != Some(true) {
        bail!("{}", reply["error"].as_str().unwrap_or("attach failed"));
    }
    let old = console::raw();
    let (mut rd, mut wr) = tokio::io::split(pipe);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut pending = None;
        let mut last = (cols, rows);
        let mut held: Vec<u8> = Vec::new();
        while let Some(events) = console::read(&mut pending) {
            let mut frames = Vec::new();
            for ev in events {
                match ev {
                    console::Event::Text(t) => held.extend_from_slice(&t),
                    console::Event::Resize(c, r) => {
                        if !held.is_empty() {
                            frames.push(frame(FRAME_INPUT, &std::mem::take(&mut held)));
                        }
                        if (c, r) != last {
                            last = (c, r);
                            frames.push(resize_frame(c, r));
                        }
                    }
                }
            }
            // Keep an escape sequence in one frame: sshd's ConPTY splits them
            // across reads, and a lone ESC reaches the shell as the Escape key.
            let wait = console::incomplete_escape(&held) && console::more_input(20);
            if !wait && !held.is_empty() {
                frames.push(frame(FRAME_INPUT, &std::mem::take(&mut held)));
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
            if n == 0 || !console::write(&buf[..n]) {
                break;
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    let r = tokio::select! { r = up => r, r = down => r };
    console::restore(old);
    // The input thread is blocked in ReadConsoleInputW; exiting ends it.
    r
}
