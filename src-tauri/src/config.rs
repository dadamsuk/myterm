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
    /// Shows the sidebar while it is in auto-hide mode.
    pub peek_sidebar: String,
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
            peek_sidebar: "Ctrl+Alt+B".into(),
        }
    }
}

/// One host in the sidebar. `name` is anything `ssh` accepts as a destination:
/// an alias from ~/.ssh/config such as `intra`, `user@server`, or `local`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct HostEntry {
    pub name: String,
    /// Connect when the app starts. Off: connect on first click.
    pub auto_connect: bool,
}

/// A session to start again later (`[[saved]]` tables): after a reboot, say.
/// Its commands are typed into the new session one line at a time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct SavedSession {
    pub name: String,
    pub host: String,
    /// "shell" or "claude".
    pub kind: String,
    pub cwd: String,
    /// Typed in order; each waits for the one before to settle, so
    /// `wsl` and then a command for the Linux shell works.
    pub commands: Vec<String>,
    /// Started when myterm starts, unless a session of that name is running.
    pub autostart: bool,
}

impl Default for SavedSession {
    fn default() -> Self {
        SavedSession {
            name: String::new(),
            host: "local".into(),
            kind: "shell".into(),
            cwd: String::new(),
            commands: vec![],
            autostart: false,
        }
    }
}

impl Default for HostEntry {
    fn default() -> Self {
        HostEntry { name: String::new(), auto_connect: true }
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
    /// Hosts shown, in order (`[[host]]` tables). None until first written:
    /// then the list is built once from ~/.ssh/config (see `migrate_hosts`).
    #[serde(rename = "host", skip_serializing_if = "Option::is_none")]
    pub host_list: Option<Vec<HostEntry>>,
    /// Legacy: hosts hidden before the `[[host]]` list existed. Read once by
    /// `migrate_hosts`, then removed from the file.
    #[serde(skip_serializing)]
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
    /// Sessions to start again later (`[[saved]]` tables).
    #[serde(rename = "saved", skip_serializing_if = "Vec::is_empty")]
    pub saved: Vec<SavedSession>,
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
            host_list: None,
            hidden_hosts: vec![],
            local_alias: None,
            hide_tmux_status: true,
            max_open_sessions: 8,
            pin_attention: false,
            notify_permission: false,
            mouse: true,
            local_shell: None,
            hosts: HashMap::new(),
            saved: vec![],
        }
    }
}

const CONFIG_TEMPLATE: &str = r##"# myterm settings. Changes are picked up automatically.

# font_family = "'Cascadia Mono', Consolas, monospace"
# font_size = 13
# line_height = 1.1

# The ssh alias that points at this machine, so it is not added twice.
# local_alias = "my-laptop"

# Hosts in the sidebar, in this order. Manage them with the "Hosts" dialog
# (the host button in the sidebar) or edit them here. `name` is anything ssh
# accepts: an alias from ~/.ssh/config such as "intra", or "user@server".
# "local" is this machine. auto_connect = false connects on first click.
# On first run the list is filled from ~/.ssh/config.
#
# [[host]]
# name = "local"
# auto_connect = true
#
# [[host]]
# name = "intra"
# auto_connect = false

# Hide the tmux status bar in sessions the app creates.
# hide_tmux_status = true

# Background sessions kept connected; the least recently viewed is detached beyond this.
# max_open_sessions = 8

# Also move sessions that need attention into a "Needs you" section at the
# top. Off: they stay in place and are highlighted.
# pin_attention = false

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
# peek_sidebar = "Ctrl+Alt+B"

# [theme]
# background = "#16181d"
# foreground = "#c8ccd4"

# Saved sessions: started again from the palette ("Start saved sessions"),
# or when myterm starts if autostart is on. Made by "Save session…" in a
# session's right-click menu; hand edits work too. Each command is typed in
# turn, waiting for the previous one's output to settle.
#
# [[saved]]
# name = "api"
# host = "local"
# kind = "shell"
# cwd = 'C:\dev\api'
# commands = ["wsl", "cd ~/api && ./run.sh"]
# autostart = true

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

