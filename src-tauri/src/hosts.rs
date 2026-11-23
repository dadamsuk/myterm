//! Hosts, their channels, discovery polling, and session operations.

use crate::channel::{self, q, Channel, Probe, Target};
use crate::config::{self, now_secs, Config, State};
use crate::hooks;
use crate::pty::PtyManager;
use crate::sshconfig;
use crate::tmux::{self, ListResult, Session};
use anyhow::{anyhow, bail, Result};
use portable_pty::CommandBuilder;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tauri::ipc::{Channel as IpcChannel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tokio::sync::Notify;

pub const LOCAL: &str = "local";
const BACKOFF: &[u64] = &[1, 2, 5, 10, 30];
const CMD_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Serialize, Clone, Debug, PartialEq, Default)]
pub struct HostInfo {
    pub alias: String,
    /// "tmux" | "ephemeral" | "unavailable"
    pub kind: String,
    pub connected: bool,
    pub connecting: bool,
    pub status: String,
    pub needs_login: bool,
    pub os: String,
    pub tmux: Option<String>,
    pub tmux_version: Option<String>,
    pub home: Option<String>,
    /// Remote login name, from the bootstrap.
    pub user: Option<String>,
    pub hidden: bool,
    pub local: bool,
    pub win_powershell: bool,
    pub hooks_installed: bool,
}

pub struct Host {
    pub alias: String,
    pub is_local: bool,
    pub info: Mutex<HostInfo>,
    pub channel: Mutex<Option<Arc<Channel>>>,
    pub sessions: Mutex<Vec<Session>>,
    pub wake: Notify,
    pub removed: AtomicBool,
}

#[derive(Clone, Debug)]
pub struct Ephemeral {
    pub id: u64,
    pub host: String,
    pub name: String,
    pub dir: String,
    pub kind: String,
    pub created: i64,
    pub ended: bool,
    pub state: Option<String>,
    pub state_at: i64,
}

impl Ephemeral {
    fn to_session(&self) -> Session {
        let slug = format!("~eph-{}", self.id);
        Session {
            key: format!("{}/{}", self.host, slug),
            host: self.host.clone(),
            slug,
            display_name: self.name.clone(),
            kind: self.kind.clone(),
            cwd: self.dir.clone(),
            state: self.state.clone(),
            state_at: self.state_at,
            created: self.created,
            last_attached: self.created,
            attached: 1,
            persistent: false,
            ephemeral_id: Some(self.id),
            ended: self.ended,
            windows: vec![],
        }
    }
}

#[derive(Serialize, Clone)]
pub struct Snapshot {
    pub hosts: Vec<HostInfo>,
    pub sessions: Vec<Session>,
}

pub struct App {
    pub handle: AppHandle,
    pub config: RwLock<Config>,
    pub config_error: Mutex<Option<String>>,
    pub state: Mutex<State>,
    pub state_dirty: AtomicBool,
    pub hosts: Mutex<Vec<Arc<Host>>>,
    pub ephemerals: Mutex<Vec<Ephemeral>>,
    pub ptys: Arc<PtyManager>,
    next_eph: AtomicU64,
}

fn local_is_windows() -> bool {
    cfg!(windows)
}

impl App {
    pub fn new(handle: AppHandle) -> Arc<App> {
        let (cfg, err) = config::load_config();
        let base = (now_secs() as u64) * 1000;
        Arc::new(App {
            handle,
            config: RwLock::new(cfg),
            config_error: Mutex::new(err),
            state: Mutex::new(config::load_state()),
            state_dirty: AtomicBool::new(false),
            hosts: Mutex::new(vec![]),
            ephemerals: Mutex::new(vec![]),
            ptys: Arc::new(PtyManager::default()),
            next_eph: AtomicU64::new(base),
        })
    }

