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
    killer: StdMutex<Option<crate::channel::Killer>>,
    pub hello: Hello,
}

impl KeeperConn {
    async fn start(r: Box<dyn AsyncRead + Send + Unpin>, w: Writer, killer: Option<crate::channel::Killer>, timeout: Duration) -> Result<KeeperConn> {
        let mut conn = KeeperConn {
            io: Mutex::new((BufReader::new(r), w)),
            dead: AtomicBool::new(false),
            killer: StdMutex::new(killer),
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

    /// Runs `remote_cmd` (which starts `myterm-cli channel`) on a remote host.
    pub async fn remote(via: &crate::channel::Via, remote_cmd: &str) -> Result<KeeperConn> {
        let pipe = crate::channel::batch(via, remote_cmd).await?;
        let err = pipe.stderr.clone();
        // Starting the keeper on the host (scheduled task or WMI) can take a while.
        match Self::start(pipe.stdout, pipe.stdin, Some(pipe.killer), Duration::from_secs(40)).await {
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
        if let Some(mut k) = self.killer.lock().unwrap().take() {
            k.kill();
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
