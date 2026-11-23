# myterm — specification

Status: draft for review. Nothing is built yet.
Working name: `myterm` (from the folder; rename freely).

## 1. Purpose

A small desktop app that shows every long-running terminal session across all my machines in one list and attaches to any of them with one click or a few keystrokes. Most of the time I should not need to care which computer a session lives on.

The primary workload is long-running Claude Code sessions started with `--rc '<name>'`, plus ordinary shells.

## 2. Decisions already made

| Topic | Decision |
|---|---|
| Client shape | GUI with an embedded terminal, one terminal visible at a time |
| Client platforms | Windows and macOS. Linux desktop is not a target |
| Stack | Tauri 2 (Rust backend) + xterm.js (WebGL renderer) |
| Transport | The system `ssh` binary, run in a local PTY. No SSH library |
| Host list | Parsed from `~/.ssh/config`; that file is the only way to define hosts |
| Persistence on macOS/Linux hosts | tmux, one tmux session per destination |
| Persistence on Windows hosts | None in v1: sessions are "ephemeral". A keeper mode is planned for v2 |
| Local sessions | Yes; the local machine is just another host |
| Needs-attention badges | In v1, driven by Claude Code hooks |
| Quick switcher | Ctrl+Shift+O |
| Command palette | Ctrl+Shift+P |
| Hover preview | Not wanted |

## 3. Non-goals

- Splits, panes, tabs or multiple visible terminals.
- A custom wire protocol or a remote daemon on macOS/Linux hosts.
- Managing SSH keys, passwords, or host definitions.
- Replacing tmux for people attaching from a plain terminal; sessions stay ordinary tmux sessions.
- Linux desktop client support.

## 4. Concepts

**Host.** One entry from `~/.ssh/config`, or the local machine. Each host has a detected kind:

- `tmux` — macOS or Linux with tmux 3.2 or newer. Sessions persist and are discoverable.
- `ephemeral` — Windows (remote or local). Sessions exist only while the app holds them open.
- `unavailable` — unreachable, or no tmux found.

**Session.** One destination in the list. On tmux hosts it is exactly one tmux session. Sessions the app creates have one window and one pane. Sessions with several windows list them under the session (5.2.1). Fields:

| Field | Source |
|---|---|
| `host` | ssh config alias, or `local` |
| `slug` | tmux session name; restricted to `[a-z0-9_-]` |
| `display_name` | tmux user option `@myterm_name`; falls back to the slug. This is the `--rc` name |
| `kind` | `claude` or `shell`; from `@myterm_kind`, else inferred from the pane's current command |
| `cwd` | `#{pane_current_path}` |
| `state` | see section 8 |
| `last_used` | later of the app's own "last viewed" time and tmux `#{session_last_attached}` |
| `persistent` | true on tmux hosts, false for ephemeral sessions |

Existing tmux sessions that the app did not create are adopted automatically and shown like any other.

## 5. Architecture

```
┌──────────────────────────── myterm (Tauri) ────────────────────────────┐
│  Frontend (TypeScript)                Backend (Rust)                   │
│  ┌──────────┐ ┌──────────────┐        ┌──────────────────────────────┐ │
│  │ Sidebar  │ │ xterm.js     │◄─────► │ PTY manager (portable-pty)   │ │
│  │ list     │ │ terminal     │ bytes  │  one `ssh -t` per open       │ │
│  └──────────┘ └──────────────┘        │  session                     │ │
│  ┌──────────────────────────┐         ├──────────────────────────────┤ │
│  │ Switcher / palette       │◄─────►  │ Host channels                │ │
│  └──────────────────────────┘ events  │  one `ssh -T host sh` per    │ │
│                                       │  host: polling + commands    │ │
│                                       ├──────────────────────────────┤ │
│                                       │ ssh config parser, state     │ │
│                                       └──────────────────────────────┘ │
└────────────────────────────────────────────────────────────────────────┘
```

