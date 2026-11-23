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
true"#
    )
}

#[derive(Debug, Default, PartialEq)]
pub struct Bootstrap {
    pub os: String,
    pub tmux: Option<String>,
    pub version: Option<String>,
    pub home: String,
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

const LIST_FIELDS: &str = "#{session_name}\\t#{@myterm_name}\\t#{@myterm_kind}\\t#{pane_current_path}\\t#{pane_current_command}\\t#{session_created}\\t#{session_last_attached}\\t#{session_attached}\\t#{@myterm_state}\\t#{@myterm_state_at}";

pub fn list_script(tmux: &str) -> String {
    format!(
        "F=$(printf '{LIST_FIELDS}')\n{} list-panes -a -F \"$F\"",
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
    Error(#[allow(dead_code)] String),
}

pub fn parse_list(host: &str, code: i32, out: &str) -> ListResult {
    if code != 0 {
        let l = out.to_ascii_lowercase();
        if l.contains("no server running") || l.contains("error connecting to") || l.contains("no such file") {
            return ListResult::Sessions(vec![]);
        }
        return ListResult::Error(out.trim().to_string());
    }
    let mut sessions: Vec<Session> = vec![];
    for line in out.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 10 {
            continue;
        }
        let slug = f[0].to_string();
        if sessions.iter().any(|s| s.slug == slug) {
            continue; // first pane only
        }
        let num = |s: &str| s.trim().parse::<i64>().unwrap_or(0);
        let kind = match f[2] {
            "claude" | "shell" => f[2].to_string(),
            _ if looks_like_claude(f[4]) => "claude".into(),
            _ => "shell".into(),
        };
        let state = match f[8].trim() {
            "" => None,
            s => Some(s.to_string()),
        };
        sessions.push(Session {
            key: format!("{host}/{slug}"),
            host: host.to_string(),
            display_name: if f[1].is_empty() { slug.clone() } else { f[1].to_string() },
            slug,
            kind,
            cwd: f[3].to_string(),
            state,
            state_at: num(f[9]),
            created: num(f[5]),
            last_attached: num(f[6]),
            attached: num(f[7]) as u32,
            persistent: true,
            ephemeral_id: None,
            ended: false,
        });
    }
    ListResult::Sessions(sessions)
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
{t} new-session -d -s "$s" -c {dir} || exit 1
{t} set-option -t "=$s" @myterm_name {name}
{t} set-option -t "=$s" @myterm_kind {kind}
"#,
        base = q(&base),
        dir = q(dir),
        name = q(name),
        kind = q(kind),
    );
    if hide_status {
        s += &format!("{t} set-option -t \"=$s\" status off\n");
    }
    if kind == "claude" {
        let line = format!("claude --rc {}", q(name));
        s += &format!("{t} send-keys -t \"=$s\" -l {}\n{t} send-keys -t \"=$s\" Enter\n", q(&line));
    }
    s += "echo \"SLUG=$s\"";
    s
}

pub fn rename_script(tmux: &str, slug: &str, name: &str) -> String {
    format!("{} set-option -t {} @myterm_name {}", q(tmux), q(&format!("={slug}")), q(name))
}

pub fn kill_script(tmux: &str, slug: &str) -> String {
    format!("{} kill-session -t {}", q(tmux), q(&format!("={slug}")))
}

pub fn mouse_script(tmux: &str, slug: &str, on: bool) -> String {
    format!(
        "{} set-option -t {} mouse {}",
        q(tmux),
        q(&format!("={slug}")),
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
        let out = "web\tWeb App\tclaude\t/home/u/web\tnode\t100\t200\t1\tdone\t150\nweb\t\t\t/x\tzsh\t1\t2\t0\t\t\nsh\t\t\t/tmp\tzsh\t5\t0\t0\t\t";
        let ListResult::Sessions(s) = parse_list("h", 0, out) else { panic!() };
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].display_name, "Web App");
        assert_eq!(s[0].state.as_deref(), Some("done"));
        assert_eq!(s[0].state_at, 150);
        assert_eq!(s[1].display_name, "sh");
        assert_eq!(s[1].kind, "shell");
        assert!(matches!(parse_list("h", 1, "no server running on /tmp/x"), ListResult::Sessions(v) if v.is_empty()));
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
