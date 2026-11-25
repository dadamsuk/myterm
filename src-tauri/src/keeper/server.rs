//! `myterm.exe --keeper`: owns the sessions and serves the control pipe.

use super::conpty::Con;
use super::winsec;
use super::{Hello, KSession, FRAME_INPUT, FRAME_RESIZE, PROTO, VERSION};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::{broadcast, watch};

pub fn pipe_name() -> String {
    format!(r"\\.\pipe\myterm-keeper-{}", winsec::user_sid())
}

/// `%LOCALAPPDATA%\myterm`: keeper.json, keeper.log and the keeper's exe copy.
pub fn local_dir() -> PathBuf {
    let d = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("myterm");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Merges `fields` into keeper.json, which ssh commands read to find the CLI.
pub fn update_info(fields: Value) {
    let path = local_dir().join("keeper.json");
    let mut v: Value = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(json!({}));
    if let (Some(o), Some(f)) = (v.as_object_mut(), fields.as_object()) {
        for (k, x) in f {
            o.insert(k.clone(), x.clone());
        }
    }
    let _ = std::fs::write(path, serde_json::to_string_pretty(&v).unwrap_or_default());
}

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

fn log(msg: impl AsRef<str>) {
    if let Some(f) = LOG.lock().unwrap().as_mut() {
        let _ = writeln!(f, "{} [{}] {}", crate::config::now_secs(), std::process::id(), msg.as_ref());
    }
}

fn open_log() {
    let path = local_dir().join("keeper.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1 << 20) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    *LOG.lock().unwrap() = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
}

struct Sess {
    id: String,
    name: Mutex<String>,
    kind: String,
    cwd: String,
    created: i64,
    last_attached: AtomicI64,
    attached: AtomicU32,
    con: Con,
    out: broadcast::Sender<Vec<u8>>,
    ended: watch::Sender<bool>,
    size: Mutex<(u16, u16)>,
}

impl Sess {
    fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        self.con.resize(cols, rows)?;
        *self.size.lock().unwrap() = (cols, rows);
        Ok(())
    }

    /// Makes ConPTY repaint the whole screen: resize to the client's size, or,
    /// if it already is that size, by a row and back.
    async fn repaint(&self, cols: u16, rows: u16) -> Result<()> {
        if *self.size.lock().unwrap() == (cols, rows) {
            self.resize(cols, rows + 1)?;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.resize(cols, rows)
    }

    fn info(&self) -> KSession {
        let (state, state_at) = read_state(&self.id);
        KSession {
            id: self.id.clone(),
            name: self.name.lock().unwrap().clone(),
            kind: self.kind.clone(),
            cwd: self.cwd.clone(),
            created: self.created,
            last_attached: self.last_attached.load(Ordering::SeqCst),
            attached: self.attached.load(Ordering::SeqCst),
            state,
            state_at,
        }
    }
}

/// Attention state written by the Claude hook (hooks.rs HOOK_CMD), keyed by MYTERM_SESSION.
fn state_file(id: &str) -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".myterm").join("state").join(id)
}

fn read_state(id: &str) -> (Option<String>, i64) {
    let path = state_file(id);
    let Ok(text) = std::fs::read_to_string(&path) else { return (None, 0) };
    let at = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let s = text.trim().to_string();
    (if s.is_empty() { None } else { Some(s) }, at)
}

struct Keeper {
    sessions: Mutex<Vec<Arc<Sess>>>,
    next: AtomicU64,
}

impl Keeper {
    fn find(&self, id: &str) -> Result<Arc<Sess>> {
        self.sessions.lock().unwrap().iter().find(|s| s.id == id).cloned().ok_or_else(|| anyhow!("no session {id}"))
    }