    pub fn cfg(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    pub fn mark_state_dirty(&self) {
        self.state_dirty.store(true, Ordering::SeqCst);
    }

    pub fn snapshot(&self) -> Snapshot {
        let hosts = self.hosts.lock().unwrap().clone();
        let mut infos = vec![];
        let mut sessions = vec![];
        for h in &hosts {
            infos.push(h.info.lock().unwrap().clone());
            sessions.extend(h.sessions.lock().unwrap().iter().cloned());
        }
        sessions.extend(self.ephemerals.lock().unwrap().iter().map(Ephemeral::to_session));
        Snapshot { hosts: infos, sessions }
    }

    pub fn emit_snapshot(&self) {
        let _ = self.handle.emit("snapshot", self.snapshot());
    }

    pub fn host(&self, alias: &str) -> Result<Arc<Host>> {
        self.hosts
            .lock()
            .unwrap()
            .iter()
            .find(|h| h.alias == alias)
            .cloned()
            .ok_or_else(|| anyhow!("unknown host {alias}"))
    }

    /// Re-reads ~/.ssh/config and the hidden list; starts or stops host tasks.
    pub fn sync_hosts(self: &Arc<Self>) -> Vec<std::path::PathBuf> {
        let parsed = sshconfig::parse();
        let cfg = self.cfg();
        let mut wanted = vec![LOCAL.to_string()];
        for h in parsed.hosts {
            if Some(&h) != cfg.local_alias.as_ref() && h != LOCAL {
                wanted.push(h);
            }
        }
        let mut hosts = self.hosts.lock().unwrap();
        hosts.retain(|h| {
            let keep = wanted.contains(&h.alias);
            if !keep {
                h.removed.store(true, Ordering::SeqCst);
                if let Some(c) = h.channel.lock().unwrap().take() {
                    c.kill();
                }
                h.wake.notify_one();
            }
            keep
        });
        let mut ordered = vec![];
        for alias in &wanted {
            let hidden = cfg.hidden_hosts.contains(alias);
            if let Some(h) = hosts.iter().find(|h| &h.alias == alias) {
                let changed = {
                    let mut i = h.info.lock().unwrap();
                    let c = i.hidden != hidden;
                    i.hidden = hidden;
                    c
                };
                if changed {
                    if hidden {
                        if let Some(c) = h.channel.lock().unwrap().take() {
                            c.kill();
                        }
                    }
                    h.wake.notify_one();
                }
                ordered.push(h.clone());
                continue;
            }
            let is_local = alias == LOCAL;
            let h = Arc::new(Host {
                alias: alias.clone(),
                is_local,
                info: Mutex::new(HostInfo {
                    alias: alias.clone(),
                    kind: "unavailable".into(),
                    connecting: !hidden,
                    status: if hidden { "hidden".into() } else { "connecting".into() },
                    hidden,
                    local: is_local,
                    user: if is_local { std::env::var("USERNAME").or_else(|_| std::env::var("USER")).ok() } else { None },
                    ..Default::default()
                }),
                channel: Mutex::new(None),
                sessions: Mutex::new(vec![]),
                wake: Notify::new(),
                removed: AtomicBool::new(false),
            });
            let app = self.clone();
            let hh = h.clone();
            // Stagger first connections so a dozen ssh handshakes don't start at once.
            let delay = Duration::from_millis(200 * ordered.len() as u64);
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(delay).await;
                host_task(app, hh).await
            });
            ordered.push(h);
        }
        *hosts = ordered;
        drop(hosts);
        self.emit_snapshot();
        parsed.files
    }

    fn set_info(&self, h: &Host, f: impl FnOnce(&mut HostInfo)) {
        let changed = {
            let mut i = h.info.lock().unwrap();
            let before = i.clone();
            f(&mut i);
            *i != before
        };
        if changed {
            self.emit_snapshot();
        }
    }

    fn remember_folders(&self, host: &str, sessions: &[Session]) {
        let mut st = self.state.lock().unwrap();
        let folders = st.known_folders.entry(host.to_string()).or_default();
        let mut dirty = false;
        for s in sessions {
            if s.cwd.is_empty() {
                continue;
            }
            let t = s.last_attached.max(s.created);
            let e = folders.entry(s.cwd.clone()).or_insert_with(|| {
                dirty = true;
                t
            });
            if t > *e {
                *e = t;
                dirty = true;
            }
        }
        drop(st);
        if dirty {
            self.mark_state_dirty();
        }
    }

    pub fn touch_folder(&self, host: &str, dir: &str) {
        self.state
            .lock()
            .unwrap()
            .known_folders
            .entry(host.to_string())
            .or_default()
            .insert(dir.to_string(), now_secs());
        self.mark_state_dirty();
    }

    // ---------- channel helpers ----------

    fn chan(&self, h: &Host) -> Result<Arc<Channel>> {
        h.channel
            .lock()
            .unwrap()
            .clone()
            .filter(|c| !c.is_dead())
            .ok_or_else(|| anyhow!("{} is not connected", h.alias))
    }

    fn tmux_of(&self, h: &Host) -> Result<String> {
        h.info.lock().unwrap().tmux.clone().ok_or_else(|| anyhow!("no tmux on {}", h.alias))
    }

    async fn run(&self, h: &Host, script: &str) -> Result<String> {
        let r = self.chan(h)?.run(script, CMD_TIMEOUT).await?;
        if r.code != 0 {
            bail!("{}", if r.out.trim().is_empty() { format!("exit {}", r.code) } else { r.out.trim().to_string() });
        }
        Ok(r.out)
    }

    /// Re-polls a host right away so the list reflects a change without waiting.
    async fn refresh(&self, h: &Host) {
        if let (Ok(c), Ok(t)) = (self.chan(h), self.tmux_of(h)) {
            if let Ok(r) = c.run(&tmux::list_script(&t), CMD_TIMEOUT).await {
                if let ListResult::Sessions(s) = tmux::parse_list(&h.alias, r.code, &r.out) {
                    *h.sessions.lock().unwrap() = s;
                    self.emit_snapshot();
                }
            }
        }
    }

    // ---------- session operations ----------

    pub async fn create_session(&self, host: &str, dir: &str, name: &str, kind: &str) -> Result<String> {
        let h = self.host(host)?;
        let home = h.info.lock().unwrap().home.clone().unwrap_or_default();
        let dir = tmux::expand_home(dir.trim(), &home);
        let dir = if dir.is_empty() { home.clone() } else { dir };
        let t = self.tmux_of(&h)?;
        let cfg = self.cfg();
        let out = self.run(&h, &tmux::create_script(&t, name, &dir, kind, cfg.hide_tmux_status)).await?;
        let slug = out
            .lines()
            .find_map(|l| l.strip_prefix("SLUG="))
            .map(str::to_string)
            .ok_or_else(|| anyhow!("create failed: {out}"))?;
        self.touch_folder(host, &dir);
        self.refresh(&h).await;
        Ok(slug)
    }

    /// Adds a window to an existing tmux session; returns the window id.
    pub async fn create_window(&self, host: &str, slug: &str, dir: &str, name: &str, kind: &str) -> Result<String> {
        let h = self.host(host)?;
        let home = h.info.lock().unwrap().home.clone().unwrap_or_default();
        let dir = tmux::expand_home(dir.trim(), &home);
        let dir = if dir.is_empty() { home.clone() } else { dir };
        let t = self.tmux_of(&h)?;
        let out = self.run(&h, &tmux::new_window_script(&t, slug, name, &dir, kind)).await?;
        let id = out
            .lines()
            .find_map(|l| l.strip_prefix("WIN="))
            .filter(|s| s.starts_with('@'))
            .map(str::to_string)
            .ok_or_else(|| anyhow!("new window failed: {out}"))?;
        self.touch_folder(host, &dir);
        self.refresh(&h).await;
        Ok(id)
    }

    pub async fn rename_window(&self, host: &str, slug: &str, window_id: &str, name: &str) -> Result<()> {
        let h = self.host(host)?;
        let t = self.tmux_of(&h)?;
        self.run(&h, &tmux::rename_window_script(&t, slug, window_id, name)).await?;
        self.refresh(&h).await;
        Ok(())
    }

    pub async fn kill_window(&self, host: &str, slug: &str, window_id: &str) -> Result<()> {
        let h = self.host(host)?;
        let t = self.tmux_of(&h)?;
        self.run(&h, &tmux::kill_window_script(&t, slug, window_id)).await?;
        self.refresh(&h).await;
        Ok(())
    }

    pub async fn rename_session(&self, host: &str, slug: &str, name: &str) -> Result<()> {
        if let Some(id) = slug.strip_prefix("~eph-").and_then(|s| s.parse::<u64>().ok()) {
            if let Some(e) = self.ephemerals.lock().unwrap().iter_mut().find(|e| e.id == id) {
                e.name = name.to_string();
            }
            self.emit_snapshot();
            return Ok(());
        }
        let h = self.host(host)?;
        let t = self.tmux_of(&h)?;
        self.run(&h, &tmux::rename_script(&t, slug, name)).await?;
        self.refresh(&h).await;
        Ok(())
    }

    pub async fn kill_session(&self, host: &str, slug: &str) -> Result<()> {
        if let Some(id) = slug.strip_prefix("~eph-").and_then(|s| s.parse::<u64>().ok()) {
            self.ephemerals.lock().unwrap().retain(|e| e.id != id);
            remove_eph_state_file(id);
            self.emit_snapshot();
            return Ok(()); // the frontend closes the PTY
        }
        let h = self.host(host)?;
        let t = self.tmux_of(&h)?;
        self.run(&h, &tmux::kill_script(&t, slug)).await?;
        self.refresh(&h).await;
        Ok(())
    }

    /// Makes a window the session's current one; every attached client follows.
    pub async fn select_window(&self, host: &str, slug: &str, window_id: &str) -> Result<()> {
        let h = self.host(host)?;
        let t = self.tmux_of(&h)?;
        self.run(&h, &tmux::select_window_script(&t, slug, window_id)).await?;
        self.refresh(&h).await;
        Ok(())
    }

    pub async fn complete_path(&self, host: &str, prefix: &str) -> Result<Vec<String>> {
        let h = self.host(host)?;
        if h.is_local && local_is_windows() {
            return Ok(complete_local(prefix));
        }
        let home = h.info.lock().unwrap().home.clone().unwrap_or_default();
        let expanded = tmux::expand_home(prefix, &home);
        let out = self.run(&h, &tmux::complete_script(&expanded)).await.unwrap_or_default();
        // Show results the way they were typed (keep a leading ~).
        let tilde = prefix.starts_with('~') && !home.is_empty();
        Ok(out
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                if tilde && l.starts_with(&home) {
                    format!("~{}", &l[home.len()..])
                } else {
                    l.to_string()
                }
            })
            .collect())
    }

    pub fn attach_command_text(&self, host: &str, slug: &str) -> Result<String> {
        let h = self.host(host)?;
        let t = self.tmux_of(&h)?;
        let inner = format!("{} attach-session -t {}", q(&t), q(&format!("={slug}")));
        Ok(if h.is_local { inner } else { format!("ssh -t {host} {}", q(&inner)) })
    }

    /// Builds the PTY command that attaches to a tmux session (5.3).
    pub async fn attach_command(&self, host: &str, slug: &str) -> Result<CommandBuilder> {
        let h = self.host(host)?;
        let t = self.tmux_of(&h)?;
        let cfg = self.cfg();
        let mouse = cfg.hosts.get(host).and_then(|o| o.mouse).unwrap_or(cfg.mouse);
        if mouse {
            let _ = self.run(&h, &tmux::mouse_script(&t, slug, true)).await;
        }
        let target = format!("={slug}");
        let mut cmd = if h.is_local {
            let mut c = CommandBuilder::new(&t);
            c.args(["-u", "-T", "RGB,clipboard", "attach-session", "-t", &target]);
            if let Some(home) = dirs::home_dir() {
                c.cwd(home);
            }
            c
        } else {
            let mut c = CommandBuilder::new("ssh");
            c.arg("-t");
            c.args(channel::SSH_OPTS);
            c.arg(host);
            c.arg(format!("{} -u -T RGB,clipboard attach-session -t {}", q(&t), q(&target)));
            c
        };
        term_env(&mut cmd);
        let key = format!("{host}/{slug}");
        self.state.lock().unwrap().last_viewed.insert(key, now_secs());
        self.mark_state_dirty();
        Ok(cmd)
    }

    pub fn login_command(&self, host: &str) -> CommandBuilder {
        let mut c = CommandBuilder::new("ssh");
        c.arg(host);
        term_env(&mut c);
        c
    }

    /// Starts an ephemeral session (5.6). Returns (session key, pty id, text to type).
    pub fn start_ephemeral(
        self: &Arc<Self>,
        host: &str,
        dir: &str,
        name: &str,
        kind: &str,
        cols: u16,
        rows: u16,
        chan: IpcChannel<InvokeResponseBody>,
    ) -> Result<(String, u32, String)> {
        let h = self.host(host)?;
        let id = self.next_eph.fetch_add(1, Ordering::SeqCst);
        let cfg = self.cfg();
        let (mut cmd, powershell) = if h.is_local {
            let shell = cfg.local_shell.clone().unwrap_or_else(default_windows_shell);
            let ps = shell.to_ascii_lowercase().contains("pwsh") || shell.to_ascii_lowercase().contains("powershell");
            let mut c = CommandBuilder::new(&shell);
            let d = expand_local(dir);
            if !d.is_empty() && std::path::Path::new(&d).is_dir() {
                c.cwd(&d);
            } else if let Some(home) = dirs::home_dir() {
                c.cwd(home);
            }
            c.env("MYTERM_SESSION", id.to_string());
            (c, ps)
        } else {
            let ps = h.info.lock().unwrap().win_powershell;
            let mut c = CommandBuilder::new("ssh");
            c.arg("-t");
            c.args(channel::SSH_OPTS);
            c.arg(host);
            (c, ps)
        };
        term_env(&mut cmd);
        let mut input = String::new();
        let d = dir.trim();
        if !h.is_local && !d.is_empty() {
            input += &if powershell {
                format!("Set-Location -LiteralPath '{}'; ", d.replace('\'', "''"))
            } else {
                format!("cd /d \"{}\" && ", d.replace('"', ""))
            };
        }
        if kind == "claude" {
            input += &if powershell {
                format!("claude --rc '{}'", name.replace('\'', "''"))
            } else {
                format!("claude --rc \"{}\"", name.replace('"', ""))
            };
        }
        if !input.is_empty() {
            input.push('\r');
        }
        let app = self.clone();
        let on_exit = Box::new(move |_code: u32| {
            if let Some(e) = app.ephemerals.lock().unwrap().iter_mut().find(|e| e.id == id) {
                e.ended = true;
                e.state = None;
            }
            remove_eph_state_file(id);
            app.emit_snapshot();
        });
        let pty = self.ptys.spawn(cmd, cols, rows, chan, Some(on_exit))?;
        let e = Ephemeral {
            id,
            host: host.to_string(),
            name: name.to_string(),
            dir: dir.to_string(),
            kind: kind.to_string(),
            created: now_secs(),
            ended: false,
            state: None,
            state_at: 0,
        };
        let key = e.to_session().key;
        self.ephemerals.lock().unwrap().push(e);
        if !dir.trim().is_empty() {
            self.touch_folder(host, dir.trim());
        }
        self.emit_snapshot();
        Ok((key, pty, input))
    }

    pub fn running_ephemerals(&self) -> usize {
        self.ephemerals.lock().unwrap().iter().filter(|e| !e.ended).count()
    }

    pub async fn reconnect(&self, host: &str) -> Result<()> {
        let h = self.host(host)?;
        if let Some(c) = h.channel.lock().unwrap().take() {
            c.kill();
        }
        h.wake.notify_one();
        Ok(())
    }

    // ---------- attention hooks (8.3) ----------

    pub async fn hooks_plan(&self, host: &str, install: bool) -> Result<HookPlan> {
        let h = self.host(host)?;
        let kind = h.info.lock().unwrap().kind.clone();
        if h.is_local && local_is_windows() {
            let home = dirs::home_dir().ok_or_else(|| anyhow!("no home directory"))?;
            let settings_path = home.join(".claude").join("settings.json");
            let script_path = home.join(".myterm").join("hook.cmd");
            let before = std::fs::read_to_string(&settings_path).unwrap_or_default();
            let cmd = format!("\"{}\"", script_path.to_string_lossy().replace('\\', "/"));
            let after = hooks::merge_settings(&before, &cmd, install)?;
            let script_before = std::fs::read_to_string(&script_path).unwrap_or_default();
            return Ok(HookPlan {
                host: host.into(),
                install,
                files: vec![
                    FileChange::new(script_path.to_string_lossy(), script_before, if install { hooks::HOOK_CMD.replace("\r\n", "\n") } else { String::new() }),
                    FileChange::new(settings_path.to_string_lossy(), before, after),
                ],
            });
        }
        if kind != "tmux" {
            bail!("attention hooks need a connected tmux host (Windows hosts other than this machine are planned for v1.1)");
        }
        let home = h.info.lock().unwrap().home.clone().unwrap_or_default();
        let t = self.tmux_of(&h)?;
        let out = self
            .run(&h, "if [ -f \"$HOME/.claude/settings.json\" ]; then echo EXISTS; cat \"$HOME/.claude/settings.json\"; else echo MISSING; fi")
            .await?;
        let before = out.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or_default();
        let script_path = format!("{home}/.myterm/hook.sh");
        let after = hooks::merge_settings(&before, &q(&script_path), install)?;
        let script_before = self
            .run(&h, "cat \"$HOME/.myterm/hook.sh\" 2>/dev/null; true")
            .await
            .unwrap_or_default();
        Ok(HookPlan {
            host: host.into(),
            install,
            files: vec![
                FileChange::new(script_path, script_before, if install { hooks::hook_sh(&t) } else { String::new() }),
                FileChange::new(format!("{home}/.claude/settings.json"), before, after),
            ],
        })
    }

    pub async fn hooks_apply(&self, host: &str, install: bool) -> Result<()> {
        let plan = self.hooks_plan(host, install).await?;
        let h = self.host(host)?;
        let script = &plan.files[0];
        let settings = &plan.files[1];
        if h.is_local && local_is_windows() {
            let sp = std::path::Path::new(&script.path);
            let st = std::path::Path::new(&settings.path);
            if install {
                std::fs::create_dir_all(sp.parent().unwrap())?;
                std::fs::write(sp, hooks::HOOK_CMD)?;
            } else {
                let _ = std::fs::remove_file(sp);
            }
            std::fs::create_dir_all(st.parent().unwrap())?;
            if st.exists() {
                std::fs::copy(st, st.with_extension("json.myterm-bak"))?;
            }
            std::fs::write(st, &settings.after)?;
        } else {
            let delim = format!("MYTERM_EOF_{}", now_secs());
            let heredoc = |body: &str| {
                let mut b = body.to_string();
                if !b.ends_with('\n') {
                    b.push('\n');
                }
                format!("<<'{delim}'\n{b}{delim}")
            };
            let mut s = String::from("mkdir -p \"$HOME/.myterm\" \"$HOME/.claude\" || exit 1\n");
            if install {
                s += &format!("cat > \"$HOME/.myterm/hook.sh\" {}\n", heredoc(&script.after));
                s += "chmod +x \"$HOME/.myterm/hook.sh\" || exit 1\n";
            } else {
                s += "rm -f \"$HOME/.myterm/hook.sh\"\n";
            }
            s += "S=\"$HOME/.claude/settings.json\"\n";
            s += "[ -f \"$S\" ] && cp \"$S\" \"$S.myterm-bak\"\n";
            s += &format!("cat > \"$S.myterm-tmp\" {}\n", heredoc(&settings.after));
            // cat into the original keeps a symlinked settings.json a symlink.
            s += "cat \"$S.myterm-tmp\" > \"$S\" && rm -f \"$S.myterm-tmp\"";
            self.run(&h, &s).await?;
        }
        self.set_info(&h, |i| i.hooks_installed = install);
        Ok(())
    }
}