/// Writes the `[[host]]` list, preserving the rest of the file, and drops the
/// legacy `hidden_hosts` key.
pub fn write_hosts(list: &[HostEntry]) -> anyhow::Result<()> {
    let path = config_path();
    let _ = std::fs::create_dir_all(data_dir());
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| CONFIG_TEMPLATE.to_string());
    std::fs::write(&path, with_hosts(&text, list)?)?;
    Ok(())
}

/// Drops a one-line `hidden_hosts = [...]` and the template comment above it
/// as text: toml_edit keeps the comments above a key as part of that key, so
/// removing it there would also delete the file's header.
fn strip_hidden_hosts(text: &str) -> String {
    let mut out: Vec<&str> = vec![];
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("hidden_hosts") && t.contains(']') {
            if out.last().is_some_and(|l| l.trim_start().starts_with("# Hosts not shown or connected to")) {
                out.pop();
            }
            continue;
        }
        out.push(line);
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    s
}

fn with_hosts(text: &str, list: &[HostEntry]) -> anyhow::Result<String> {
    let mut doc: toml_edit::DocumentMut = strip_hidden_hosts(text).parse()?;
    // A multi-line list is left to toml_edit.
    doc.remove("hidden_hosts");
    doc.remove("host");
    if list.is_empty() {
        // An empty list must still exist, or the next start would re-import.
        doc["host"] = toml_edit::value(toml_edit::Array::new());
    } else {
        let mut arr = toml_edit::ArrayOfTables::new();
        for (i, h) in list.iter().enumerate() {
            let mut t = toml_edit::Table::new();
            if i == 0 {
                t.decor_mut().set_prefix(
                    "\n# Hosts in the sidebar, in this order. Edited by the Hosts dialog; hand\n\
                     # edits work too. name: an ssh destination, e.g. an alias from ~/.ssh/config.\n\
                     # auto_connect = false: connect on first click instead of at start.\n",
                );
            }
            t["name"] = toml_edit::value(h.name.clone());
            t["auto_connect"] = toml_edit::value(h.auto_connect);
            arr.push(t);
        }
        doc["host"] = toml_edit::Item::ArrayOfTables(arr);
    }
    Ok(doc.to_string())
}

/// Writes the `[[saved]]` list, preserving the rest of the file.
pub fn write_saved(list: &[SavedSession]) -> anyhow::Result<()> {
    let path = config_path();
    let _ = std::fs::create_dir_all(data_dir());
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| CONFIG_TEMPLATE.to_string());
    std::fs::write(&path, with_saved(&text, list)?)?;
    Ok(())
}

fn with_saved(text: &str, list: &[SavedSession]) -> anyhow::Result<String> {
    let mut doc: toml_edit::DocumentMut = text.parse()?;
    doc.remove("saved");
    if !list.is_empty() {
        let mut arr = toml_edit::ArrayOfTables::new();
        for (i, s) in list.iter().enumerate() {
            let mut t = toml_edit::Table::new();
            if i == 0 {
                t.decor_mut().set_prefix(
                    "
# Saved sessions, started again from the palette (\"Start saved sessions\")
                     # or at launch with autostart = true. commands are typed in turn.
",
                );
            }
            t["name"] = toml_edit::value(s.name.clone());
            t["host"] = toml_edit::value(s.host.clone());
            t["kind"] = toml_edit::value(s.kind.clone());
            t["cwd"] = toml_edit::value(s.cwd.clone());
            let mut cmds = toml_edit::Array::new();
            for c in &s.commands {
                cmds.push(c.clone());
            }
            t["commands"] = toml_edit::value(cmds);
            t["autostart"] = toml_edit::value(s.autostart);
            arr.push(t);
        }
        doc["saved"] = toml_edit::Item::ArrayOfTables(arr);
    }
    Ok(doc.to_string())
}