    fn create(self: &Arc<Self>, req: &Value) -> Result<String> {
        let name = req["name"].as_str().unwrap_or("session").to_string();
        let kind = if req["kind"].as_str() == Some("claude") { "claude" } else { "shell" }.to_string();
        let cols = req["cols"].as_u64().unwrap_or(120) as u16;
        let rows = req["rows"].as_u64().unwrap_or(30) as u16;
        let id = self.next.fetch_add(1, Ordering::SeqCst).to_string();

        let mut env = winsec::user_environment();
        winsec::set_var(&mut env, "TERM", "xterm-256color");
        winsec::set_var(&mut env, "COLORTERM", "truecolor");
        winsec::set_var(&mut env, "MYTERM_SESSION", &id);
        let home = winsec::get_var(&env, "USERPROFILE").map(str::to_string).unwrap_or_default();
        let shell = req["shell"].as_str().filter(|s| !s.trim().is_empty()).map(str::to_string).unwrap_or_else(|| default_shell(&env));
        let cwd = expand_dir(req["cwd"].as_str().unwrap_or(""), &home);

        let con = Con::spawn(&shell, &cwd, &winsec::environment_block(env), cols, rows)?;
        if kind == "claude" {
            // Typed into the shell (as on tmux hosts, 5.5): Claude gets the shell's
            // environment, and the session stays a shell when Claude exits.
            let l = shell.to_ascii_lowercase();
            let line = if l.contains("pwsh") || l.contains("powershell") {
                format!("claude --rc '{}'\r", name.replace('\'', "''"))
            } else {
                format!("claude --rc \"{}\"\r", name.replace('"', ""))
            };
            (&con.input).write_all(line.as_bytes())?;
        }
        let mut reader = con.output.try_clone()?;
        let (out, _) = broadcast::channel(1024);
        let (ended, _) = watch::channel(false);
        let s = Arc::new(Sess {
            id: id.clone(),
            name: Mutex::new(name),
            kind,
            cwd: cwd.clone(),
            created: crate::config::now_secs(),
            last_attached: AtomicI64::new(0),
            attached: AtomicU32::new(0),
            con,
            out: out.clone(),
            ended,
            size: Mutex::new((cols, rows)),
        });
        log(format!("create {id} pid {} shell {shell:?} cwd {cwd:?}", s.con.pid));

        // Always drain output, attached or not, or the child blocks on a full pipe.
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = out.send(buf[..n].to_vec());
            }
        });
        let k = self.clone();
        let s2 = s.clone();
        std::thread::spawn(move || {
            let code = s2.con.wait();
            k.sessions.lock().unwrap().retain(|x| x.id != s2.id);
            s2.con.close();
            let _ = s2.ended.send(true);
            let _ = std::fs::remove_file(state_file(&s2.id));
            log(format!("session {} exited {code}", s2.id));
        });
        self.sessions.lock().unwrap().push(s);
        Ok(id)
    }
}

fn default_shell(env: &[(String, String)]) -> String {
    let path = winsec::get_var(env, "PATH").unwrap_or("");
    let found = std::env::split_paths(path).any(|d| d.join("pwsh.exe").is_file());
    if found { "pwsh.exe".into() } else { "powershell.exe".into() }
}

fn expand_dir(dir: &str, home: &str) -> String {
    let d = dir.trim();
    let d = match d.strip_prefix('~') {
        Some(rest) => format!("{home}{}", rest.replace('/', "\\")),
        None => d.to_string(),
    };
    if !d.is_empty() && std::path::Path::new(&d).is_dir() {
        d
    } else {
        home.to_string()
    }
}

fn hello() -> Hello {
    Hello {
        version: VERSION.into(),
        proto: PROTO,
        pid: std::process::id(),
        session_id: winsec::session_id(),
        interactive: winsec::interactive(),
        integrity: winsec::integrity(),
        windows_build: winsec::windows_build(),
        home: dirs::home_dir().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default(),
        user: std::env::var("USERNAME").unwrap_or_default(),
    }
}

/// Writes an upload into `%TEMP%\myterm\` and returns its path (7, file drop).
fn upload(req: &Value) -> Result<String> {
    let name = crate::files::safe_name(req["name"].as_str().unwrap_or("file"));
    let data = super::base64_decode(req["data"].as_str().unwrap_or("")).ok_or_else(|| anyhow!("bad upload data"))?;
    let dir = std::env::temp_dir().join("myterm");
    std::fs::create_dir_all(&dir)?;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() > 86400);
            if old {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let path = dir.join(format!("{}-{name}", crate::config::now_secs()));
    std::fs::write(&path, data)?;
    Ok(path.to_string_lossy().into_owned())
}

enum After {
    Continue,
    Exit,
}