### 5.1 Host channel

Windows OpenSSH has no connection multiplexing, so opening a fresh connection for every poll and every command would be slow. Instead the backend keeps **one long-lived non-interactive connection per host**:

```
ssh -T -o BatchMode=yes -o ConnectTimeout=5 \
    -o ServerAliveInterval=15 -o ServerAliveCountMax=2 <host> /bin/sh
```

The backend writes shell snippets to its stdin. Each snippet ends by printing a unique sentinel line containing a request id and the exit code, so replies can be framed and matched. All discovery, session creation, rename, kill and path completion go through this channel. For the local machine on macOS the same protocol runs against a local `/bin/sh` with no ssh.

Channel behaviour:

- **Bootstrap** on connect: detect OS (`uname -s`), locate tmux, read its version. Non-interactive ssh sessions do not load the login profile, so tmux installed by Homebrew is often not on `PATH`. Bootstrap tries `command -v tmux`, then the login shell's `PATH`, then `/opt/homebrew/bin` and `/usr/local/bin`. The resolved absolute path is cached per host and can be overridden in config.
- **Polling:** every 1 second the backend sends the list command (5.2). The frontend is told only when the result differs from the last one.
- **Failure:** if the channel dies, the host is marked `unavailable`, its sessions are greyed out, and the backend reconnects with backoff (1, 2, 5, 10, then every 30 seconds).
- **BatchMode** means the channel never prompts. A host that needs a passphrase or host-key confirmation shows "needs interactive login"; clicking it opens a normal interactive `ssh` in the terminal pane so the prompt can be answered.

### 5.2 Discovery

One command lists everything, one line per pane, tab-separated:

```
tmux list-panes -a -F '#{session_name}\t#{@myterm_name}\t#{@myterm_kind}\t#{pane_current_path}\t#{pane_current_command}\t#{session_created}\t#{session_last_attached}\t#{session_attached}\t#{@myterm_state}\t#{@myterm_state_at}'
```

The same command also prints `#{window_id}`, `#{window_index}`, `#{window_active}`, `#{pane_active}` and `#{window_name}`, so panes are grouped into windows within each session. A window's folder and kind come from its active pane. Its state is that of its panes, with `permission` winning and otherwise the most recent. A session's folder is its current window's, and its state is the same roll-up over its windows.

### 5.2.1 Windows

The list shows tmux windows, not sessions: every window is its own row directly under its host, with its own name (the tmux window name), folder and state dot. The tmux session level is not shown; when a host has more than one tmux session, each row carries its session's name as a small tag. Ephemeral sessions have no windows and are one row each. There is still one attach PTY per session. Clicking a window row runs `tmux select-window -t =<slug>:<window_id>` and shows the session's terminal. Since a tmux session has one current window shared by every client, any other client attached to that session follows the switch. Last-viewed times for windows are keyed `host/slug:<window_id>`, so a `done` badge on one window clears only when that window has been on screen.

### 5.3 Attaching

Each opened session gets its own xterm.js instance and its own PTY running:

```
ssh -t -o ServerAliveInterval=15 -o ServerAliveCountMax=2 <host> \
    <tmux> -T RGB,clipboard attach-session -t =<slug>
```

- `=<slug>` forces an exact name match.
- `-T RGB,clipboard` declares truecolor and clipboard support for this client without editing the host's `tmux.conf`.
- Before the first attach, the backend sets `mouse on` on that session only, so the wheel scrolls tmux history. This is a per-session option and is configurable.
- `TERM` is `xterm-256color`.

Opened sessions stay connected in the background so switching back is instant. At most 8 stay open (configurable); the least recently viewed is detached beyond that. Detaching never affects the remote session.

### 5.4 Reconnect

When an attach PTY exits unexpectedly (sleep, network change), the terminal pane shows a "reconnecting" overlay and retries with the same backoff as the host channel. On success tmux redraws the screen. Ephemeral sessions cannot reconnect; they are marked ended and their scrollback stays readable until dismissed.