#[derive(Serialize)]
pub struct FileChange {
    pub path: String,
    pub before: String,
    pub after: String,
}

impl FileChange {
    fn new(path: impl Into<String>, before: String, after: String) -> Self {
        FileChange { path: path.into(), before, after }
    }
}

#[derive(Serialize)]
pub struct HookPlan {
    pub host: String,
    pub install: bool,
    pub files: Vec<FileChange>,
}

fn term_env(c: &mut CommandBuilder) {
    c.env("TERM", "xterm-256color");
    c.env("COLORTERM", "truecolor");
}

fn expand_local(dir: &str) -> String {
    let d = dir.trim();
    if let Some(rest) = d.strip_prefix('~') {
        if let Some(home) = dirs::home_dir() {
            return format!("{}{}", home.to_string_lossy(), rest.replace('/', std::path::MAIN_SEPARATOR_STR));
        }
    }
    d.to_string()
}

/// Directory completion on this machine (used where there is no host channel).
fn complete_local(prefix: &str) -> Vec<String> {
    let expanded = expand_local(prefix);
    let split = expanded.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0);
    let (dir, part) = expanded.split_at(split);
    let shown_dir = &prefix[..prefix.rfind(['/', '\\']).map(|i| i + 1).unwrap_or(0)];
    let part = part.to_lowercase();
    let Ok(rd) = std::fs::read_dir(if dir.is_empty() { "." } else { dir }) else { return vec![] };
    let mut out: Vec<String> = rd
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.to_lowercase().starts_with(&part))
        .map(|n| format!("{shown_dir}{n}\\"))
        .collect();
    out.sort_by_key(|s| s.to_lowercase());
    out.truncate(200);
    out
}