fn dispatch(k: &Arc<Keeper>, op: &str, req: &Value) -> Result<(Value, After)> {
    let v = match op {
        "hello" => json!({ "hello": hello() }),
        "list" => {
            let list: Vec<KSession> = k.sessions.lock().unwrap().iter().map(|s| s.info()).collect();
            json!({ "sessions": list })
        }
        "create" => json!({ "id": k.create(req)? }),
        "rename" => {
            let s = k.find(req["id"].as_str().unwrap_or(""))?;
            *s.name.lock().unwrap() = req["name"].as_str().unwrap_or("").to_string();
            json!({})
        }
        "kill" => {
            let s = k.find(req["id"].as_str().unwrap_or(""))?;
            log(format!("kill {}", s.id));
            s.con.kill();
            json!({})
        }
        "upload" => json!({ "path": upload(req)? }),
        "shutdown" => {
            let n = k.sessions.lock().unwrap().len();
            if n > 0 && req["force"].as_bool() != Some(true) {
                bail!("{n} sessions are running");
            }
            log("shutdown requested");
            return Ok((json!({}), After::Exit));
        }
        _ => bail!("unknown request {op:?}"),
    };
    Ok((v, After::Continue))
}

fn reply(r: &Result<Value>) -> String {
    let mut v = match r {
        Ok(v) => v.clone(),
        Err(e) => json!({ "error": format!("{e:#}") }),
    };
    if let Some(o) = v.as_object_mut() {
        o.insert("ok".into(), json!(r.is_ok()));
    }
    format!("{v}\n")
}

async fn handle(conn: NamedPipeServer, k: Arc<Keeper>) -> Result<()> {
    let (rd, mut wr) = tokio::io::split(conn);
    let mut rd = BufReader::new(rd);
    let mut line = String::new();
    loop {
        line.clear();
        if rd.read_line(&mut line).await? == 0 {
            return Ok(());
        }
        let req: Value = match serde_json::from_str(line.trim()) {
            Ok(v) => v,
            Err(e) => {
                wr.write_all(reply(&Err(anyhow!("bad request: {e}"))).as_bytes()).await?;
                continue;
            }
        };
        let op = req["op"].as_str().unwrap_or("").to_string();
        if op == "attach" {
            return attach(rd, wr, &k, &req).await;
        }
        let k2 = k.clone();
        let res = tokio::task::spawn_blocking(move || dispatch(&k2, &op, &req)).await?;
        let (r, after) = match res {
            Ok((v, a)) => (Ok(v), a),
            Err(e) => (Err(e), After::Continue),
        };
        wr.write_all(reply(&r).as_bytes()).await?;
        wr.flush().await?;
        if let After::Exit = after {
            std::process::exit(0);
        }
    }
}

/// Decrements the attached count however the attach ends.
struct Attached(Arc<Sess>);

