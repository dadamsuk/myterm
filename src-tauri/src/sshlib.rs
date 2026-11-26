//! Built-in SSH client (14.4), used on Android and for desktop hosts set to
//! `transport = "builtin"`. One connection per destination; the host channel,
//! keeper channel and every attach are SSH channels on it.

use crate::config::data_dir;
use anyhow::{anyhow, bail, Result};
use russh::client::{self, AuthResult, Handle};
use russh::keys::{Algorithm, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::ChannelMsg;
use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{mpsc, oneshot, Mutex};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const OPEN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Dest {
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Desktop only: the key files ssh would offer (`ssh -G`), tried before
    /// the app's own key.
    pub identities: Vec<PathBuf>,
}

impl Dest {
    /// The name the host key is stored under, as OpenSSH writes it.
    fn known_name(&self) -> String {
        if self.port == 22 {
            self.host.clone()
        } else {
            format!("[{}]:{}", self.host, self.port)
        }
    }
}

/// Authentication or host-key failure: retrying will not help until the user
/// acts (adds the key, forgets a changed host key).
#[derive(Debug)]
pub struct NeedsKey(pub String);

impl std::fmt::Display for NeedsKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NeedsKey {}

// ---------- host keys and the app's own key ----------

fn known_hosts_path() -> PathBuf {
    data_dir().join("known_hosts")
}

pub fn key_path() -> PathBuf {
    data_dir().join("ssh_ed25519")
}

/// "type base64", without the comment.
fn key_text(k: &PublicKey) -> String {
    k.to_openssh().unwrap_or_default().split_whitespace().take(2).collect::<Vec<_>>().join(" ")
}

enum HostKey {
    Known,
    New,
    Changed,
}

fn check_known(text: &str, name: &str, key: &str) -> HostKey {
    let mut seen = false;
    for line in text.lines() {
        let mut parts = line.splitn(2, ' ');
        if parts.next() != Some(name) {
            continue;
        }
        if parts.next().map(str::trim) == Some(key) {
            return HostKey::Known;
        }
        seen = true;
    }
    if seen {
        HostKey::Changed
    } else {
        HostKey::New
    }
}

/// Removes a host's stored keys, so the next connection trusts its key again.
pub fn forget_host_key(dest: &Dest) -> Result<()> {
    let path = known_hosts_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let name = dest.known_name();
    let kept: Vec<&str> = text.lines().filter(|l| l.split(' ').next() != Some(name.as_str())).collect();
    std::fs::write(&path, kept.iter().map(|l| format!("{l}\n")).collect::<String>())?;
    Ok(())
}

static KEY_LOCK: StdMutex<()> = StdMutex::new(());

/// The app's key, created on first use.
pub fn app_key() -> Result<PrivateKey> {
    let _g = KEY_LOCK.lock().unwrap();
    let path = key_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        return Ok(PrivateKey::from_openssh(text)?);
    }
    let mut key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
    key.set_comment("myterm");
    let text = key.to_openssh(russh::keys::ssh_key::LineEnding::LF)?;
    std::fs::create_dir_all(data_dir())?;
    std::fs::write(&path, text.as_bytes())?;
    Ok(key)
}

/// The public half, as one `authorized_keys` line.
pub fn public_key_line() -> Result<String> {
    Ok(app_key()?.public_key().to_openssh()?)
}

// ---------- connections ----------

struct Client {
    dest: Dest,
    refusal: Arc<StdMutex<Option<String>>>,
}

impl client::Handler for Client {
    type Error = anyhow::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(c) => PublicKey::from(c.public_key().clone()),
        };
        let text = key_text(&key);
        let name = self.dest.known_name();
        let path = known_hosts_path();
        let known = std::fs::read_to_string(&path).unwrap_or_default();
        match check_known(&known, &name, &text) {
            HostKey::Known => Ok(true),
            HostKey::New => {
                use std::io::Write;
                std::fs::create_dir_all(data_dir())?;
                let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
                writeln!(f, "{name} {text}")?;
                Ok(true)
            }
            HostKey::Changed => {
                *self.refusal.lock().unwrap() =
                    Some(format!("the host key of {} has changed; forget it from the host's menu if that was expected", self.dest.host));
                Ok(false)
            }
        }
    }
}

