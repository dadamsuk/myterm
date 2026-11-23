//! Host channel (section 5.1): one long-lived non-interactive shell per host.
//! Requests are shell snippets; each reply is framed by a sentinel line that
//! carries the request id and the exit code.

use anyhow::{anyhow, bail, Result};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
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

pub enum Target {
    Ssh(String),
    LocalShell,
}

struct Io {
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

pub struct Channel {
    io: Mutex<Io>,
    child: StdMutex<Option<Child>>,
    stderr: Arc<StdMutex<String>>,
    dead: AtomicBool,
    next_id: AtomicU64,
}

pub struct Reply {
    pub code: i32,
    pub out: String,
}

impl Channel {
    pub fn spawn(target: &Target) -> Result<Arc<Channel>> {
        let mut cmd = match target {
            Target::Ssh(host) => {
                let mut c = ssh_batch_command(host);
                c.arg("/bin/sh");
                c
            }
            Target::LocalShell => {
                let mut c = Command::new("/bin/sh");
                no_window(&mut c);
                c
            }
        };
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
        Ok(Arc::new(Channel {
            io: Mutex::new(Io { stdin, stdout: BufReader::new(stdout) }),
            child: StdMutex::new(Some(child)),
            stderr: err_buf,
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
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.start_kill();
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
pub async fn probe(host: &str) -> Probe {
    let mut cmd = ssh_batch_command(host);
    cmd.arg("echo MTPROBE %OS% $OSTYPE")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let out = match tokio::time::timeout(Duration::from_secs(15), cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Probe::Other(e.to_string()),
        Err(_) => return Probe::Other("timed out".into()),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().map(str::trim).find(|l| l.starts_with("MTPROBE"));
    match first {
        Some(l) if l.contains("Windows_NT") => Probe::Windows { powershell: false },
        Some("MTPROBE") => Probe::Windows { powershell: true },
        _ => Probe::Other(String::from_utf8_lossy(&out.stderr).trim().to_string()),
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
