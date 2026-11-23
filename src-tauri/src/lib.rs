mod channel;
mod config;
mod hooks;
mod hosts;
#[cfg(test)]
mod live_test;
mod pty;
mod sshconfig;
mod tmux;

use hosts::{App, HookPlan, Snapshot};
use notify::{RecursiveMode, Watcher};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{Emitter, Manager, State, WindowEvent};

type R<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

struct Quitting(AtomicBool);

#[derive(Serialize)]
struct Boot {
    config: config::Config,
    config_error: Option<String>,
    state: config::State,
    snapshot: Snapshot,
    platform: &'static str,
}

#[tauri::command]
fn boot(app: State<Arc<App>>) -> Boot {
    Boot {
        config: app.cfg(),
        config_error: app.config_error.lock().unwrap().clone(),
        state: app.state.lock().unwrap().clone(),
        snapshot: app.snapshot(),
        platform: if cfg!(target_os = "macos") { "macos" } else if cfg!(windows) { "windows" } else { "linux" },
    }
}

#[tauri::command]
fn snapshot(app: State<Arc<App>>) -> Snapshot {
    app.snapshot()
}

#[tauri::command]
async fn attach(
    app: State<'_, Arc<App>>,
    host: String,
    slug: String,
    cols: u16,
    rows: u16,
    channel: Channel<InvokeResponseBody>,
) -> R<u32> {
    let cmd = app.attach_command(&host, &slug).await.map_err(err)?;
    app.ptys.spawn(cmd, cols, rows, channel, None).map_err(err)
}

#[tauri::command]
fn open_login(app: State<Arc<App>>, host: String, cols: u16, rows: u16, channel: Channel<InvokeResponseBody>) -> R<u32> {
    let cmd = app.login_command(&host);
    app.ptys.spawn(cmd, cols, rows, channel, None).map_err(err)
}