type Conn = Arc<Handle<Client>>;

/// One slot per destination, so connecting to one host never waits on another.
type Slot = Arc<Mutex<Option<Conn>>>;

static POOL: LazyLock<StdMutex<HashMap<Dest, Slot>>> = LazyLock::new(Default::default);

async fn open(dest: &Dest) -> Result<Handle<Client>> {
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 2,
        inactivity_timeout: None,
        nodelay: true,
        ..Default::default()
    });
    let refusal = Arc::new(StdMutex::new(None));
    let handler = Client { dest: dest.clone(), refusal: refusal.clone() };
    let connecting = client::connect(config, (dest.host.as_str(), dest.port), handler);
    let mut h = match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => bail!("connection timed out"),
        Ok(Err(e)) => {
            if let Some(why) = refusal.lock().unwrap().take() {
                return Err(NeedsKey(why).into());
            }
            return Err(e);
        }
        Ok(Ok(h)) => h,
    };
    // Keys first: ssh's own (unencrypted ones; there is no passphrase
    // prompt), then the app's.
    let mut keys: Vec<PrivateKey> = dest
        .identities
        .iter()
        .filter(|p| p.is_file())
        .filter_map(|p| russh::keys::load_secret_key(p, None).ok())
        .collect();
    keys.push(app_key()?);
    for key in keys {
        let hash = if matches!(key.algorithm(), Algorithm::Rsa { .. }) {
            h.best_supported_rsa_hash().await?.flatten()
        } else {
            None
        };
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
        if matches!(h.authenticate_publickey(&dest.user, key).await?, AuthResult::Success) {
            return Ok(h);
        }
    }
    // Tailscale SSH accepts "none" when the tailnet policy allows this user.
    if matches!(h.authenticate_none(&dest.user).await?, AuthResult::Success) {
        return Ok(h);
    }
    Err(NeedsKey(format!("{}@{} did not accept this app's key", dest.user, dest.host)).into())
}

/// The connection to `dest`, opening it if there is none or it has closed.
async fn connection(dest: &Dest) -> Result<Conn> {
    let slot = POOL.lock().unwrap().entry(dest.clone()).or_default().clone();
    let mut g = slot.lock().await;
    if let Some(c) = g.as_ref() {
        if !c.is_closed() {
            return Ok(c.clone());
        }
    }
    *g = None;
    let c = Arc::new(open(dest).await?);
    *g = Some(c.clone());
    Ok(c)
}

/// Drops the connection to `dest`; the next request opens a new one.
pub fn forget_connection(dest: &Dest) {
    let slot = POOL.lock().unwrap().remove(dest);
    if let Some(slot) = slot {
        tokio::spawn(async move {
            if let Some(c) = slot.lock().await.take() {
                let _ = c.disconnect(russh::Disconnect::ByApplication, "", "").await;
            }
        });
    }
}

// ---------- processes ----------

pub enum In {
    Data(Vec<u8>),
    Eof,
    Resize(u16, u16),
    Close,
}

/// A remote command or shell on its own SSH channel.
pub struct Proc {
    pub tx: mpsc::UnboundedSender<In>,
    pub rx: mpsc::Receiver<Vec<u8>>,
    pub stderr: Arc<StdMutex<String>>,
    /// The exit status; 255 when the channel closed without one.
    pub exit: oneshot::Receiver<u32>,
}