fn default_windows_shell() -> String {
    let in_path = |exe: &str| {
        std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).any(|d| d.join(exe).is_file()))
            .unwrap_or(false)
    };
    if in_path("pwsh.exe") { "pwsh.exe".into() } else { "powershell.exe".into() }
}

fn eph_state_dir() -> std::path::PathBuf {
    dirs::home_dir().unwrap_or_default().join(".myterm").join("state")
}

fn remove_eph_state_file(id: u64) {
    let _ = std::fs::remove_file(eph_state_dir().join(id.to_string()));
}

/// Reads hook state files for local ephemeral sessions (8.5).
pub fn poll_ephemeral_states(app: &App) {
    let mut changed = false;
    {
        let mut eph = app.ephemerals.lock().unwrap();
        for e in eph.iter_mut().filter(|e| !e.ended && e.host == LOCAL) {
            let path = eph_state_dir().join(e.id.to_string());
            let (state, at) = match std::fs::read_to_string(&path) {
                Ok(text) => {
                    let s = text.trim().to_string();
                    let at = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    (if s.is_empty() { None } else { Some(s) }, at)
                }
                Err(_) => (None, 0),
            };
            if e.state != state || e.state_at != at {
                e.state = state;
                e.state_at = at;
                changed = true;
            }
        }
    }
    if changed {
        app.emit_snapshot();
    }
}

