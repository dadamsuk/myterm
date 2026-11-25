mod channel;
mod config;
pub(crate) mod files;
mod hooks;
mod hosts;
pub mod keeper;
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
async fn create_window(
    app: State<'_, Arc<App>>,
    host: String,
    slug: String,
    dir: String,
    name: String,
    kind: String,
) -> R<String> {
    app.create_window(&host, &slug, &dir, &name, &kind).await.map_err(err)
}

#[tauri::command]
async fn rename_window(app: State<'_, Arc<App>>, host: String, slug: String, window: String, name: String) -> R<()> {
    app.rename_window(&host, &slug, &window, &name).await.map_err(err)
}

#[tauri::command]
async fn kill_window(app: State<'_, Arc<App>>, host: String, slug: String, window: String) -> R<()> {
    app.kill_window(&host, &slug, &window).await.map_err(err)
}

#[tauri::command]
async fn select_window(app: State<'_, Arc<App>>, host: String, slug: String, window: String) -> R<()> {
    app.select_window(&host, &slug, &window).await.map_err(err)
}

#[tauri::command]
async fn complete_path(app: State<'_, Arc<App>>, host: String, prefix: String) -> R<Vec<String>> {
    app.complete_path(&host, &prefix).await.map_err(err)
}

#[tauri::command]
async fn paste_image(app: State<'_, Arc<App>>, host: String) -> R<Option<String>> {
    app.paste_image(&host).await.map_err(err)
}

#[tauri::command]
async fn drop_files(app: State<'_, Arc<App>>, host: String, paths: Vec<String>) -> R<Vec<String>> {
    app.drop_files(&host, &paths).await.map_err(err)
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
fn set_hosts(app: State<Arc<App>>, hosts: Vec<config::HostEntry>) -> R<()> {
    app.inner().set_hosts(hosts).map_err(err)?;
    reload_config(app.inner());
    Ok(())
}

#[tauri::command]
fn ssh_aliases(app: State<Arc<App>>) -> Vec<String> {
    app.ssh_aliases()
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
fn set_ui_state(
    app: State<Arc<App>>,
    list_mode: Option<String>,
    sidebar_width: Option<f64>,
    sidebar_collapsed: Option<bool>,
    sidebar_auto_hide: Option<bool>,
    font_zoom: Option<i32>,
) {
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
    if let Some(a) = sidebar_auto_hide {
        st.sidebar_auto_hide = a;
    }
    if let Some(z) = font_zoom {
        st.font_zoom = z;
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

/// Opens config.toml in the default editor. Done here rather than through the
/// opener plugin from JS, whose path scope would otherwise have to allow it.
#[tauri::command]
fn open_config(handle: tauri::AppHandle) -> R<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = config::config_path();
    if !path.exists() {
        config::load_config(); // writes the commented template
    }
    let p = path.to_string_lossy().to_string();
    match handle.opener().open_path(&p, None::<&str>) {
        Ok(()) => Ok(()),
        // .toml often has no associated program on Windows.
        #[cfg(windows)]
        Err(_) => std::process::Command::new("notepad.exe").arg(&p).spawn().map(|_| ()).map_err(err),
        #[cfg(not(windows))]
        Err(e) => Err(err(e)),
    }
}

/// Opens ~/.ssh/config, where hosts are added; myterm picks up changes itself.
#[tauri::command]
fn open_ssh_config(handle: tauri::AppHandle) -> R<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = sshconfig::ssh_dir().join("config");
    if !path.exists() {
        std::fs::create_dir_all(sshconfig::ssh_dir()).map_err(err)?;
        std::fs::write(&path, "# Add a host:
# Host myserver
#   HostName 192.0.2.10
#   User me
").map_err(err)?;
    }
    let p = path.to_string_lossy().to_string();
    match handle.opener().open_path(&p, None::<&str>) {
        Ok(()) => Ok(()),
        // A file without an extension has no associated program on Windows.
        #[cfg(windows)]
        Err(_) => std::process::Command::new("notepad.exe").arg(&p).spawn().map(|_| ()).map_err(err),
        #[cfg(not(windows))]
        Err(e) => Err(err(e)),
    }
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
            // inner_size, because set_size on restore sets the inner size.
            if let (Ok(p), Ok(s)) = (w.outer_position(), w.inner_size()) {
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
            // Record geometry as it changes, so it survives a crash or forced quit;
            // the 2 s state writer persists it.
            if matches!(event, WindowEvent::Moved(_) | WindowEvent::Resized(_)) {
                let handle = window.app_handle();
                if let Some(app) = handle.try_state::<Arc<App>>() {
                    save_window(handle, &app);
                    app.mark_state_dirty();
                }
                return;
            }
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
            select_window,
            create_window,
            rename_window,
            kill_window,
            complete_path,
            attach_command_text,
            paste_image,
            drop_files,
            reconnect_host,
            set_hosts,
            ssh_aliases,
            hooks_plan,
            hooks_apply,
            mark_viewed,
            set_ui_state,
            get_state,
            config_path,
            open_config,
            open_ssh_config,
            quit
        ])
        .run(tauri::generate_context!())
        .expect("error while running myterm");
}