/// Runs `cmd` (a shell when None) on `dest`, with a PTY of `pty` columns and
/// rows when given.
pub async fn spawn(dest: &Dest, cmd: Option<&str>, pty: Option<(u16, u16)>) -> Result<Proc> {
    let conn = connection(dest).await?;
    let ch = match tokio::time::timeout(OPEN_TIMEOUT, conn.channel_open_session()).await {
        Ok(Ok(ch)) => ch,
        Ok(Err(e)) => {
            forget_connection(dest);
            return Err(e.into());
        }
        Err(_) => {
            forget_connection(dest);
            bail!("timed out opening a channel");
        }
    };
    if let Some((cols, rows)) = pty {
        ch.request_pty(false, "xterm-256color", cols.max(10) as u32, rows.max(2) as u32, 0, 0, &[]).await?;
        // Most sshd configurations refuse this; harmless if so.
        let _ = ch.set_env(false, "COLORTERM", "truecolor").await;
    }
    match cmd {
        Some(c) => ch.exec(false, c).await?,
        None => ch.request_shell(false).await?,
    }

    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<In>();
    // Bounded: a reader that falls behind stops us reading the channel, which
    // stops the SSH window from refilling, which stops the remote end.
    let (out_tx, out_rx) = mpsc::channel::<Vec<u8>>(64);
    let (exit_tx, exit_rx) = oneshot::channel();
    let stderr = Arc::new(StdMutex::new(String::new()));
    let (mut read, write) = ch.split();

    let writer = tokio::spawn(async move {
        let mut open = true;
        while let Some(m) = in_rx.recv().await {
            let ok = match m {
                In::Data(d) => open && write.data(&d[..]).await.is_ok(),
                In::Eof => {
                    open = false;
                    write.eof().await.is_ok()
                }
                In::Resize(c, r) => write.window_change(c.max(10) as u32, r.max(2) as u32, 0, 0).await.is_ok(),
                In::Close => {
                    let _ = write.close().await;
                    false
                }
            };
            if !ok {
                break;
            }
        }
    });

    let err = stderr.clone();
    tokio::spawn(async move {
        let mut code = None;
        while let Some(msg) = read.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    if out_tx.send(data.to_vec()).await.is_err() {
                        break;
                    }
                }
                ChannelMsg::ExtendedData { data, .. } => {
                    let mut s = err.lock().unwrap();
                    if s.len() < 8192 {
                        s.push_str(&String::from_utf8_lossy(&data));
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
                ChannelMsg::ExitSignal { .. } => code = code.or(Some(255)),
                ChannelMsg::Close => break,
                _ => {}
            }
        }
        writer.abort();
        drop(out_tx);
        let _ = exit_tx.send(code.unwrap_or(255));
    });

    Ok(Proc { tx: in_tx, rx: out_rx, stderr, exit: exit_rx })
}

/// Runs `cmd` with no input; returns (stdout, stderr).
pub async fn output(dest: &Dest, cmd: &str, timeout: Duration) -> Result<(String, String)> {
    let p = spawn(dest, Some(cmd), None).await?;
    let _ = p.tx.send(In::Eof);
    let Proc { tx, mut rx, stderr, .. } = p;
    let mut out = Vec::new();
    let collect = async {
        while let Some(d) = rx.recv().await {
            out.extend_from_slice(&d);
        }
    };
    if tokio::time::timeout(timeout, collect).await.is_err() {
        let _ = tx.send(In::Close);
        bail!("timed out");
    }
    let err = stderr.lock().unwrap().clone();
    Ok((String::from_utf8_lossy(&out).into_owned(), err))
}

// ---------- byte-stream adapters ----------

/// A Proc's output as a byte stream, async (host channel) or blocking (PTY reader thread).
pub struct Reader {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
}

impl Reader {
    pub fn new(rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Reader { rx, buf: vec![], pos: 0 }
    }

    fn take(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        n
    }
}

impl AsyncRead for Reader {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        if self.pos >= self.buf.len() {
            match self.rx.poll_recv(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Ready(Some(d)) => {
                    self.buf = d;
                    self.pos = 0;
                }
            }
        }
        let n = buf.remaining().min(self.buf.len() - self.pos);
        let pos = self.pos;
        buf.put_slice(&self.buf[pos..pos + n]);
        self.pos += n;
        Poll::Ready(Ok(()))
    }
}

