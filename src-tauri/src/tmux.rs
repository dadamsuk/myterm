//! tmux shell snippets and output parsers (sections 5.2, 5.5).

use crate::channel::q;
use serde::Serialize;

pub fn bootstrap_script(tmux_override: Option<&str>) -> String {
    let find = match tmux_override {
        Some(p) => format!("T={}", q(p)),
        None => r#"T=$(command -v tmux 2>/dev/null)
[ -z "$T" ] && T=$("${SHELL:-/bin/sh}" -lc 'command -v tmux' 2>/dev/null | tail -n 1)
for p in /opt/homebrew/bin/tmux /usr/local/bin/tmux /usr/bin/tmux; do
  [ -z "$T" ] && [ -x "$p" ] && T=$p
done"#
            .to_string(),
    };
    format!(
        r#"echo "OS=$(uname -s)"
{find}
echo "TMUX=$T"
[ -n "$T" ] && echo "VER=$("$T" -V 2>/dev/null)"
echo "HOME=$HOME"
echo "USER=$(id -un 2>/dev/null)"
true"#
    )
}

#[derive(Debug, Default, PartialEq)]
pub struct Bootstrap {
    pub os: String,
    pub tmux: Option<String>,
    pub version: Option<String>,
    pub home: String,
    pub user: String,
}

