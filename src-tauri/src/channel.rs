//! Host channel (section 5.1): one long-lived non-interactive shell per host.
//! Requests are shell snippets; each reply is framed by a sentinel line that
//! carries the request id and the exit code.

use anyhow::{anyhow, bail, Result};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use crate::sshlib::{self, Dest, In};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

pub const SSH_OPTS: &[&str] = &[
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=2",
];

/// Single-quote a string for a POSIX shell.
pub fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn no_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    {
        let _ = cmd;
    }
}

pub fn ssh_batch_command(host: &str) -> Command {
    let mut cmd = Command::new("ssh");
    cmd.args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .args(SSH_OPTS)
        .arg(host);
    no_window(&mut cmd);
    cmd
}

/// How a remote host is reached: the system `ssh`, or the built-in client (14.4).
#[derive(Clone, Debug)]
pub enum Via {
    System(String),
    Builtin(Dest),
}

pub enum Target {
    Remote(Via),
    LocalShell,
}

/// Something to stop when a channel is done: a child process or an SSH channel.
pub enum Killer {
    Child(Box<Child>),
    Builtin(tokio::sync::mpsc::UnboundedSender<In>),
}

impl Killer {
    pub fn kill(&mut self) {
        match self {
            Killer::Child(c) => {
                let _ = c.start_kill();
            }
            Killer::Builtin(tx) => {
                let _ = tx.send(In::Close);
            }
        }
    }
}

/// A non-interactive remote command: its stdin, stdout and collected stderr.
pub struct Pipe {
    pub stdin: Box<dyn AsyncWrite + Send + Unpin>,
    pub stdout: Box<dyn AsyncRead + Send + Unpin>,
    pub stderr: Arc<StdMutex<String>>,
    pub killer: Killer,
}

fn pipe_from(mut cmd: Command) -> Result<Pipe> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
    let err_buf = Arc::new(StdMutex::new(String::new()));
    let eb = err_buf.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 2048];
        while let Ok(n) = stderr.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let mut s = eb.lock().unwrap();
            s.push_str(&String::from_utf8_lossy(&buf[..n]));
            if s.len() > 8192 {
                let cut = s.len() - 4096;
                let cut = (cut..s.len()).find(|i| s.is_char_boundary(*i)).unwrap_or(0);
                s.drain(..cut);
            }
        }
    });
    Ok(Pipe { stdin: Box::new(stdin), stdout: Box::new(stdout), stderr: err_buf, killer: Killer::Child(Box::new(child)) })
}

/// Starts `remote_cmd` on a host, with no terminal, for a long-lived channel.
pub async fn batch(via: &Via, remote_cmd: &str) -> Result<Pipe> {
    match via {
        Via::System(host) => {
            let mut cmd = ssh_batch_command(host);
            cmd.arg(remote_cmd);
            pipe_from(cmd)
        }
        Via::Builtin(dest) => {
            let p = sshlib::spawn(dest, Some(remote_cmd), None).await?;
            Ok(Pipe {
                stdin: Box::new(sshlib::Writer(p.tx.clone())),
                stdout: Box::new(sshlib::Reader::new(p.rx)),
                stderr: p.stderr,
                killer: Killer::Builtin(p.tx),
            })
        }
    }
}

/// Runs `remote_cmd` on a host with no input; returns (stdout, stderr).
pub async fn output(via: &Via, remote_cmd: &str, timeout: Duration) -> Result<(String, String)> {
    match via {
        Via::System(host) => {
            let mut cmd = ssh_batch_command(host);
            cmd.arg(remote_cmd).stdin(Stdio::null()).kill_on_drop(true);
            let out = tokio::time::timeout(timeout, cmd.output()).await.map_err(|_| anyhow!("timed out"))??;
            Ok((String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned()))
        }
        Via::Builtin(dest) => sshlib::output(dest, remote_cmd, timeout).await,
    }
}