#[derive(Serialize)]
struct Started {
    key: String,
    pty: u32,
    input: String,
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
fn start_ephemeral(
    app: State<Arc<App>>,
    host: String,
    dir: String,
    name: String,
    kind: String,
    cols: u16,
    rows: u16,
    channel: Channel<InvokeResponseBody>,
) -> R<Started> {
    let (key, pty, input) = app
        .inner()
        .start_ephemeral(&host, &dir, &name, &kind, cols, rows, channel)
        .map_err(err)?;
    Ok(Started { key, pty, input })
}

#[tauri::command]
fn pty_write(app: State<Arc<App>>, id: u32, data: String) -> R<()> {
    app.ptys.write(id, data.as_bytes()).map_err(err)
}

#[tauri::command]
fn pty_resize(app: State<Arc<App>>, id: u32, cols: u16, rows: u16) -> R<()> {
    app.ptys.resize(id, cols, rows).map_err(err)
}

#[tauri::command]
fn pty_ack(app: State<Arc<App>>, id: u32, bytes: usize) {
    app.ptys.ack(id, bytes);
}

#[tauri::command]
fn pty_close(app: State<Arc<App>>, id: u32) {
    app.ptys.close(id);
}

#[tauri::command]
async fn create_session(app: State<'_, Arc<App>>, host: String, dir: String, name: String, kind: String) -> R<String> {
    app.create_session(&host, &dir, &name, &kind).await.map_err(err)
}

#[tauri::command]
async fn rename_session(app: State<'_, Arc<App>>, host: String, slug: String, name: String) -> R<()> {
    app.rename_session(&host, &slug, &name).await.map_err(err)
}

#[tauri::command]
async fn kill_session(app: State<'_, Arc<App>>, host: String, slug: String) -> R<()> {
    app.kill_session(&host, &slug).await.map_err(err)
}

#[tauri::command]
async fn complete_path(app: State<'_, Arc<App>>, host: String, prefix: String) -> R<Vec<String>> {
    app.complete_path(&host, &prefix).await.map_err(err)
}

#[tauri::command]
fn attach_command_text(app: State<Arc<App>>, host: String, slug: String) -> R<String> {
    app.attach_command_text(&host, &slug).map_err(err)
}

#[tauri::command]
async fn reconnect_host(app: State<'_, Arc<App>>, host: String) -> R<()> {
    app.reconnect(&host).await.map_err(err)
}

#[tauri::command]
fn set_host_hidden(app: State<Arc<App>>, host: String, hidden: bool) -> R<()> {
    config::set_host_hidden(&host, hidden).map_err(err)?;
    reload_config(app.inner());
    Ok(())
}

#[tauri::command]
async fn hooks_plan(app: State<'_, Arc<App>>, host: String, install: bool) -> R<HookPlan> {
    app.hooks_plan(&host, install).await.map_err(err)
}

#[tauri::command]
async fn hooks_apply(app: State<'_, Arc<App>>, host: String, install: bool) -> R<()> {
    app.hooks_apply(&host, install).await.map_err(err)
}

#[tauri::command]
fn mark_viewed(app: State<Arc<App>>, key: String) {
    app.state.lock().unwrap().last_viewed.insert(key, config::now_secs());
    app.mark_state_dirty();
}

#[tauri::command]
fn set_ui_state(app: State<Arc<App>>, list_mode: Option<String>, sidebar_width: Option<f64>, sidebar_collapsed: Option<bool>) {
    let mut st = app.state.lock().unwrap();
    if let Some(m) = list_mode {
        st.list_mode = m;
    }
    if let Some(w) = sidebar_width {
        st.sidebar_width = w;
    }
    if let Some(c) = sidebar_collapsed {
        st.sidebar_collapsed = c;
    }
    drop(st);
    app.mark_state_dirty();
}

#[tauri::command]
fn get_state(app: State<Arc<App>>) -> config::State {
    app.state.lock().unwrap().clone()
}

#[tauri::command]
fn config_path() -> String {
    config::config_path().to_string_lossy().to_string()
}

#[tauri::command]
fn quit(app: tauri::AppHandle, st: State<Arc<App>>, quitting: State<Quitting>) {
    quitting.0.store(true, Ordering::SeqCst);
    save_window(&app, &st);
    config::save_state(&st.state.lock().unwrap());
    st.ptys.close_all();
    app.exit(0);
}

fn reload_config(app: &Arc<App>) {
    let (cfg, error) = config::load_config();
    let changed = *app.config.read().unwrap() != cfg;
    *app.config_error.lock().unwrap() = error.clone();
    if changed {
        *app.config.write().unwrap() = cfg.clone();
        app.sync_hosts();
    }
    let _ = app.handle.emit("config", serde_json::json!({ "config": cfg, "error": error }));
}

fn save_window(handle: &tauri::AppHandle, app: &App) {
    if let Some(w) = handle.get_webview_window("main") {
        let maximized = w.is_maximized().unwrap_or(false);
        let mut st = app.state.lock().unwrap();
        let mut g = st.window.clone().unwrap_or_default();
        g.maximized = maximized;
        if !maximized && !w.is_minimized().unwrap_or(false) {
            if let (Ok(p), Ok(s)) = (w.outer_position(), w.outer_size()) {
                g.x = p.x;
                g.y = p.y;
                g.width = s.width;
                g.height = s.height;
            }
        }
        st.window = Some(g);
    }
}

fn start_watchers(app: Arc<App>, ssh_files: Vec<std::path::PathBuf>) {
    let (tx, rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let Ok(mut watcher) = notify::recommended_watcher(tx) else { return };
    let cfg_dir = config::data_dir();
    let _ = watcher.watch(&cfg_dir, RecursiveMode::NonRecursive);
    let mut dirs: Vec<std::path::PathBuf> = vec![sshconfig::ssh_dir()];
    for f in &ssh_files {
        if let Some(d) = f.parent() {
            if !dirs.iter().any(|x| x == d) {
                dirs.push(d.to_path_buf());
            }
        }
    }
    for d in &dirs {
        let _ = watcher.watch(d, RecursiveMode::NonRecursive);
    }
    std::thread::spawn(move || {
        let _watcher = watcher;
        let config_file = config::config_path();
        loop {
            let Ok(first) = rx.recv() else { return };
            // Debounce bursts of events from editors.
            let mut events = vec![first];
            while let Ok(e) = rx.recv_timeout(Duration::from_millis(300)) {
                events.push(e);
            }
            let paths: Vec<std::path::PathBuf> =
                events.into_iter().filter_map(Result::ok).flat_map(|e| e.paths).collect();
            if paths.iter().any(|p| p == &config_file) {
                reload_config(&app);
            }
            let ssh_dir = sshconfig::ssh_dir();
            if paths.iter().any(|p| p.starts_with(&ssh_dir) && !p.to_string_lossy().contains("known_hosts")) {
                app.sync_hosts();
            }
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_notification::init())
        .manage(Quitting(AtomicBool::new(false)))
        .setup(|tauri_app| {
            let app = App::new(tauri_app.handle().clone());
            tauri_app.manage(app.clone());

            if let Some(w) = tauri_app.get_webview_window("main") {
                if let Some(g) = app.state.lock().unwrap().window.clone() {
                    if g.width >= 400 && g.height >= 300 {
                        let _ = w.set_size(tauri::PhysicalSize::new(g.width, g.height));
                        let _ = w.set_position(tauri::PhysicalPosition::new(g.x, g.y));
                    }
                    if g.maximized {
                        let _ = w.maximize();
                    }
                }
                let _ = w.show();
            }

            let files = app.sync_hosts();
            start_watchers(app.clone(), files);

            // Persist state.json when it changes; poll local ephemeral hook states.
            let a = app.clone();
            tauri::async_runtime::spawn(async move {
                let mut tick = 0u64;
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    hosts::poll_ephemeral_states(&a);
                    tick += 1;
                    if tick % 2 == 0 && a.state_dirty.swap(false, Ordering::SeqCst) {
                        let st = a.state.lock().unwrap().clone();
                        config::save_state(&st);
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let handle = window.app_handle();
                let app = handle.state::<Arc<App>>();
                let quitting = handle.state::<Quitting>();
                if quitting.0.load(Ordering::SeqCst) {
                    return;
                }
                if app.running_ephemerals() > 0 {
                    api.prevent_close();
                    let _ = handle.emit("confirm-quit", app.running_ephemerals());
                    return;
                }
                save_window(handle, &app);
                config::save_state(&app.state.lock().unwrap());
                app.ptys.close_all();
            }
        })
        .invoke_handler(tauri::generate_handler![
            boot,
            snapshot,
            attach,
            open_login,
            start_ephemeral,
            pty_write,
            pty_resize,
            pty_ack,
            pty_close,
            create_session,
            rename_session,
            kill_session,
            complete_path,
            attach_command_text,
            reconnect_host,
            set_host_hidden,
            hooks_plan,
            hooks_apply,
            mark_viewed,
            set_ui_state,
            get_state,
            config_path,
            quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running myterm");
}
