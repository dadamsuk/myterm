//! A control connection to a keeper: the local pipe, or `myterm-cli channel`
//! over `ssh -T` for a remote Windows host. Requests are serialised.

use super::{Hello, KSession, PROTO};
use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

type Reader = BufReader<Box<dyn AsyncRead + Send + Unpin>>;
type Writer = Box<dyn AsyncWrite + Send + Unpin>;

pub struct KeeperConn {
    io: Mutex<(Reader, Writer)>,
    dead: AtomicBool,
    child: StdMutex<Option<tokio::process::Child>>,
    pub hello: Hello,
}

impl KeeperConn {
    async fn start(r: Box<dyn AsyncRead + Send + Unpin>, w: Writer, child: Option<tokio::process::Child>, timeout: Duration) -> Result<KeeperConn> {
        let mut conn = KeeperConn {
            io: Mutex::new((BufReader::new(r), w)),
            dead: AtomicBool::new(false),
            child: StdMutex::new(child),
            hello: Hello::default(),
        };
        let v = conn.request(json!({ "op": "hello", "proto": PROTO }), timeout).await?;
        conn.hello = serde_json::from_value(v["hello"].clone())?;
        Ok(conn)
    }

    /// Connects to this machine's keeper.
    #[cfg(windows)]
    pub async fn local() -> Result<KeeperConn> {
        let pipe = super::server::pipe_name();
        let c = super::cli::open_pipe(&pipe).await?;
        let (r, w) = tokio::io::split(c);
        Self::start(Box::new(r), Box::new(w), None, Duration::from_secs(5)).await
    }

    /// Runs `remote_cmd` (which starts `myterm-cli channel`) on `host` over ssh.
    pub async fn ssh(host: &str, remote_cmd: &str) -> Result<KeeperConn> {
        let mut cmd = crate::channel::ssh_batch_command(host);
        cmd.arg(remote_cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let mut stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;
        let err = std::sync::Arc::new(StdMutex::new(String::new()));
        let e2 = err.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut b = [0u8; 1024];
            while let Ok(n) = stderr.read(&mut b).await {
                if n == 0 {
                    break;
                }
                let mut s = e2.lock().unwrap();
                if s.len() < 4096 {
                    s.push_str(&String::from_utf8_lossy(&b[..n]));
                }
            }
        });
        // Starting the keeper on the host (scheduled task or WMI) can take a while.
        match Self::start(Box::new(stdout), Box::new(stdin), Some(child), Duration::from_secs(40)).await {
            Ok(c) => Ok(c),
            Err(e) => {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let msg = err.lock().unwrap().trim().to_string();
                let last = msg.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string();
                if last.is_empty() { Err(e) } else { Err(anyhow!("{last}")) }
            }
        }
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    pub fn kill(&self) {
        self.dead.store(true, Ordering::SeqCst);
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.start_kill();
        }
    }

    /// Sends one request; returns the reply object, or its error.
    pub async fn request(&self, req: Value, timeout: Duration) -> Result<Value> {
        if self.is_dead() {
            bail!("keeper connection closed");
        }
        let mut io = self.io.lock().await;
        let res = tokio::time::timeout(timeout, async {
            let (r, w) = &mut *io;
            w.write_all(format!("{req}\n").as_bytes()).await?;
            w.flush().await?;
            let mut line = String::new();
            loop {
                line.clear();
                if r.read_line(&mut line).await? == 0 {
                    bail!("keeper connection closed");
                }
                // Anything before the JSON (a shell banner, say) is skipped.
                if line.trim_start().starts_with('{') {
                    break;
                }
            }
            Ok::<Value, anyhow::Error>(serde_json::from_str(line.trim())?)
        })
        .await;
        let v = match res {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                drop(io);
                self.kill();
                return Err(e);
            }
            Err(_) => {
                drop(io);
                self.kill();
                bail!("keeper did not answer");
            }
        };
        if v["ok"].as_bool() != Some(true) {
            bail!("{}", v["error"].as_str().unwrap_or("keeper error"));
        }
        Ok(v)
    }

    pub async fn list(&self) -> Result<Vec<KSession>> {
        let v = self.request(json!({ "op": "list" }), Duration::from_secs(15)).await?;
        Ok(serde_json::from_value(v["sessions"].clone())?)
    }
}