/// Removes duplicate and empty names, keeping the first of each.
pub fn clean_hosts(list: Vec<HostEntry>) -> Vec<HostEntry> {
    let mut out: Vec<HostEntry> = vec![];
    for mut h in list {
        h.name = h.name.trim().to_string();
        if !h.name.is_empty() && !out.iter().any(|x| x.name == h.name) {
            out.push(h);
        }
    }
    out
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
    /// The sidebar slides over the terminal when the pointer reaches the left
    /// edge (or on peek_sidebar), and hides again when it leaves.
    pub sidebar_auto_hide: bool,
    /// Points added to font_size by Ctrl+= / Ctrl+-.
    pub font_zoom: i32,
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
            sidebar_auto_hide: false,
            font_zoom: 0,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_list_parses_in_order() {
        let c: Config = toml::from_str(
            "[[host]]\nname = \"local\"\n\n[[host]]\nname = \"intra\"\nauto_connect = false\n\n[hosts.intra]\nmouse = false\n",
        )
        .unwrap();
        let l = c.host_list.unwrap();
        assert_eq!(l.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["local", "intra"]);
        assert!(l[0].auto_connect);
        assert!(!l[1].auto_connect);
        assert_eq!(c.hosts["intra"].mouse, Some(false));
        // No list at all means "not set up yet", not "no hosts".
        assert!(toml::from_str::<Config>("font_size = 12").unwrap().host_list.is_none());
        assert_eq!(toml::from_str::<Config>("host = []").unwrap().host_list, Some(vec![]));
    }

    #[test]
    fn writes_hosts_keeping_the_rest() {
        let e = |n: &str, a: bool| HostEntry { name: n.into(), auto_connect: a };
        let start = "font_size = 14\nhidden_hosts = [\"x\"]\n\n[hosts.intra]\nmouse = false\n";
        let text = with_hosts(start, &[e("local", true), e("intra", false)]).unwrap();
        let c: Config = toml::from_str(&text).unwrap();
        assert_eq!(c.font_size, 14.0);
        assert!(c.hidden_hosts.is_empty());
        assert_eq!(c.hosts["intra"].mouse, Some(false));
        assert_eq!(c.host_list, Some(vec![e("local", true), e("intra", false)]));
        // Rewriting replaces the list rather than appending to it.
        let text = with_hosts(&text, &[e("intra", true)]).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap().host_list, Some(vec![e("intra", true)]));
        let text = with_hosts(&text, &[]).unwrap();
        assert_eq!(toml::from_str::<Config>(&text).unwrap().host_list, Some(vec![]));
        // The template itself parses, with no list yet.
        assert!(toml::from_str::<Config>(CONFIG_TEMPLATE).unwrap().host_list.is_none());
    }

    #[test]
    fn migrates_an_old_file() {
        let old = "# intro\n\n# Hosts not shown or connected to. \"Hide host\" in the palette edits this list.\nhidden_hosts = []\n\n# hide_tmux_status = true\n\n[hosts.het1-ts]\ntmux_path = \"/x/tmux\"\n";
        let e = |n: &str| HostEntry { name: n.into(), auto_connect: true };
        let text = with_hosts(old, &[e("local"), e("het1-ts")]).unwrap();
        assert!(!text.contains("Hide host"));
        assert!(text.starts_with("# intro\n"));
        assert!(text.contains("# hide_tmux_status = true"));
        let c: Config = toml::from_str(&text).unwrap();
        assert_eq!(c.hosts["het1-ts"].tmux_path.as_deref(), Some("/x/tmux"));
        assert_eq!(c.host_list.unwrap().len(), 2);
    }

    #[test]
    fn writes_saved_keeping_the_rest() {
        let s = SavedSession {
            name: "api".into(),
            host: "local".into(),
            kind: "shell".into(),
            cwd: r"C:\dev\api".into(),
            commands: vec!["wsl".into(), "cd ~/api && ./run.sh \"x\"".into()],
            autostart: true,
        };
        let start = "font_size = 14

[[host]]
name = \"local\"

[hosts.intra]
mouse = false
";
        let text = with_saved(start, &[s.clone()]).unwrap();
        let c: Config = toml::from_str(&text).unwrap();
        assert_eq!(c.font_size, 14.0);
        assert_eq!(c.host_list.unwrap().len(), 1);
        assert_eq!(c.hosts["intra"].mouse, Some(false));
        assert_eq!(c.saved, vec![s]);
        let text = with_saved(&text, &[]).unwrap();
        assert!(toml::from_str::<Config>(&text).unwrap().saved.is_empty());
        assert!(toml::from_str::<Config>(CONFIG_TEMPLATE).unwrap().saved.is_empty());
    }

    #[test]
    fn clean_drops_duplicates_and_blanks() {
        let e = |n: &str| HostEntry { name: n.into(), auto_connect: true };
        let l = clean_hosts(vec![e(" a "), e(""), e("b"), e("a")]);
        assert_eq!(l.iter().map(|h| h.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }
}