enum ConnectError {
    Windows { powershell: bool },
    NeedsLogin(String),
    Failed(String),
}

fn last_line(s: &str) -> String {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string()
}

fn needs_login(stderr: &str) -> bool {
    let l = stderr.to_ascii_lowercase();
    [
        "permission denied",
        "host key verification failed",
        "passphrase",
        "remote host identification",
        "authenticity of host",
        "too many authentication failures",
        "keyboard-interactive",
    ]
    .iter()
    .any(|p| l.contains(p))
}

async fn connect(app: &App, h: &Host) -> Result<(Arc<Channel>, tmux::Bootstrap, bool), ConnectError> {
    // A host last seen as Windows is probed first: its default shell cannot
    // run /bin/sh, and finding that out through the channel takes a timeout.
    let cached_windows = app.state.lock().unwrap().hosts.get(&h.alias).is_some_and(|c| c.kind == "windows");
    if cached_windows && !h.is_local {
        if let Probe::Windows { powershell } = channel::probe(&h.alias).await {
            return Err(ConnectError::Windows { powershell });
        }
    }
    let target = if h.is_local { Target::LocalShell } else { Target::Ssh(h.alias.clone()) };
    let ch = Channel::spawn(&target).map_err(|e| ConnectError::Failed(e.to_string()))?;
    let cfg = app.cfg();
    let override_path = cfg.hosts.get(&h.alias).and_then(|o| o.tmux_path.clone());
    let script = format!(
        "{}\ngrep -qs '.myterm/hook' \"$HOME/.claude/settings.json\" && echo HOOKS=yes\ntrue",
        tmux::bootstrap_script(override_path.as_deref())
    );
    // The first request also covers connecting and authenticating, which can
    // take several seconds over relayed links.
    match ch.run(&script, Duration::from_secs(40)).await {
        Ok(r) => {
            let b = tmux::parse_bootstrap(&r.out);
            if b.os.is_empty() {
                ch.kill();
                return Err(ConnectError::Failed(last_line(&r.out)));
            }
            let hooks = r.out.lines().any(|l| l == "HOOKS=yes");
            Ok((ch, b, hooks))
        }
        Err(e) => {
            // Give stderr a moment to arrive after the process exits.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let err = ch.stderr_text();
            ch.kill();
            if h.is_local {
                return Err(ConnectError::Failed(e.to_string()));
            }
            if needs_login(&err) {
                return Err(ConnectError::NeedsLogin(last_line(&err)));
            }
            match channel::probe(&h.alias).await {
                Probe::Windows { powershell } => Err(ConnectError::Windows { powershell }),
                Probe::Other(msg) => {
                    let msg = if last_line(&err).is_empty() { last_line(&msg) } else { last_line(&err) };
                    Err(ConnectError::Failed(if msg.is_empty() { e.to_string() } else { msg }))
                }
            }
        }
    }
}

