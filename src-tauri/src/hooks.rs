//! Claude Code attention hooks (section 8).

use anyhow::{bail, Result};
use serde_json::{json, Map, Value};

/// Hook events and the argument passed to the hook script.
pub const EVENTS: &[(&str, &str)] = &[
    ("UserPromptSubmit", "working"),
    ("PostToolUse", "working"),
    ("Notification", "notification"),
    ("Stop", "done"),
    ("SessionEnd", "clear"),
];

const MARK: &str = ".myterm/hook";

pub fn hook_sh(tmux: &str) -> String {
    format!(
        r#"#!/bin/sh
# myterm attention hook, installed by myterm. Usage: hook.sh <event>
# Does nothing outside tmux. Records Claude's state as tmux pane options.
input=$(cat 2>/dev/null)
[ -n "$TMUX" ] && [ -n "$TMUX_PANE" ] || exit 0
T={tmux}
[ -x "$T" ] || T=tmux
case "$1" in
  working|done) s=$1 ;;
  notification)
    case "$input" in
      *permission*) s=permission ;;
      *) exit 0 ;;
    esac ;;
  clear)
    "$T" set-option -p -t "$TMUX_PANE" -u @myterm_state 2>/dev/null
    "$T" set-option -p -t "$TMUX_PANE" -u @myterm_state_at 2>/dev/null
    exit 0 ;;
  *) exit 0 ;;
esac
"$T" set-option -p -t "$TMUX_PANE" @myterm_state "$s" 2>/dev/null
"$T" set-option -p -t "$TMUX_PANE" @myterm_state_at "$(date +%s)" 2>/dev/null
exit 0
"#,
        tmux = crate::channel::q(tmux)
    )
}

/// Windows variant for ephemeral sessions (8.5): writes the state into
/// %USERPROFILE%\.myterm\state\<MYTERM_SESSION>; the file's mtime is the time.
pub const HOOK_CMD: &str = "@echo off\r
rem myterm attention hook, installed by myterm. Usage: hook.cmd <event>\r
setlocal\r
if \"%MYTERM_SESSION%\"==\"\" goto :eof\r
set \"D=%USERPROFILE%\\.myterm\\state\"\r
if not exist \"%D%\" mkdir \"%D%\" >nul 2>&1\r
set \"S=%~1\"\r
if /i \"%S%\"==\"notification\" goto notif\r
if /i \"%S%\"==\"clear\" goto clear\r
if /i \"%S%\"==\"working\" goto write\r
if /i \"%S%\"==\"done\" goto write\r
goto :eof\r
:notif\r
findstr /i \"permission\" >nul 2>&1\r
if errorlevel 1 goto :eof\r
set \"S=permission\"\r
:write\r
>\"%D%\\%MYTERM_SESSION%\" echo %S%\r
goto :eof\r
:clear\r
del /q \"%D%\\%MYTERM_SESSION%\" >nul 2>&1\r
";

/// Merges (install) or strips (uninstall) the myterm hook entries in a
/// Claude Code settings.json, leaving everything else as it was.
/// `script_cmd` is the command prefix, e.g. "/home/u/.myterm/hook.sh".
pub fn merge_settings(existing: &str, script_cmd: &str, install: bool) -> Result<String> {
    let mut root: Value = if existing.trim().is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(existing)?
    };
    let Some(obj) = root.as_object_mut() else {
        bail!("settings.json is not a JSON object");
    };
    let hooks = obj.entry("hooks").or_insert_with(|| Value::Object(Map::new()));
    let Some(hooks) = hooks.as_object_mut() else {
        bail!("\"hooks\" in settings.json is not an object");
    };
    for (event, arg) in EVENTS {
        if let Some(Value::Array(groups)) = hooks.get_mut(*event) {
            for g in groups.iter_mut() {
                if let Some(Value::Array(list)) = g.get_mut("hooks") {
                    list.retain(|h| {
                        !h.get("command").and_then(Value::as_str).is_some_and(|c| c.contains(MARK))
                    });
                }
            }
            groups.retain(|g| {
                g.get("hooks").and_then(Value::as_array).is_none_or(|l| !l.is_empty())
            });
        }
        if install {
            let mut group = json!({
                "hooks": [{ "type": "command", "command": format!("{script_cmd} {arg}") }]
            });
            if *event == "PostToolUse" {
                group["matcher"] = json!("*");
            }
            let arr = hooks.entry(event.to_string()).or_insert_with(|| Value::Array(vec![]));
            if let Some(a) = arr.as_array_mut() {
                a.push(group);
            }
        }
        if hooks.get(*event).and_then(Value::as_array).is_some_and(|a| a.is_empty()) {
            hooks.remove(*event);
        }
    }
    if hooks.is_empty() {
        obj.remove("hooks");
    }
    let mut s = serde_json::to_string_pretty(&root)?;
    s.push('\n');
    Ok(s)
}

pub fn is_installed(settings: &str) -> bool {
    settings.contains(MARK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_roundtrip() {
        let orig = r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say hi"}]}]}}"#;
        let installed = merge_settings(orig, "/h/.myterm/hook.sh", true).unwrap();
        let v: Value = serde_json::from_str(&installed).unwrap();
        assert_eq!(v["hooks"]["Stop"].as_array().unwrap().len(), 2);
        assert_eq!(v["hooks"]["PostToolUse"][0]["matcher"], "*");
        // Keys keep their order.
        assert!(installed.find("model").unwrap() < installed.find("hooks").unwrap());
        // Idempotent.
        assert_eq!(merge_settings(&installed, "/h/.myterm/hook.sh", true).unwrap(), installed);
        // Uninstall restores the other hook only.
        let removed = merge_settings(&installed, "/h/.myterm/hook.sh", false).unwrap();
        let v: Value = serde_json::from_str(&removed).unwrap();
        assert_eq!(v["hooks"].as_object().unwrap().len(), 1);
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "say hi");
        assert!(merge_settings("", "x/.myterm/hook.sh", false).unwrap().trim() == "{}");
    }
}