struct Io {
    stdin: Box<dyn AsyncWrite + Send + Unpin>,
    stdout: BufReader<Box<dyn AsyncRead + Send + Unpin>>,
}

pub struct Channel {
    io: Mutex<Io>,
    killer: StdMutex<Option<Killer>>,
    stderr: Arc<StdMutex<String>>,
    dead: AtomicBool,
    next_id: AtomicU64,
}

pub struct Reply {
    pub code: i32,
    pub out: String,
}

impl Channel {
    pub async fn spawn(target: &Target) -> Result<Arc<Channel>> {
        let pipe = match target {
            Target::Remote(via) => batch(via, "/bin/sh").await?,
            Target::LocalShell => {
                let mut c = Command::new("/bin/sh");
                no_window(&mut c);
                pipe_from(c)?
            }
        };
        Ok(Arc::new(Channel {
            io: Mutex::new(Io { stdin: pipe.stdin, stdout: BufReader::new(pipe.stdout) }),
            killer: StdMutex::new(Some(pipe.killer)),
            stderr: pipe.stderr,
            dead: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
        }))
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    pub fn stderr_text(&self) -> String {
        self.stderr.lock().unwrap().clone()
    }

    pub fn kill(&self) {
        self.dead.store(true, Ordering::SeqCst);
        if let Some(mut k) = self.killer.lock().unwrap().take() {
            k.kill();
        }
    }

    /// Runs a snippet. Its stdin is /dev/null and stderr is merged into the output.
    pub async fn run(&self, script: &str, timeout: Duration) -> Result<Reply> {
        if self.is_dead() {
            bail!("channel closed");
        }
        let mut io = self.io.lock().await;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let framed = format!(
            "{{\n{script}\n}} </dev/null 2>&1; printf '\\n__MT_END__ {id} %s\\n' \"$?\"\n"
        );
        let res = tokio::time::timeout(timeout, async {
            io.stdin.write_all(framed.as_bytes()).await?;
            io.stdin.flush().await?;
            let marker = format!("__MT_END__ {id} ");
            let mut lines: Vec<String> = vec![];
            let mut buf = Vec::new();
            loop {
                buf.clear();
                let n = io.stdout.read_until(b'\n', &mut buf).await?;
                if n == 0 {
                    bail!("channel closed");
                }
                let line = String::from_utf8_lossy(&buf);
                let line = line.trim_end_matches(['\n', '\r']);
                if let Some(code) = line.strip_prefix(&marker) {
                    let code = code.trim().parse().unwrap_or(-1);
                    if lines.last().is_some_and(|l| l.is_empty()) {
                        lines.pop();
                    }
                    return Ok(Reply { code, out: lines.join("\n") });
                }
                lines.push(line.to_string());
            }
        })
        .await;
        match res {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => {
                drop(io);
                self.kill();
                Err(e)
            }
            Err(_) => {
                drop(io);
                self.kill();
                Err(anyhow!("request timed out"))
            }
        }
    }
}

/// Result of probing a host whose /bin/sh channel could not start.
pub enum Probe {
    Windows { powershell: bool },
    Other(String),
}

/// Asks the host's default shell to identify itself. sh leaves `%OS%` alone,
/// cmd expands it to Windows_NT, PowerShell splits the words onto lines.
pub async fn probe(via: &Via) -> Probe {
    let (out, err) = match output(via, "echo MTPROBE %OS% $OSTYPE", Duration::from_secs(15)).await {
        Ok(r) => r,
        Err(e) => return Probe::Other(e.to_string()),
    };
    let first = out.lines().map(str::trim).find(|l| l.starts_with("MTPROBE"));
    match first {
        Some(l) if l.contains("Windows_NT") => Probe::Windows { powershell: false },
        Some("MTPROBE") => Probe::Windows { powershell: true },
        _ => Probe::Other(err.trim().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::q;

    #[test]
    fn quoting() {
        assert_eq!(q("a b"), "'a b'");
        assert_eq!(q("it's"), "'it'\\''s'");
    }
}