impl std::io::Read for Reader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.buf.len() {
            match self.rx.blocking_recv() {
                None => return Ok(0),
                Some(d) => {
                    self.buf = d;
                    self.pos = 0;
                }
            }
        }
        Ok(self.take(out))
    }
}

/// A Proc's input as a byte sink. Writes never block.
pub struct Writer(pub mpsc::UnboundedSender<In>);

fn closed() -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "channel closed")
}

impl AsyncWrite for Writer {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Poll::Ready(self.0.send(In::Data(buf.to_vec())).map(|_| buf.len()).map_err(|_| closed()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let _ = self.0.send(In::Eof);
        Poll::Ready(Ok(()))
    }
}

impl std::io::Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.send(In::Data(buf.to_vec())).map(|_| buf.len()).map_err(|_| closed())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Fills unset fields of a destination: `ssh -G` on desktop reads
/// ~/.ssh/config exactly as ssh would.
#[cfg_attr(target_os = "android", allow(unused_mut))]
pub async fn resolve(alias: &str, address: Option<&str>, user: Option<&str>, port: Option<u16>) -> Result<Dest> {
    let mut identities = vec![];
    let (mut host, mut u, mut p) = match alias.split_once('@') {
        Some((u, h)) => (h.to_string(), Some(u.to_string()), None),
        None => (alias.to_string(), None, None),
    };
    #[cfg(not(target_os = "android"))]
    {
        let mut cmd = tokio::process::Command::new("ssh");
        cmd.arg("-G").arg(alias).stdin(std::process::Stdio::null()).kill_on_drop(true);
        crate::channel::no_window(&mut cmd);
        if let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(5), cmd.output()).await {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                match line.split_once(' ') {
                    Some(("hostname", v)) => host = v.trim().to_string(),
                    Some(("user", v)) => u = Some(v.trim().to_string()),
                    Some(("port", v)) => p = v.trim().parse().ok(),
                    Some(("identityfile", v)) => identities.push(expand_home(v.trim())),
                    _ => {}
                }
            }
        }
    }
    let host = address.map(str::to_string).unwrap_or(host);
    let user = user.map(str::to_string).or(u).ok_or_else(|| anyhow!("no user set for {alias}"))?;
    Ok(Dest { host, port: port.or(p).unwrap_or(22), user, identities })
}

#[cfg(not(target_os = "android"))]
fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => dirs::home_dir().unwrap_or_default().join(rest),
        None => PathBuf::from(p),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_hosts_lookup() {
        let text = "a ssh-ed25519 AAA\n[b]:2222 ssh-ed25519 BBB\n";
        assert!(matches!(check_known(text, "a", "ssh-ed25519 AAA"), HostKey::Known));
        assert!(matches!(check_known(text, "a", "ssh-ed25519 XXX"), HostKey::Changed));
        assert!(matches!(check_known(text, "[b]:2222", "ssh-ed25519 BBB"), HostKey::Known));
        assert!(matches!(check_known(text, "c", "ssh-ed25519 AAA"), HostKey::New));
    }

    #[test]
    fn known_name_has_port_only_when_not_22() {
        let d = |p| Dest { host: "h".into(), port: p, user: "u".into(), identities: vec![] };
        assert_eq!(d(22).known_name(), "h");
        assert_eq!(d(2222).known_name(), "[h]:2222");
    }

    #[test]
    fn key_round_trip() {
        let mut k = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        k.set_comment("myterm");
        let text = k.to_openssh(russh::keys::ssh_key::LineEnding::LF).unwrap();
        let back = PrivateKey::from_openssh(text.as_bytes()).unwrap();
        assert_eq!(key_text(back.public_key()), key_text(k.public_key()));
    }
}