### 5.5 Creating a session

If the tmux host already has sessions, nothing new is created at session level: the new work becomes a window in the most recently used session on that host, which is then shown:

```
tmux new-window -t =<slug>: -P -F '#{window_id}' -n '<display name>' -c <dir>
tmux send-keys  -t <window_id> "claude --rc '<display name>'" Enter
```

Otherwise, through the host channel:

```
tmux new-session -d -s <slug> -c <dir>
tmux set-option -t =<slug> @myterm_name '<display name>'
tmux set-option -t =<slug> @myterm_kind claude
tmux set-option -t =<slug> status off
tmux send-keys  -t =<slug> "claude --rc '<display name>'" Enter
```

The session starts as a shell and Claude is typed into it. This way Claude gets the login environment, and when Claude exits the session and its folder remain as a shell rather than disappearing. A shell session is the same without the last step.

`status off` hides the tmux status bar for sessions the app creates, since the sidebar and header already show that information. It is a per-session option, so it also applies when attaching to that session from a plain terminal. Adopted sessions are left as they are. A config switch (`hide_tmux_status`, default true) turns this off.

Slug rules: lower-cased display name, anything outside `[a-z0-9_-]` replaced by `-`, with a numeric suffix if the slug is already taken on that host. All user-supplied strings (names, paths) are single-quote escaped before being sent to a remote shell.

### 5.6 Ephemeral sessions (Windows hosts, and local Windows)

- Remote Windows: the app opens `ssh -t <host>` in a PTY, changes to the chosen folder, and runs `claude --rc "<name>"`.
- Local Windows: the app spawns the default shell in a ConPTY in the chosen folder and runs the same command.
- These sessions appear in the list with an "ephemeral" marker. They end when the connection drops or the app quits, and the app warns on quit if any are running.
- Sessions on a Windows host that the app did not start cannot be discovered.

## 6. User interface

### 6.1 Layout

- **Sidebar (left, resizable, collapsible):** the session list with a three-way toggle at the top.
- **Header (thin, above the terminal):** display name, host, folder, state, connection status.
- **Terminal (rest of the window):** one session at a time.

### 6.2 Session list

Rows are tmux windows (5.2.1). Row contents: state dot, window name, session tag (only on hosts with several tmux sessions), host tag, folder basename, relative last-used time. Within a host, rows follow tmux order: session, then window index. Ephemeral and unreachable rows are visually distinct.

Toggle modes:

| Mode | Order |
|---|---|
| Machine | Grouped under host headings; within a group, by name |
| Name | Flat, alphabetical |
| Recent | Flat, most recently used first (default) |

In every mode, sessions that need attention are pinned in a "Needs you" section at the top. This pinning can be turned off.

Right-click on a row: rename, kill (immediate, no confirmation), copy the manual `ssh … tmux attach` command. Rename runs `tmux rename-window` on the host, so the name is tmux's own and shows in its status bar too. Kill runs `kill-window`; killing a session's last window ends the session.

### 6.3 Quick switcher — Ctrl+Shift+O

A centred overlay with one input. Fuzzy matching runs over display name, folder and host together. Enter attaches. With an empty query the order is: sessions needing attention, then most recently used, so Ctrl+Shift+O then Enter toggles to the previous session.

### 6.4 Command palette — Ctrl+Shift+P

Everything doable with the mouse is available here:

- New Claude session…
- New shell session…
- Rename session / Kill session
- Group by machine / Sort by name / Sort by recent
- Reconnect host / Hide host / Show hidden hosts
- Install attention hooks on host…
- Toggle sidebar / Open settings file

### 6.5 New-session flow

Three fields, keyboard-driven:

1. **Folder.** Fuzzy match over known folders across all hosts (every folder a session has been seen in, plus recently used ones). Typing a path beginning with `/` or `~` switches to live remote completion on the selected host, via the host channel.
2. **Host.** Pre-filled with the host where the chosen folder was last used; changeable.
3. **Name.** Pre-filled with the folder basename.