impl Drop for Attached {
    fn drop(&mut self) {
        self.0.attached.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn attach<R, W>(mut rd: BufReader<R>, mut wr: W, k: &Arc<Keeper>, req: &Value) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let s = match k.find(req["id"].as_str().unwrap_or("")) {
        Ok(s) => s,
        Err(e) => {
            wr.write_all(reply(&Err(e)).as_bytes()).await?;
            return Ok(());
        }
    };
    let cols = req["cols"].as_u64().unwrap_or(80) as u16;
    let rows = req["rows"].as_u64().unwrap_or(24) as u16;
    let mut sub = s.out.subscribe();
    let mut ended = s.ended.subscribe();
    s.attached.fetch_add(1, Ordering::SeqCst);
    let _guard = Attached(s.clone());
    s.last_attached.store(crate::config::now_secs(), Ordering::SeqCst);
    wr.write_all(reply(&Ok(json!({}))).as_bytes()).await?;
    // The client may show an earlier screen; the repaint redraws onto a clear one.
    wr.write_all(b"\x1b[H\x1b[2J").await?;
    wr.flush().await?;
    s.repaint(cols, rows).await?;

    let out = async {
        loop {
            match sub.recv().await {
                Ok(b) => {
                    wr.write_all(&b).await?;
                    wr.flush().await?;
                }
                // Output was dropped for this client: a repaint puts its screen right.
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let (c, r) = *s.size.lock().unwrap();
                    s.repaint(c, r).await?;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
        Ok::<_, anyhow::Error>(())
    };
    let inp = async {
        let mut hdr = [0u8; 5];
        loop {
            rd.read_exact(&mut hdr).await?;
            let len = u32::from_le_bytes(hdr[1..5].try_into().unwrap()) as usize;
            if len > 1 << 20 {
                bail!("frame too large");
            }
            let mut payload = vec![0u8; len];
            rd.read_exact(&mut payload).await?;
            match hdr[0] {
                FRAME_INPUT => (&s.con.input).write_all(&payload).context("write to session")?,
                FRAME_RESIZE if len == 4 => {
                    let c = u16::from_le_bytes([payload[0], payload[1]]);
                    let r = u16::from_le_bytes([payload[2], payload[3]]);
                    s.resize(c, r)?;
                }
                t => bail!("bad frame type {t}"),
            }
        }
        #[allow(unreachable_code)]
        Ok::<_, anyhow::Error>(())
    };
    let end = async {
        while !*ended.borrow() {
            if ended.changed().await.is_err() {
                break;
            }
        }
    };
    tokio::select! {
        r = out => r?,
        r = inp => r?,
        _ = end => {}
    }
    Ok(())
}

/// Entry point for `myterm.exe --keeper`. Never returns.
pub fn run() -> ! {
    open_log();
    // The UI starts the keeper with CREATE_NEW_PROCESS_GROUP, which makes it
    // ignore Ctrl+C, and every process it starts inherits that: Ctrl+C typed in
    // a session reached the shell's line editor but never stopped a running
    // program (ping, node, python). Turn it back on before any session starts.
    unsafe { windows_sys::Win32::System::Console::SetConsoleCtrlHandler(None, 0) };
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let r = rt.block_on(serve());
    if let Err(e) = r {
        log(format!("keeper stopped: {e:#}"));
        std::process::exit(1);
    }
    std::process::exit(0)
}

/// The notification-area icon (tray.rs), for a keeper in the desktop logon.
fn start_tray(k: &Arc<Keeper>) {
    let (k1, k2) = (k.clone(), k.clone());
    super::tray::spawn(super::tray::Hooks {
        count: Box::new(move || k1.sessions.lock().unwrap().len()),
        quit: Box::new(move || {
            let all: Vec<Arc<Sess>> = k2.sessions.lock().unwrap().clone();
            log(format!("quit from the tray, ending {} sessions", all.len()));
            // Until myterm is started here again, ssh channels from other
            // machines must not start a new keeper (cli::ensure_keeper).
            update_info(json!({ "quit_from_tray": crate::config::now_secs() }));
            for s in &all {
                s.con.kill();
                // Closing the pseudoconsole also ends programs still attached to it.
                s.con.close();
                let _ = std::fs::remove_file(state_file(&s.id));
            }
            std::process::exit(0);
        }),
        // Written by the UI and by myterm-cli (cli::keeper_exe); the keeper itself
        // runs from a copy, so its own path is no use.
        app: Box::new(|| {
            let text = std::fs::read_to_string(local_dir().join("keeper.json")).ok()?;
            let v: Value = serde_json::from_str(&text).ok()?;
            let p = PathBuf::from(v["app"].as_str()?);
            p.is_file().then_some(p)
        }),
        log: |m| log(m),
    });
}

async fn serve() -> Result<()> {
    let pipe = pipe_name();
    let sa = winsec::security_attributes(&format!("D:P(A;;GA;;;{})", winsec::user_sid()))?;
    let create = |first: bool| unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(&pipe, sa as *mut _)
    };
    // The first instance is the single-instance lock: pipe names are machine-wide.
    let mut server = match create(true) {
        Ok(s) => s,
        Err(e) => {
            log(format!("not starting: {e} (another keeper is running)"));
            return Ok(());
        }
    };
    let h = hello();
    log(format!("started v{VERSION} session {} interactive {} integrity {}", h.session_id, h.interactive, h.integrity));
    update_info(json!({
        "pipe": pipe,
        "keeper_pid": std::process::id(),
        "keeper_version": VERSION,
        "keeper_exe": std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
    }));
    let base = (crate::config::now_secs() as u64) * 1000;
    let k = Arc::new(Keeper { sessions: Mutex::new(vec![]), next: AtomicU64::new(base) });
    if h.interactive && h.session_id != 0 {
        start_tray(&k);
    }
    loop {
        server.connect().await?;
        let conn = server;
        server = create(false)?;
        let k = k.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(conn, k).await {
                log(format!("connection: {e:#}"));
            }
        });
    }
}