pub fn parse_bootstrap(out: &str) -> Bootstrap {
    let mut b = Bootstrap::default();
    for line in out.lines() {
        if let Some(v) = line.strip_prefix("OS=") {
            b.os = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("TMUX=") {
            let v = v.trim();
            if !v.is_empty() && v.starts_with('/') {
                b.tmux = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("VER=") {
            let v = v.trim();
            if !v.is_empty() {
                b.version = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("HOME=") {
            b.home = v.trim().to_string();
        } else if let Some(v) = line.strip_prefix("USER=") {
            b.user = v.trim().to_string();
        }
    }
    b
}

/// Parses "tmux 3.3a" / "tmux next-3.4" / "tmux 3.2" into (major, minor).
pub fn parse_version(v: &str) -> Option<(u32, u32)> {
    let start = v.find(|c: char| c.is_ascii_digit())?;
    let rest = &v[start..];
    let mut parts = rest.split(|c: char| !c.is_ascii_digit());
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

pub fn version_ok(v: &str) -> bool {
    parse_version(v).is_some_and(|(a, b)| (a, b) >= (3, 2))
}

const LIST_FIELDS: &str = "#{session_name}\\t#{@myterm_name}\\t#{@myterm_kind}\\t#{pane_current_path}\\t#{pane_current_command}\\t#{session_created}\\t#{session_last_attached}\\t#{session_attached}\\t#{@myterm_state}\\t#{@myterm_state_at}\\t#{window_id}\\t#{window_index}\\t#{window_active}\\t#{pane_active}\\t#{window_name}";

pub fn list_script(tmux: &str) -> String {
    format!(
        // -u: without a UTF-8 locale (common over non-interactive ssh) tmux
        // prints control characters, including the tab separators, as '_'.
        "F=$(printf '{LIST_FIELDS}')\n{} -u list-panes -a -F \"$F\"",
        q(tmux)
    )
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Session {
    /// Unique key: "host/slug" for tmux sessions, "host/~eph-N" for ephemeral ones.
    pub key: String,
    pub host: String,
    pub slug: String,
    pub display_name: String,
    pub kind: String,
    pub cwd: String,
    pub state: Option<String>,
    pub state_at: i64,
    pub created: i64,
    pub last_attached: i64,
    pub attached: u32,
    pub persistent: bool,
    pub ephemeral_id: Option<u64>,
    pub ended: bool,
    /// tmux windows, ordered by index. Empty for ephemeral sessions.
    pub windows: Vec<Window>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Window {
    /// tmux window id, e.g. "@3"; stable for the window's lifetime.
    pub id: String,
    pub index: u32,
    pub name: String,
    /// The session's current window.
    pub active: bool,
    pub kind: String,
    pub cwd: String,
    pub state: Option<String>,
    pub state_at: i64,
}

/// Picks the state to show for a group of panes or windows: a permission
/// wait wins, otherwise the most recent state.
fn pick_state<'a>(items: impl Iterator<Item = (&'a Option<String>, i64)>) -> (Option<String>, i64) {
    let mut best: (Option<String>, i64) = (None, 0);
    for (s, at) in items {
        let Some(s) = s else { continue };
        let is_perm = s == "permission";
        let best_perm = best.0.as_deref() == Some("permission");
        if best.0.is_none() || (is_perm && !best_perm) || (is_perm == best_perm && at > best.1) {
            best = (Some(s.clone()), at);
        }
    }
    best
}

/// True when a pane's current command looks like Claude Code. The native
/// installer's binary shows up under its version number (e.g. "2.1.3").
pub fn looks_like_claude(cmd: &str) -> bool {
    let c = cmd.to_ascii_lowercase();
    c == "claude" || c.starts_with("claude") || {
        let parts: Vec<&str> = c.split('.').collect();
        parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|ch| ch.is_ascii_digit()))
    }
}

pub enum ListResult {
    Sessions(Vec<Session>),
    /// tmux failed for a reason other than "no server"; keep the last list.
    Error(String),
}

pub fn parse_list(host: &str, code: i32, out: &str) -> ListResult {
    if code != 0 {
        let l = out.to_ascii_lowercase();
        if l.contains("no server running") || l.contains("error connecting to") || l.contains("no such file") {
            return ListResult::Sessions(vec![]);
        }
        return ListResult::Error(out.trim().to_string());
    }
    struct Pane {
        win: Window,
        pane_active: bool,
    }
    let num = |s: &str| s.trim().parse::<i64>().unwrap_or(0);
    // Sessions in first-seen order, each with its panes.
    let mut sessions: Vec<(Session, Vec<Pane>)> = vec![];
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 15 {
            continue;
        }
        let slug = f[0].to_string();
        let i = match sessions.iter().position(|(s, _)| s.slug == slug) {
            Some(i) => i,
            None => {
                sessions.push((
                    Session {
                        key: format!("{host}/{slug}"),
                        host: host.to_string(),
                        display_name: if f[1].is_empty() { slug.clone() } else { f[1].to_string() },
                        slug,
                        kind: String::new(),
                        cwd: String::new(),
                        state: None,
                        state_at: 0,
                        created: num(f[5]),
                        last_attached: num(f[6]),
                        attached: num(f[7]) as u32,
                        persistent: true,
                        ephemeral_id: None,
                        ended: false,
                        windows: vec![],
                    },
                    vec![],
                ));
                sessions.len() - 1
            }
        };
        let kind = match f[2] {
            "claude" | "shell" => f[2].to_string(),
            _ if looks_like_claude(f[4]) => "claude".into(),
            _ => "shell".into(),
        };
        let state = match f[8].trim() {
            "" => None,
            s => Some(s.to_string()),
        };
        sessions[i].1.push(Pane {
            win: Window {
                id: f[10].to_string(),
                index: num(f[11]) as u32,
                name: f[14].to_string(),
                active: f[12].trim() == "1",
                kind,
                cwd: f[3].to_string(),
                state,
                state_at: num(f[9]),
            },
            pane_active: f[13].trim() == "1",
        });
    }
    let out = sessions
        .into_iter()
        .map(|(mut s, panes)| {
            let mut ids: Vec<&str> = vec![];
            for p in &panes {
                if !ids.contains(&p.win.id.as_str()) {
                    ids.push(&p.win.id);
                }
            }
            for id in ids {
                let ps: Vec<&Pane> = panes.iter().filter(|p| p.win.id == id).collect();
                // The window's folder and kind come from its active pane.
                let main = ps.iter().find(|p| p.pane_active).unwrap_or(&ps[0]);
                let mut w = main.win.clone();
                if ps.iter().any(|p| p.win.kind == "claude") {
                    w.kind = "claude".into();
                }
                (w.state, w.state_at) = pick_state(ps.iter().map(|p| (&p.win.state, p.win.state_at)));
                s.windows.push(w);
            }
            s.windows.sort_by_key(|w| w.index);
            let main = s.windows.iter().find(|w| w.active).unwrap_or(&s.windows[0]);
            s.cwd = main.cwd.clone();
            s.kind = if s.windows.iter().any(|w| w.kind == "claude") { "claude".into() } else { "shell".into() };
            (s.state, s.state_at) = pick_state(s.windows.iter().map(|w| (&w.state, w.state_at)));
            s
        })
        .collect();
    ListResult::Sessions(out)
}

pub fn select_window_script(tmux: &str, slug: &str, window_id: &str) -> String {
    format!("{} select-window -t {}", q(tmux), q(&format!("={slug}:{window_id}")))
}

/// Lower-cased name, anything outside [a-z0-9_-] replaced by '-'.
pub fn slugify(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "session".into() } else { s }
}

pub fn expand_home(path: &str, home: &str) -> String {
    if path == "~" {
        home.to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{}/{}", home.trim_end_matches('/'), rest)
    } else {
        path.to_string()
    }
}

/// Creates a session; prints the slug actually used (a numeric suffix is
/// added when the name is taken).
pub fn create_script(tmux: &str, name: &str, dir: &str, kind: &str, hide_status: bool) -> String {
    let base = slugify(name);
    let t = q(tmux);
    let mut s = format!(
        r#"s={base}; i=2
while {t} has-session -t "=$s" 2>/dev/null; do s={base}-$i; i=$((i+1)); done
{t} new-session -d -s "$s" -n {name} -c {dir} || exit 1
{t} set-option -t "=$s:" @myterm_name {name}
{t} set-option -t "=$s:" @myterm_kind {kind}
"#,
        base = q(&base),
        dir = q(dir),
        name = q(name),
        kind = q(kind),
    );
    if hide_status {
        s += &format!("{t} set-option -t \"=$s:\" status off\n");
    }
    if kind == "claude" {
        let line = format!("claude --rc {}", q(name));
        s += &format!("{t} send-keys -t \"=$s:\" -l {}\n{t} send-keys -t \"=$s:\" Enter\n", q(&line));
    }
    s += "echo \"SLUG=$s\"";
    s
}

/// Adds a window to an existing session and makes it current; prints its id.
pub fn new_window_script(tmux: &str, slug: &str, name: &str, dir: &str, kind: &str) -> String {
    let t = q(tmux);
    let mut s = format!(
        "W=$({t} new-window -t {target} -P -F '#{{window_id}}' -n {name} -c {dir}) || exit 1\n",
        target = q(&format!("={slug}:")),
        name = q(name),
        dir = q(dir),
    );
    if kind == "claude" {
        let line = format!("claude --rc {}", q(name));
        s += &format!("{t} send-keys -t \"$W\" -l {}\n{t} send-keys -t \"$W\" Enter\n", q(&line));
    }
    s += "echo \"WIN=$W\"";
    s
}

// Targets for set-option and send-keys are panes, so a session needs the
// trailing colon: "=name" alone is "no such session" on tmux 3.4.
pub fn rename_script(tmux: &str, slug: &str, name: &str) -> String {
    format!("{} set-option -t {} @myterm_name {}", q(tmux), q(&format!("={slug}:")), q(name))
}

pub fn rename_window_script(tmux: &str, slug: &str, window_id: &str, name: &str) -> String {
    format!("{} rename-window -t {} {}", q(tmux), q(&format!("={slug}:{window_id}")), q(name))
}

pub fn kill_window_script(tmux: &str, slug: &str, window_id: &str) -> String {
    format!("{} kill-window -t {}", q(tmux), q(&format!("={slug}:{window_id}")))
}

pub fn kill_script(tmux: &str, slug: &str) -> String {
    format!("{} kill-session -t {}", q(tmux), q(&format!("={slug}")))
}

pub fn mouse_script(tmux: &str, slug: &str, on: bool) -> String {
    format!(
        "{} set-option -t {} mouse {}",
        q(tmux),
        q(&format!("={slug}:")),
        if on { "on" } else { "off" }
    )
}

/// Lists directories matching a typed prefix (already ~-expanded).
pub fn complete_script(prefix: &str) -> String {
    let (dir, part) = match prefix.rfind('/') {
        Some(i) => (&prefix[..=i], &prefix[i + 1..]),
        None => ("./", prefix),
    };
    format!(
        "for f in {}*; do [ -d \"$f\" ] && printf '%s/\\n' \"$f\"; done 2>/dev/null | head -n 200",
        q(&format!("{dir}{part}"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(version_ok("tmux 3.2"));
        assert!(version_ok("tmux 3.3a"));
        assert!(version_ok("tmux next-3.4"));
        assert!(version_ok("tmux 10.0"));
        assert!(!version_ok("tmux 3.1c"));
        assert!(!version_ok("tmux 2.9"));
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("My Project!"), "my-project");
        assert_eq!(slugify("a_b-c"), "a_b-c");
        assert_eq!(slugify("  "), "session");
    }

    #[test]
    fn claude_detection() {
        assert!(looks_like_claude("claude"));
        assert!(looks_like_claude("2.1.3"));
        assert!(!looks_like_claude("zsh"));
        assert!(!looks_like_claude("1.2"));
    }

    #[test]
    fn list_parsing() {
        // web: window @1 (two panes, one waiting for permission), window @2 active.
        let out = "\
web\tWeb App\tclaude\t/home/u/web\tnode\t100\t200\t1\tdone\t150\t@1\t0\t0\t1\teditor
web\tWeb App\tclaude\t/x\tzsh\t100\t200\t1\tpermission\t120\t@1\t0\t0\t0\teditor
web\tWeb App\tclaude\t/home/u/logs\ttail\t100\t200\t1\t\t\t@2\t1\t1\t1\tlogs
sh\t\t\t/tmp\tzsh\t5\t0\t0\t\t\t@3\t0\t1\t1\tzsh";
        let ListResult::Sessions(s) = parse_list("h", 0, out) else { panic!() };
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].display_name, "Web App");
        assert_eq!(s[0].windows.len(), 2);
        let w = &s[0].windows[0];
        assert_eq!((w.id.as_str(), w.name.as_str(), w.active), ("@1", "editor", false));
        assert_eq!(w.cwd, "/home/u/web");
        assert_eq!(w.state.as_deref(), Some("permission"));
        assert!(s[0].windows[1].active);
        assert_eq!(s[0].cwd, "/home/u/logs");
        assert_eq!(s[0].state.as_deref(), Some("permission"));
        assert_eq!(s[0].state_at, 120);
        assert_eq!(s[1].display_name, "sh");
        assert_eq!(s[1].kind, "shell");
        assert_eq!(s[1].windows.len(), 1);
        assert!(matches!(parse_list("h", 1, "no server running on /tmp/x"), ListResult::Sessions(v) if v.is_empty()));
    }

    #[test]
    fn pane_targets_have_colon() {
        let c = create_script("/t", "My App", "/d", "claude", true);
        assert!(!c.contains("set-option -t \"=$s\"") && !c.contains("send-keys -t \"=$s\""));
        assert_eq!(c.matches("\"=$s:\"").count(), 5);
        assert!(c.contains("has-session -t \"=$s\""));
        assert!(rename_script("/t", "web", "x").contains("'=web:'"));
        assert!(mouse_script("/t", "web", true).contains("'=web:'"));
        assert!(list_script("/t").contains("'/t' -u list-panes"));
    }

    #[test]
    fn bootstrap_parsing() {
        let b = parse_bootstrap("OS=Darwin\nTMUX=/opt/homebrew/bin/tmux\nVER=tmux 3.5a\nHOME=/Users/u");
        assert_eq!(b.os, "Darwin");
        assert_eq!(b.tmux.as_deref(), Some("/opt/homebrew/bin/tmux"));
        assert_eq!(b.home, "/Users/u");
        let b = parse_bootstrap("OS=Linux\nTMUX=\nHOME=/root");
        assert_eq!(b.tmux, None);
    }

    #[test]
    fn home_expansion() {
        assert_eq!(expand_home("~/src", "/home/u"), "/home/u/src");
        assert_eq!(expand_home("~", "/home/u"), "/home/u");
        assert_eq!(expand_home("/etc", "/home/u"), "/etc");
    }
}
