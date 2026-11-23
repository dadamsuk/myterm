//! PTY manager (sections 5.3, 7): one PTY per open terminal. Output is sent
//! to the frontend as binary, batched per frame, with ack-based backpressure.

use anyhow::{anyhow, Result};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use serde_json::json;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::{Channel, InvokeResponseBody};

/// Unacknowledged bytes above which reading from the PTY pauses.
const HIGH_WATER: usize = 2 * 1024 * 1024;
const BATCH_WINDOW: Duration = Duration::from_millis(6);
const BATCH_MAX: usize = 512 * 1024;

struct Flow {
    pending: Mutex<usize>,
    cv: Condvar,
    closed: AtomicBool,
}

struct Entry {
    writer: Mutex<Box<dyn Write + Send>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
    flow: Arc<Flow>,
}

#[derive(Default)]
pub struct PtyManager {
    ptys: Mutex<HashMap<u32, Arc<Entry>>>,
    next: AtomicU32,
}

pub type OnExit = Box<dyn FnOnce(u32) + Send>;

impl PtyManager {
    pub fn spawn(
        self: &Arc<Self>,
        cmd: CommandBuilder,
        cols: u16,
        rows: u16,
        chan: Channel<InvokeResponseBody>,
        on_exit: Option<OnExit>,
    ) -> Result<u32> {
        let size = PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 };
        let pair = native_pty_system().openpty(size)?;
        let mut child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let id = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let flow = Arc::new(Flow { pending: Mutex::new(0), cv: Condvar::new(), closed: AtomicBool::new(false) });
        let entry = Arc::new(Entry {
            writer: Mutex::new(writer),
            master: Mutex::new(pair.master),
            killer: Mutex::new(killer),
            flow: flow.clone(),
        });
        self.ptys.lock().unwrap().insert(id, entry);

        // Reader: blocking reads, paused while too much output is unacknowledged.
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let rflow = flow.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                {
                    let mut p = rflow.pending.lock().unwrap();
                    let started = Instant::now();
                    while *p > HIGH_WATER && !rflow.closed.load(Ordering::SeqCst) {
                        let (g, _) = rflow.cv.wait_timeout(p, Duration::from_millis(500)).unwrap();
                        p = g;
                        // A frontend that stopped acking (reload) must not wedge the PTY.
                        if started.elapsed() > Duration::from_secs(10) {
                            *p = 0;
                        }
                    }
                }
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // Sender: coalesces reads that arrive within one batch window.
        let sflow = flow.clone();
        let schan = chan.clone();
        std::thread::spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut out = first;
                let deadline = Instant::now() + BATCH_WINDOW;
                while out.len() < BATCH_MAX {
                    let left = deadline.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(left) {
                        Ok(more) => out.extend_from_slice(&more),
                        Err(_) => break,
                    }
                }
                *sflow.pending.lock().unwrap() += out.len();
                if schan.send(InvokeResponseBody::Raw(out)).is_err() {
                    break;
                }
            }
        });

        // Waiter: reports the exit code, then drops the PTY so the reader ends.
        let mgr = self.clone();
        std::thread::spawn(move || {
            let code = child.wait().map(|s| s.exit_code()).unwrap_or(1);
            std::thread::sleep(Duration::from_millis(150));
            let _ = chan.send(InvokeResponseBody::Json(json!({ "exit": code }).to_string()));
            mgr.remove(id);
            if let Some(f) = on_exit {
                f(code);
            }
        });
        Ok(id)
    }

    fn get(&self, id: u32) -> Result<Arc<Entry>> {
        self.ptys.lock().unwrap().get(&id).cloned().ok_or_else(|| anyhow!("no such terminal"))
    }

    fn remove(&self, id: u32) {
        if let Some(e) = self.ptys.lock().unwrap().remove(&id) {
            e.flow.closed.store(true, Ordering::SeqCst);
            e.flow.cv.notify_all();
        }
    }

    pub fn write(&self, id: u32, data: &[u8]) -> Result<()> {
        let e = self.get(id)?;
        let mut w = e.writer.lock().unwrap();
        w.write_all(data)?;
        w.flush()?;
        Ok(())
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<()> {
        let e = self.get(id)?;
        let m = e.master.lock().unwrap();
        m.resize(PtySize { rows: rows.max(2), cols: cols.max(10), pixel_width: 0, pixel_height: 0 })?;
        Ok(())
    }

    pub fn ack(&self, id: u32, bytes: usize) {
        if let Ok(e) = self.get(id) {
            let mut p = e.flow.pending.lock().unwrap();
            *p = p.saturating_sub(bytes);
            e.flow.cv.notify_all();
        }
    }

    pub fn close(&self, id: u32) {
        if let Ok(e) = self.get(id) {
            let _ = e.killer.lock().unwrap().kill();
        }
        self.remove(id);
    }

    pub fn close_all(&self) {
        let ids: Vec<u32> = self.ptys.lock().unwrap().keys().copied().collect();
        for id in ids {
            self.close(id);
        }
    }
}