Enter creates the session and attaches.

In Machine mode each reachable host heading has a **+** button that opens this flow with that host chosen, the folder set to `~` and only that host's folders offered; Enter in the folder field creates at once.

The form has a Claude / Shell choice (Alt+C / Alt+S). Shell runs nothing. The choice is remembered for the next time the form opens from **+**. New sessions and windows are named after the Name field (`new-session -n`, `new-window -n`).

### 6.6 Keys

| Key | Action |
|---|---|
| Ctrl+Shift+O | Quick switcher |
| Ctrl+Shift+P | Command palette |
| Ctrl+Shift+N | New Claude session |
| Ctrl+Shift+C / Ctrl+Shift+V | Copy / paste (Cmd+C / Cmd+V on macOS) |
| Shift+Enter | Sends ESC + CR, which Claude Code reads as "newline" |

On macOS, Cmd+Shift is accepted as an alternative to Ctrl+Shift. Every other key goes to the terminal untouched. Bindings are configurable.

## 7. Terminal quality requirements

- xterm.js with the WebGL renderer, falling back to the DOM renderer if WebGL is unavailable.
- Addons: fit, unicode11, web-links, clipboard (OSC 52).
- Truecolor, mouse reporting, bracketed paste, clickable URLs.
- PTY output is delivered to the frontend as binary over Tauri channels, batched per frame, with acknowledgement-based backpressure so a flood of output cannot freeze the UI.
- Resize is debounced and propagated to the PTY; tmux follows the most recently active client's size.
- Selection: with tmux mouse mode on, dragging selects in tmux and copies through OSC 52. Shift+drag does a native selection.
- Font family, size and colour theme come from the settings file.
- Scrollback is tmux's. There is no native scrollbar for tmux sessions; ephemeral sessions use xterm.js scrollback.

## 8. Needs-attention

### 8.1 States

| State | Meaning | Shown as |
|---|---|---|
| `working` | Claude is running a turn | Subtle animated dot |
| `permission` | Claude is blocked on a permission prompt | Strong badge; pinned to "Needs you" |
| `done` | Claude finished its turn and is waiting for me | Badge until I view the session, then cleared |
| none | Plain shell, or hooks not installed | No dot |

"Viewed" is tracked locally: a `done` state counts as unseen when its timestamp is later than the last time I had that session on screen.

### 8.2 Mechanism

Claude Code hooks call a small script that the app installs at `~/.myterm/hook.sh`. The script does nothing unless it is running inside tmux, and otherwise sets two pane options:

```
tmux set-option -p -t "$TMUX_PANE" @myterm_state <state>
tmux set-option -p -t "$TMUX_PANE" @myterm_state_at <unix time>
```

Hook mapping, added to `~/.claude/settings.json` on each host:

| Claude Code hook | State written |
|---|---|
| `UserPromptSubmit` | `working` |
| `PostToolUse` | `working` (clears a permission wait after approval) |
| `Notification` (permission prompt) | `permission` |
| `Stop` | `done` |
| `SessionEnd` | cleared |

The states arrive through the normal 1-second poll, so badges appear within about a second.

### 8.3 Installation

Palette command "Install attention hooks on host…". It shows the exact changes first, then writes the script and merges the hook entries into the existing `settings.json` without disturbing other hooks. It is idempotent and has a matching uninstall command.

### 8.4 Desktop notifications

Optional OS notification when a session I am not looking at enters `permission`. Off by default, since Claude desktop may already notify.

### 8.5 Ephemeral sessions

