//! `config.toml` (edited by the user) and `state.json` (written by the app).

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("myterm")
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.toml")
}

pub fn state_path() -> PathBuf {
    data_dir().join("state.json")
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct HostOverride {
    pub tmux_path: Option<String>,
    pub mouse: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Keys {
    pub switcher: String,
    pub palette: String,
    pub new_session: String,
    pub copy: String,
    pub paste: String,
    pub toggle_sidebar: String,
}

impl Default for Keys {
    fn default() -> Self {
        Keys {
            switcher: "Ctrl+Shift+O".into(),
            palette: "Ctrl+Shift+P".into(),
            new_session: "Ctrl+Shift+N".into(),
            copy: "Ctrl+Shift+C".into(),
            paste: "Ctrl+Shift+V".into(),
            toggle_sidebar: "Ctrl+Shift+B".into(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    pub font_family: String,
    pub font_size: f32,
    pub line_height: f32,
    /// xterm.js theme keys (background, foreground, cursor, black, red, ...).
    pub theme: BTreeMap<String, String>,
    pub keys: Keys,
    pub hidden_hosts: Vec<String>,
    /// The ssh alias that refers to this machine, so it is not listed twice.
    pub local_alias: Option<String>,
    pub hide_tmux_status: bool,
    pub max_open_sessions: usize,
    pub pin_attention: bool,
    pub notify_permission: bool,
    /// Default for `mouse on` on attach; per-host override in [hosts.<alias>].
    pub mouse: bool,
    /// Shell for local sessions on Windows. Default: pwsh if installed, else powershell.
    pub local_shell: Option<String>,
    pub hosts: HashMap<String, HostOverride>,
}

impl Default for Config {
    fn default() -> Self {
        let theme = [
            ("background", "#16181d"),
            ("foreground", "#c8ccd4"),
            ("cursor", "#c8ccd4"),
            ("cursorAccent", "#16181d"),
            ("selectionBackground", "#3b4252"),
            ("black", "#1d2027"),
            ("red", "#f7768e"),
            ("green", "#9ece6a"),
            ("yellow", "#e0af68"),
            ("blue", "#7aa2f7"),
            ("magenta", "#bb9af7"),
            ("cyan", "#7dcfff"),
            ("white", "#a9b1d6"),
            ("brightBlack", "#545c7e"),
            ("brightRed", "#ff899d"),
            ("brightGreen", "#b9f27c"),
            ("brightYellow", "#ffc777"),
            ("brightBlue", "#8db0ff"),
            ("brightMagenta", "#c7a9ff"),
            ("brightCyan", "#a4daff"),
            ("brightWhite", "#c0caf5"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        Config {
            font_family: if cfg!(target_os = "macos") {
                "Menlo, Monaco, monospace".into()
            } else {
                "'Cascadia Mono', Consolas, monospace".into()
            },
            font_size: 13.0,
            line_height: 1.1,
            theme,
            keys: Keys::default(),
            hidden_hosts: vec![],
            local_alias: None,
            hide_tmux_status: true,
            max_open_sessions: 8,
            pin_attention: true,
            notify_permission: false,
            mouse: true,
            local_shell: None,
            hosts: HashMap::new(),
        }
    }
}

const CONFIG_TEMPLATE: &str = r##"# myterm settings. Changes are picked up automatically.

# font_family = "'Cascadia Mono', Consolas, monospace"
# font_size = 13
# line_height = 1.1

# The ssh alias that points at this machine, so it is not listed twice.
# local_alias = "my-laptop"

# Hosts not shown or connected to. "Hide host" in the palette edits this list.
hidden_hosts = []

# Hide the tmux status bar in sessions the app creates.
# hide_tmux_status = true

# Background sessions kept connected; the least recently viewed is detached beyond this.
# max_open_sessions = 8

# Pin sessions that need attention to a "Needs you" section.
# pin_attention = true

# OS notification when a session you are not looking at waits for permission.
# notify_permission = false

# Turn on tmux mouse mode for a session before attaching (wheel scrolls history).
# mouse = true

# Shell for local sessions on Windows.
# local_shell = "pwsh.exe"

# [keys]
# switcher = "Ctrl+Shift+O"
# palette = "Ctrl+Shift+P"
# new_session = "Ctrl+Shift+N"
# copy = "Ctrl+Shift+C"
# paste = "Ctrl+Shift+V"
# toggle_sidebar = "Ctrl+Shift+B"

# [theme]
# background = "#16181d"
# foreground = "#c8ccd4"

# Per-host overrides:
# [hosts.my-server]
# tmux_path = "/opt/homebrew/bin/tmux"
# mouse = false
"##;

/// Loads config.toml, creating it from a commented template on first run.
/// Returns the config and a parse error message, if any.
pub fn load_config() -> (Config, Option<String>) {
    let path = config_path();
    if !path.exists() {
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(&path, CONFIG_TEMPLATE);
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => match toml::from_str::<Config>(&text) {
            Ok(c) => {
                let mut merged = Config::default();
                // Theme keys merge over the default theme instead of replacing it.
                let mut theme = merged.theme.clone();
                theme.extend(c.theme.clone());
                merged = Config { theme, ..c };
                (merged, None)
            }
            Err(e) => (Config::default(), Some(format!("config.toml: {e}"))),
        },
        Err(e) => (Config::default(), Some(format!("config.toml: {e}"))),
    }
}

/// Adds or removes a host from `hidden_hosts`, preserving the rest of the file.
pub fn set_host_hidden(alias: &str, hidden: bool) -> anyhow::Result<()> {
    let path = config_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    let mut list: Vec<String> = doc
        .get("hidden_hosts")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    list.retain(|h| h != alias);
    if hidden {
        list.push(alias.to_string());
    }
    let mut arr = toml_edit::Array::new();
    for h in list {
        arr.push(h);
    }
    doc["hidden_hosts"] = toml_edit::value(arr);
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct HostCache {
    pub kind: String,
    pub tmux_path: Option<String>,
    pub os: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct WindowGeom {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct State {
    /// Session key ("host/slug") -> unix time it was last on screen.
    pub last_viewed: HashMap<String, i64>,
    /// Host -> folder -> unix time last seen or used.
    pub known_folders: HashMap<String, HashMap<String, i64>>,
    pub hosts: HashMap<String, HostCache>,
    pub window: Option<WindowGeom>,
    pub list_mode: String,
    pub sidebar_width: f64,
    pub sidebar_collapsed: bool,
}

impl Default for State {
    fn default() -> Self {
        State {
            last_viewed: HashMap::new(),
            known_folders: HashMap::new(),
            hosts: HashMap::new(),
            window: None,
            list_mode: "recent".into(),
            sidebar_width: 280.0,
            sidebar_collapsed: false,
        }
    }
}

pub fn load_state() -> State {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save_state(s: &State) {
    let _ = std::fs::create_dir_all(data_dir());
    if let Ok(text) = serde_json::to_string_pretty(s) {
        let tmp = state_path().with_extension("json.tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, state_path());
        }
    }
}