async fn host_task(app: Arc<App>, h: Arc<Host>) {
    let mut attempt = 0usize;
    loop {
        if h.removed.load(Ordering::SeqCst) {
            return;
        }
        if h.info.lock().unwrap().hidden {
            app.set_info(&h, |i| {
                i.connected = false;
                i.connecting = false;
                i.status = "hidden".into();
            });
            h.wake.notified().await;
            continue;
        }
        if h.is_local && local_is_windows() {
            app.set_info(&h, |i| {
                i.kind = "ephemeral".into();
                i.connected = true;
                i.connecting = false;
                i.os = "Windows".into();
                i.status = "this machine".into();
                i.hooks_installed = dirs::home_dir()
                    .and_then(|d| std::fs::read_to_string(d.join(".claude").join("settings.json")).ok())
                    .is_some_and(|s| hooks::is_installed(&s));
            });
            h.wake.notified().await;
            continue;
        }
        app.set_info(&h, |i| {
            i.connecting = true;
            i.status = "connecting".into();
        });
        match connect(&app, &h).await {
            Ok((ch, b, hooks_installed)) => {
                let tmux_ok = b.tmux.is_some() && b.version.as_deref().is_some_and(tmux::version_ok);
                let status = match (&b.tmux, &b.version) {
                    (None, _) => "tmux not found".to_string(),
                    (Some(_), Some(v)) if !tmux::version_ok(v) => format!("{v} is older than 3.2"),
                    (Some(t), None) => format!("can't run tmux at {t}"),
                    _ => "connected".to_string(),
                };
                {
                    let mut st = app.state.lock().unwrap();
                    st.hosts.insert(
                        h.alias.clone(),
                        config::HostCache {
                            kind: if tmux_ok { "tmux".into() } else { "unavailable".into() },
                            tmux_path: b.tmux.clone(),
                            os: Some(b.os.clone()),
                        },
                    );
                }
                app.mark_state_dirty();
                app.set_info(&h, |i| {
                    i.kind = if tmux_ok { "tmux".into() } else { "unavailable".into() };
                    i.connected = tmux_ok;
                    i.connecting = false;
                    i.needs_login = false;
                    i.status = status;
                    i.os = b.os.clone();
                    i.tmux = b.tmux.clone();
                    i.tmux_version = b.version.clone();
                    i.home = Some(b.home.clone());
                    i.user = (!b.user.is_empty()).then(|| b.user.clone());
                    i.hooks_installed = hooks_installed;
                });
                if !tmux_ok {
                    ch.kill();
                } else {
                    attempt = 0;
                    *h.channel.lock().unwrap() = Some(ch.clone());
                    let t = b.tmux.clone().unwrap();
                    let script = tmux::list_script(&t);
                    loop {
                        match ch.run(&script, Duration::from_secs(15)).await {
                            Ok(r) => {
                                let status = match tmux::parse_list(&h.alias, r.code, &r.out) {
                                    ListResult::Sessions(s) => {
                                        let changed = *h.sessions.lock().unwrap() != s;
                                        if changed {
                                            app.remember_folders(&h.alias, &s);
                                            *h.sessions.lock().unwrap() = s;
                                            app.emit_snapshot();
                                        }
                                        "connected".to_string()
                                    }
                                    // Shown on the host instead of a silently empty list.
                                    ListResult::Error(e) => format!("tmux: {e}"),
                                };
                                app.set_info(&h, |i| i.status = status);
                            }
                            Err(_) => break,
                        }
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                            _ = h.wake.notified() => {}
                        }
                        if ch.is_dead() || h.removed.load(Ordering::SeqCst) || h.info.lock().unwrap().hidden {
                            break;
                        }
                    }
                    ch.kill();
                    *h.channel.lock().unwrap() = None;
                    if h.removed.load(Ordering::SeqCst) {
                        return;
                    }
                    app.set_info(&h, |i| {
                        i.kind = "unavailable".into();
                        i.connected = false;
                        i.status = "connection lost".into();
                    });
                }
            }
            Err(ConnectError::Windows { powershell }) => {
                app.state.lock().unwrap().hosts.insert(
                    h.alias.clone(),
                    config::HostCache { kind: "windows".into(), tmux_path: None, os: Some("Windows".into()) },
                );
                app.mark_state_dirty();
                app.set_info(&h, |i| {
                    i.kind = "ephemeral".into();
                    i.connected = true;
                    i.connecting = false;
                    i.needs_login = false;
                    i.os = "Windows".into();
                    i.win_powershell = powershell;
                    i.status = if powershell { "Windows (PowerShell)".into() } else { "Windows (cmd)".into() };
                });
                attempt = 0;
                // Nothing to poll; wait for an explicit reconnect.
                h.wake.notified().await;
                continue;
            }
            Err(ConnectError::NeedsLogin(msg)) => {
                app.set_info(&h, |i| {
                    i.kind = "unavailable".into();
                    i.connected = false;
                    i.connecting = false;
                    i.needs_login = true;
                    i.status = if msg.is_empty() { "needs interactive login".into() } else { format!("needs interactive login: {msg}") };
                });
            }
            Err(ConnectError::Failed(msg)) => {
                app.set_info(&h, |i| {
                    i.kind = "unavailable".into();
                    i.connected = false;
                    i.connecting = false;
                    i.needs_login = false;
                    i.status = if msg.is_empty() { "unreachable".into() } else { msg };
                });
            }
        }
        let wait = BACKOFF[attempt.min(BACKOFF.len() - 1)];
        attempt += 1;
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(wait)) => {}
            _ = h.wake.notified() => { attempt = 0; }
        }
    }
}