On Windows there is no tmux to hold the state. The hook script variant writes a small state file under `%USERPROFILE%\.myterm\state\`, keyed by a session id the app passes in an environment variable at launch, and the host channel reads that folder. This is the last milestone and may slip to v1.1.

## 9. Host discovery

- Parse `~/.ssh/config`, following `Include` directives and globs.
- Every `Host` pattern without wildcards (`*`, `?`, `!`) becomes a host. `Match` blocks are ignored for discovery; ssh still applies them when connecting.
- A `Host` block that contains the comment line `# myterm: ignore` is left out entirely: none of that line's aliases are listed or connected to. ssh ignores the comment. The marker only applies to the block it appears in, so it must come after the `Host` line.
- Hosts can be hidden from the app. Hidden hosts are not connected to.
- The local machine appears as `local`. A setting names the ssh alias that refers to this machine, so it is not listed twice.
- The config file is watched and re-parsed on change.

## 10. Local storage

| File | Contents | Edited by |
|---|---|---|
| `config.toml` | Font, theme, key bindings, hidden hosts, per-host overrides (tmux path, mouse mode), `hide_tmux_status`, open-session cap (default 8), pinning and notification switches | Me |
| `state.json` | Last-viewed times, known folders per host, cached host kind and tmux path, window geometry, list mode | The app |

Location: `%APPDATA%\myterm\` on Windows, `~/Library/Application Support/myterm/` on macOS. No credentials are stored.

## 11. Technology

| Layer | Choice |
|---|---|
| Shell | Tauri 2 |
| Backend | Rust: `portable-pty`, `tokio`, `serde`, `notify` |
| Frontend | TypeScript + Svelte, `@xterm/xterm` and addons |
| Fuzzy matching | Small in-process matcher shared by switcher, palette and folder picker |
| Packaging | Tauri bundler: `.msi`/`.exe` for Windows, `.dmg` for macOS |

## 12. Milestones

Each milestone ends in something usable.

| # | Deliverable | Done when |
|---|---|---|
| 1 | Tauri app with xterm.js and a local PTY | A local shell runs full-screen programs correctly; fast output stays smooth; resize, copy/paste and Shift+Enter work on Windows and macOS |
| 2 | ssh config parsing, host channels, discovery, click to attach | All tmux sessions on all reachable hosts appear within a few seconds of launch; clicking attaches with truecolor, mouse scroll and clipboard working; list toggle works |
| 3 | New-session flow, rename, kill, local sessions | A Claude session can be created on any tmux host in a chosen folder from the keyboard alone |
| 4 | Quick switcher and command palette | Every sidebar action is reachable without the mouse |
| 5 | Attention hooks and badges | Hooks install from the palette; `permission` and `done` appear within about a second and clear correctly |
| 6 | Reconnect, ephemeral Windows sessions, packaging | Sleeping and waking the laptop restores the visible session unaided; a Claude session can be started on the Windows host; installers build |

## 13. To verify during the build

These are assumptions that need a quick test before the dependent code is written.

1. `tmux -T RGB,clipboard attach` gives truecolor and OSC 52 clipboard through xterm.js, on the tmux versions actually installed.
2. The exact Claude Code hook names and the matcher for permission notifications, against the installed Claude Code version.
3. Whether `--rc` accepts any free-text name, to confirm that only the tmux slug needs sanitising.
4. The pane's current command while Claude runs (`claude` or `node`), for inferring the kind of adopted sessions.
5. The default shell that the Windows host's sshd gives (cmd or PowerShell), which decides the launch command syntax in 5.6.
6. WebGL renderer behaviour in macOS WKWebView under heavy output.

## 14. Planned for v2

**Windows keeper.** The app binary gains a headless mode (`myterm keep`) that runs on a Windows machine, holds ConPTY sessions open, and answers list/attach/create over stdio through ssh. This makes Windows sessions persistent and discoverable, for the Windows host and for local sessions on Windows laptops.

Other candidates: floating "previous session" toggle key, per-session notes, sharing `state.json` known folders across my client machines.

## 15. Resolved questions

1. Sessions created by the app hide the tmux status bar (section 5.5); configurable.
2. Killing a session never asks for confirmation.
3. The cap on background connections is 8.
