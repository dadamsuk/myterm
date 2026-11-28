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
| Client platforms | Windows and macOS; Android (14.4). Linux desktop is not a target |
| Stack | Tauri 2 (Rust backend) + xterm.js (WebGL renderer) |
| Transport | The system `ssh` binary, run in a local PTY. On Android, and for desktop hosts set to `transport = "builtin"`, a built-in SSH client instead (14.4) |
| Host list | A list in `config.toml`, managed from the Hosts dialog or by hand; names are ssh destinations (usually `~/.ssh/config` aliases), which keeps the connection details |
| Persistence on macOS/Linux hosts | tmux, one tmux session per destination |
| Persistence on Windows hosts | A keeper process inside the myterm install holds them (14.1); hosts without it fall back to v1's "ephemeral" sessions |
| Local sessions | Yes; the local machine is just another host |
| Needs-attention badges | In v1, driven by Claude Code hooks |
| Quick switcher | Ctrl+Shift+O |
| Command palette | Ctrl+Shift+P |
| Hover preview | Not wanted |

## 3. Non-goals

- Splits, panes, tabs or multiple visible terminals.
- A custom wire protocol or a remote daemon on macOS/Linux hosts.
- Managing SSH keys, passwords, or connection details (HostName, User, ...); those stay in `~/.ssh/config`.
- Replacing tmux for people attaching from a plain terminal; sessions stay ordinary tmux sessions.
- Linux desktop client support.

## 4. Concepts

**Host.** One entry in the host list (section 9): an ssh destination, usually an alias from `~/.ssh/config`, or the local machine. Each host has a detected kind:

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
  - **Auto-hide mode** (toolbar button or palette): the sidebar is hidden and the terminal takes the full width. Resting the pointer on the left edge of the window, or pressing Ctrl+Alt+B, slides it out over the terminal; it goes away when the pointer leaves it, a row is chosen, the terminal is clicked, or Esc / Ctrl+Alt+B is pressed. Ctrl+Alt+B also brings out a collapsed sidebar the same way. The mode is remembered.
- **Header (thin, above the terminal):** display name, host, folder, state, connection status.
- **Terminal (rest of the window):** one session at a time.

### 6.2 Session list

Rows are tmux windows (5.2.1). Row contents: state dot, window name, session tag (only on hosts with several tmux sessions), host tag, folder basename, relative last-used time. Within a host, rows follow tmux order: session, then window index. Ephemeral and unreachable rows are visually distinct.

Toggle modes:

| Mode | Order |
|---|---|
| Machine | Grouped under host headings; within a group, by name |
| Name | Alphabetical. A name used by more than one session (ignoring case) gets a heading, with one row per machine beneath it (folder and time on each); a name used once is a single row at the top level |
| Recent | Flat, most recently used first (default) |

In every mode, sessions that need attention stay where they are in the list and are highlighted: an orange bar and tint for a permission wait, green for finished-but-unseen. Setting `pin_attention = true` also moves them into a "Needs you" section at the top.

Right-click on a row: rename, kill (immediate, no confirmation), refresh, copy the manual `ssh … tmux attach` command. Refresh (also in the terminal's right-click menu and the palette) replaces a tmux session's terminal with a new one attached afresh, as reopening myterm does, for a screen left garbled; tmux redraws it. Rename runs `tmux rename-window` on the host, so the name is tmux's own and shows in its status bar too. Kill runs `kill-window`; killing a session's last window ends the session.

### 6.3 Quick switcher — Ctrl+Shift+O

A centred overlay with one input. Fuzzy matching runs over display name, folder and host together. Enter attaches. With an empty query the order is: sessions needing attention, then most recently used, so Ctrl+Shift+O then Enter toggles to the previous session.

### 6.4 Command palette — Ctrl+Shift+P

Everything doable with the mouse is available here:

- New Claude session…
- New shell session…
- Rename session / Kill session
- Group by machine / Sort by name / Sort by recent
- Hosts… (the Hosts dialog) / Connect host / Reconnect host / Remove host from the list
- Install attention hooks on host…
- Toggle sidebar / Toggle sidebar auto-hide / Show sidebar / Open settings file

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
| Ctrl+Shift+B | Collapse or dock the sidebar (in auto-hide mode: show or hide it) |
| Ctrl+Alt+B | Bring out the hidden sidebar over the terminal, or put it away |
| Ctrl+Shift+C / Ctrl+Shift+V | Copy / paste (Cmd+C / Cmd+V on macOS) |
| Shift+Enter | Sends ESC + CR, which Claude Code reads as "newline" |

On macOS, Cmd+Shift is accepted as an alternative to Ctrl+Shift. Every other key goes to the terminal untouched. Bindings are configurable.

## 7. Terminal quality requirements

- xterm.js with the WebGL renderer, falling back to the DOM renderer if WebGL is unavailable.
- Addons: fit, unicode11, web-links, clipboard (OSC 52).
- Truecolor, mouse reporting, bracketed paste, clickable URLs.
- Images and files for programs on the host (e.g. Claude Code, which accepts an image path in its prompt):
  - Paste with no text on the clipboard but an image: the image is saved as PNG and its path is pasted.
  - Dropping files onto the terminal pastes their paths.
  - For remote hosts the file is first uploaded through the host channel (base64 in a heredoc, up to 50 MB) into `${TMPDIR:-/tmp}/myterm-<uid>/` (mode 700); locally it goes to the OS temp dir under `myterm/`. Files older than a day are removed on the next upload.
  - Remote hosts without a `/bin/sh` channel (Windows over ssh) can't receive uploads; the app shows an error.
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
| `permission` | Claude is blocked on a permission prompt | Strong badge; row highlighted in place |
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

## 9. Hosts

- The hosts shown, and their order, are a list in `config.toml`, one `[[host]]` table each:

  ```toml
  [[host]]
  name = "local"
  auto_connect = true

  [[host]]
  name = "intra"        # an alias from ~/.ssh/config
  auto_connect = false
  ```

- `name` is passed to `ssh` as the destination, so it is normally a `~/.ssh/config` alias (HostName, User, keys and so on stay there), but `user@server` works too. `local` is this machine.
- **Order** is the order of the list: Machine mode groups by it, and the Hosts panel follows it.
- **auto_connect** (default true): the host channel connects when the app starts. When false the host is listed as "not connected" and connects the first time it is clicked (host heading, Hosts panel, or the palette's "Connect host"); after that it behaves like any other host until the app quits.
- **The Hosts dialog** (host button in the sidebar toolbar, "Add, order and auto-connect…" in the Hosts panel, or "Hosts…" in the palette) edits this list: add a host by typing a name, with the `~/.ssh/config` aliases offered as suggestions; drag or use the arrows to reorder; toggle auto-connect; remove. Every change is written straight to `config.toml`, keeping the rest of the file, so hand edits and the dialog can be mixed.
- Removing a host only takes it off the list: no connection is made to it. Its tmux sessions are not touched.
- Aliases for suggestions come from `~/.ssh/config`, following `Include` directives and globs. Every `Host` pattern without wildcards (`*`, `?`, `!`) is offered.
- **First run / upgrade:** when `config.toml` has no host list yet, one is built from `~/.ssh/config`: `local` first, then every alias in file order, leaving out the `local_alias`, any old `hidden_hosts` entries, and blocks carrying the old `# myterm: ignore` marker. It is written to `config.toml` and the old `hidden_hosts` key is removed; from then on the ssh config no longer decides which hosts are shown, and the markers can be deleted.
- `config.toml` is watched and applied on change.

## 10. Local storage

| File | Contents | Edited by |
|---|---|---|
| `config.toml` | Font, theme, key bindings, the host list (order, auto-connect), per-host overrides (tmux path, mouse mode), `hide_tmux_status`, open-session cap (default 8), pinning and notification switches | Me |
| `state.json` | Last-viewed times, known folders per host, cached host kind and tmux path, window geometry, list mode, sidebar width / collapsed / auto-hide | The app |

Location: `%APPDATA%\myterm\` on Windows, `~/Library/Application Support/myterm/` on macOS. No credentials are stored.

## 11. Technology

| Layer | Choice |
|---|---|
| Shell | Tauri 2 |
| Backend | Rust: `portable-pty`, `tokio`, `serde`, `notify`; `russh` (ring crypto) for the built-in SSH client |
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

The main item is the Windows keeper (14.1). Other candidates: floating "previous session" toggle key, per-session notes, sharing `state.json` known folders across my client machines.

### 14.1 Windows keeper

**Goal.** Sessions on a Windows machine survive disconnects, closing the UI and app updates, and other myterm clients can discover them, the same as tmux sessions. This applies to remote Windows hosts and to local sessions on a Windows client. No WSL, no admin rights, nothing to install beyond myterm itself.

**Why the sessions can't be started over ssh.** Win32-OpenSSH puts every process started in an ssh session into a job object and kills the job when the connection closes, detached children included. So the sessions are started by a process that already runs on the machine outside any ssh session: the keeper. An ssh connection only ever relays bytes to it.

**Processes.**

| Process | Role |
|---|---|
| `myterm.exe --keeper` | Headless, one per user per machine. Owns every session's ConPTY and the control pipe. Creates its ConPTYs directly, not through portable-pty, with the inbox ConPTY and no creation flags (see "Verified" below). Started by the UI if not running; keeps running after the UI quits. Optional "start keeper at login" (HKCU `Run` key) keeps the machine reachable with no window open. In the desktop logon it shows a notification-area icon (tooltip: session count) whose menu has "Start myterm" (brings an open window forward, otherwise starts `myterm.exe`, whose path the UI and `myterm-cli` record as `app` in keeper.json; a left click does the same) and "Quit" (asks first when sessions are running, then ends them and exits). A keeper in session 0 has no icon |
| myterm UI | Lists, creates, renames and kills over the pipe (locally) or `myterm-cli channel` (remote). Attaching runs `myterm-cli attach <id>` in the UI's own PTY (over `ssh -t` for remote hosts), so keeper sessions take the same attach, reconnect and eviction path in the frontend as tmux sessions, and local sessions the same path as remote ones |
| `myterm-cli.exe` | Small console-subsystem helper shipped next to `myterm.exe` (a bundle resource, built by `beforeBuildCommand`), because the GUI-subsystem `myterm.exe` handles stdio poorly under ssh. `channel` relays its stdio to the pipe, starting the keeper first if needed; `attach <id>` relays a console; `start`, `request <json>` and `version` are for diagnostics. Debug builds are console programs, so there `myterm.exe --cli …` does the same |

The keeper runs from a per-version copy, `%LOCALAPPDATA%\myterm\keeper\myterm-keeper-<version>.exe`, so an installer can replace `myterm.exe` while it runs. Each new session gets the user's default environment, built fresh from the registry (`CreateEnvironmentBlock`), plus `TERM`, `COLORTERM` and `MYTERM_SESSION`, rather than whatever environment started the keeper. On start the UI connects to its keeper; one of another version with no sessions is told to shut down and replaced.

The keeper creates the first pipe instance with `FILE_FLAG_FIRST_PIPE_INSTANCE`, so a second keeper fails to create it (access denied) and exits; pipe names are machine-global, so this works across the desktop and ssh logons and no separate mutex is needed. The keeper writes its pipe name to `%LOCALAPPDATA%\myterm\keeper.json`, and the UI adds `cli` (the command words that run `myterm-cli`), so a client connecting over ssh reads that file (`type` or `Get-Content`, depending on the host's default shell) to find it. A Windows host where myterm has never been run has no such file and stays `ephemeral`.

**Control pipe.** `\\.\pipe\myterm-keeper-<user SID>`, created with the DACL `D:P(A;;GA;;;<user SID>)` (the owning user only) and `PIPE_REJECT_REMOTE_CLIENTS`. Pipe names are machine-global, so a pipe created in the desktop logon can be reached from an ssh logon of the same user, and the other way round. No mandatory label is needed: a medium-integrity desktop client can open a pipe created by a high-integrity keeper. Requests are one JSON line each; replies are one JSON line, except `attach`, which switches the connection to a framed byte stream.

| Request | Effect |
|---|---|
| `list` | Every session: id, display name, kind, cwd, state, created, last attached, attached client count, exit status if ended |
| `create {name, kind, cwd, cols, rows}` | Starts the default shell (or `local_shell`) in a ConPTY in `cwd`; for `claude` runs `claude --rc "<name>"`. Passes the session id in `MYTERM_SESSION` for the hooks |
| `rename {id, name}` / `kill {id}` | As for tmux windows (6.2) |
| `attach {id, cols, rows}` | Output and input stream, plus resize frames |
| `upload {name, bytes}` | Writes into `%TEMP%\myterm\` (7, file drop), so Windows hosts can receive uploads |
| `version` | Keeper and protocol version |

**Remote Windows hosts.** The host channel (5.1) stays one long-lived `ssh -T` per host but runs `myterm-cli channel`, which speaks the same sentinel-framed protocol as the `/bin/sh` channel and forwards each request to the pipe. Polling, create, rename, kill and upload all go through it. Attaching runs `ssh -t <host> myterm-cli attach <id>`. When the connection drops only the helper dies; the reconnect path (5.4) then works unchanged.

`myterm-cli attach` relays like this, because sshd runs it inside Win32-OpenSSH's own ConPTY:

- Console code pages set to UTF-8 (65001) both ways; stdout in VT processing mode, written with `WriteFile`.
- stdin in raw VT input mode (processed, line and echo input off; `ENABLE_VIRTUAL_TERMINAL_INPUT` and `ENABLE_WINDOW_INPUT` on), read with `ReadConsoleInputW`, not `ReadFile`, so resizes arrive as `WINDOW_BUFFER_SIZE_EVENT` records.
- Text comes from key-down records, plus key-up records of Alt (VK 0x12): characters that are not on the keyboard layout (✓, emoji) arrive as an Alt+numpad sequence whose character is on the Alt key-up. Surrogate pairs can span two records.
- An escape sequence that is still incomplete at the end of a read (lone ESC, `ESC [` with no final byte, `ESC O`) is held for up to 20 ms for the rest. sshd's ConPTY splits sequences across reads, and a lone ESC written to the keeper's ConPTY reaches the shell as the Escape key, so Up arrow arrived as Escape then `[A`.
- The new size is taken from the record's `dwSize`. Asking the console for its size while handling the event can still return the old size.

**Attach and redraw.** Several clients may attach to one session; input from any is accepted, and the most recently active client's size wins (as tmux). ConPTY keeps no history a client can ask for, so on attach the keeper forces a full repaint by resizing the pseudoconsole (to the attaching size, or by one row and back). This works only with the inbox ConPTY: the bundled ConPTY 1.25 that the UI uses for its own terminals never repaints on resize, whatever the flags. A later version can keep screen state and scrollback in the keeper with a VT emulator (`vt100` or `alacritty_terminal`) and replay it instead. Scrollback on Windows sessions is xterm.js's and covers only what that client has seen.

**Host kind and bootstrap.** The Windows probe (channel.rs) then checks for the keeper:

- `myterm-cli` found and keeper running → kind `keeper`; sessions are persistent and discoverable.
- `myterm-cli` found, keeper not running → the channel starts it outside the ssh job:
  1. Preferred: a per-user scheduled task `myterm-keeper` (registered by the installer or on first use; action `myterm.exe --keeper`, logon type Interactive, run level Limited, no time limit), started with `Start-ScheduledTask`. It runs in the user's desktop logon (session 1, medium integrity), so it has the same credentials as a keeper the UI started. It only runs while the user is logged on at the desktop.
  2. Otherwise, when nobody is logged on at the desktop: WMI (`Win32_Process.Create`). This keeper runs in session 0 in a network logon derived from the ssh logon, at high integrity for administrators. It survives the disconnect, but its sessions cannot see Windows Credential Manager (so Git Credential Manager, `gh` and similar find no saved logins) and probably cannot reach network shares. The host shows a note saying so.
- myterm not installed → kind `ephemeral` as in v1 (5.6).

A version mismatch between client and keeper shows on the host heading; the protocol carries a version so older keepers can still be listed.

**Attention state.** The Claude hook script on Windows writes the state file keyed by `MYTERM_SESSION` (8.5) and the keeper includes that state in `list`, so the channel no longer reads the folder itself.

**Lifetime.** Sessions end on reboot or logoff, as with tmux. Quitting the UI, updating it, or a UI crash does not touch them. Updating the keeper itself ends its sessions, so the updater asks first when any are running and otherwise leaves the old keeper running until it is idle. A keeper started by the UI or by the scheduled task runs in the user's interactive logon, so its sessions have the user's full credentials (Credential Manager, network shares, DPAPI), unlike processes started directly by sshd. A keeper started through WMI does not (see bootstrap).

**Model changes.** Host kind gains `keeper`. On keeper hosts `persistent` is true, rows have no windows (one row per session, like ephemeral rows today), and the ephemeral marker and quit warning do not apply.

**Verified** (2026-11-25, Windows 11 build 26200, Win32-OpenSSH sshd 9.5, ssh to localhost as an administrator; spike in `spikes/keeper-verify/`):

1. **A process started by WMI from an ssh session survives the disconnect: yes.** Control: the same keeper started with `Start-Process` inside the ssh session dies when the connection closes. The WMI keeper runs in session 0, network logon, high integrity, and sees none of the user's Credential Manager entries (67 from the desktop, 0 from it); DPAPI works. A scheduled task with an Interactive principal, started from ssh, runs the keeper in the desktop logon (session 1, medium, all entries visible) instead, hence the bootstrap order above.
2. **A pipe created in the desktop logon with a user-SID DACL accepts a connection from an ssh logon of the same user: yes**, and the other way round: a high-integrity keeper started over ssh accepts medium-integrity desktop clients without a mandatory label. A second keeper is refused by `FILE_FLAG_FIRST_PIPE_INSTANCE`.
3. **Resizing the pseudoconsole repaints the full screen: yes, with the inbox ConPTY and no creation flags.** A blank client fed only the bytes after the resize ends up identical cell for cell (text, colours, attributes, cursor) to one that saw everything, for pwsh, Windows PowerShell, cmd and Claude Code, both for a one-row nudge and a real size change. Startup takes about 280 ms with no stall. Two flags are wrong for a keeper, and portable-pty 0.9 always passes both: `PSEUDOCONSOLE_INHERIT_CURSOR` asks the terminal for the cursor position and waits forever when no client is attached, and `PSEUDOCONSOLE_RESIZE_QUIRK` suppresses the repaint in newer ConPTYs (the inbox one ignores it). The bundled ConPTY 1.25 never repaints on resize. Note that `LoadLibrary("conpty.dll")` searches PATH, so a sideloading app can pick up another program's copy (WezTerm's here); the keeper loads it from kernel32 explicitly.
4. **`myterm-cli attach` under `ssh -t` passes keys, resize and UTF-8: yes, with the relay rules above.** Driven through a local ConPTY standing in for xterm.js: UTF-8 both ways (é ✓ 日本 🙂), Up arrow history, Ctrl+C, Tab completion and two resizes reaching pwsh all pass in three runs out of three. Without those rules, ✓ and emoji were dropped, Up arrow arrived as Escape then `[A`, and a resize was lost.

Still unverified: the scheduled-task bootstrap for a non-administrator user; what happens to a WMI-started keeper at logoff; repaint on older inbox ConPTYs (Windows 10, Server 2019/2022), so the keeper should report the Windows build in `version`; remote network shares from a WMI keeper.

### 14.2 Saved sessions

**Goal.** Long-running jobs kept in a session (a dev server, a worker in WSL) can be started again after a reboot with one command, without turning them into system services.

- A saved session is a `[[saved]]` table in `config.toml`: `name`, `host`, `kind`, `cwd`, `commands` (lines typed in turn) and `autostart`. "Save session…" in a row's right-click menu or the palette writes it; hand edits work too.
- The dialog fills `commands` from the lines typed at the shell prompt in that terminal since it opened. Only plain typing and Backspace are followed; a line that used history, Tab completion or cursor keys is left out and counted, and the dialog says so. Nothing is recorded while a program has the alternate screen. The record stays in memory only; nothing is written until the user saves.
- Starting one creates the session as "New session" would, then types each command once the output has been quiet for 0.8 s (at least 1.5 s after the previous command, at most 30 s). Typing early is safe, as input waits in the console's buffer; so `wsl` followed by a command for the Linux shell works.
- "Start saved sessions" starts every saved session not already running; "running" means a session or tmux window of that name (case-insensitive) on that host. `autostart = true` does the same once per window when the host first becomes usable, 2 s later so the host's session list has arrived and a keeper session that outlived the window is not started twice.

### 14.3 Take over from another machine

**Goal.** At another of my computers, "Take over from…" in the palette, then picking the machine I was using, opens the same sessions in the same order with the same row and window on screen, and the other machine lets go. Nothing is copied ahead of time. The new machine asks the old one at the moment of the takeover.

**Nothing moves.** Sessions stay where they run. Sessions on tmux hosts are attached as usual. The old machine's local sessions become remote sessions under that machine's host, with execution, files, environment and logins all staying there. So the old machine must be awake and reachable over ssh for its local sessions to come across. If it is not, the takeover brings what it can and says what it left out. Ephemeral sessions cannot be taken over and are listed as left behind. Scrollback of keeper sessions is only the current screen until the keeper keeps screen state (14.1).

**The UI's own channel.** The running UI listens on a local-only channel with the keeper's protection rules: `\\.\pipe\myterm-ui-<user SID>` (owner-only DACL, `PIPE_REJECT_REMOTE_CLIENTS`) on Windows, and a `0600` Unix socket in the app's support folder on macOS. There is no network listener. ssh provides authentication and transport. `myterm-cli` gains:

| Command | Effect |
|---|---|
| `ui state` | One JSON line: machine name (the hostname), time of the last keyboard input, the open sessions in order (host, slug, window), the row on screen, last-viewed times, known folders, and for each host the machine name it reported at bootstrap. When the UI is not running, the same is read from `state.json`, with no input time |
| `ui release <taker>` | The UI closes every attach PTY and shows "Moved to *taker*" in place of the terminal, with a "Take it back" button that runs a takeover in the other direction. It does nothing when the UI is not running |

This needs the view in `state.json`: the open sessions, their order and the row on screen are saved when they change, and also restored at start-up.

**Picking the machine.** The palette's "Take over from…" lists every host in the host list where `myterm-cli` was found (keeper hosts, and macOS hosts where it is installed), other than this machine. When the list opens, `ui state` is asked of each one in parallel. Each row shows the machine, whether its UI is running, and how long ago it was last used, most recent first. Hosts that do not answer within 3 s are greyed out.

**Mapping hosts.** Host names are those of the machine that answered, and `local` means that machine. The taker therefore maps:

- `local` to the alias it used to reach the old machine;
- any host whose reported machine name is the taker's own (case-insensitive) to `local`;
- every other name as is. A host that is not in the taker's list is offered to be added, and its sessions are listed as left behind until it is.

Last-viewed times and known folders are merged under the mapped names, the newer time winning, so `done` badges the old machine had cleared stay cleared. Host bootstrap records each host's machine name (`hostname`) in `state.json` for this. `config.toml` is not copied. Saved sessions, theme and keys stay per machine, so a `host = "local"` saved session never starts on the wrong machine.

**Order.** (1) `ui state` from the chosen machine. (2) Connect any listed host that is not connected. (3) Open the sessions in order, up to the open-session cap, and select the row and window that was on screen. (4) `ui release <this machine>`. Release comes last, so a takeover that fails part-way leaves the old machine as it was.

**Not included.** Moving a running session's process to the new machine. A later "Move here" for Claude sessions alone could copy the conversation from `~/.claude/projects/` and run `claude --resume`, provided the working copy on the new machine matches.

### 14.4 Android client

**Goal.** The whole app on my Android phone: the list, attaching, new sessions, rename and kill, switcher and palette, and needs-attention badges, reaching hosts over the tailnet with ordinary SSH keys. Claude sessions are the main use, but nothing is cut down for the phone.

**Same project.** Tauri 2 builds the phone app from this repo (`npx tauri android init` adds `src-tauri/gen/android`). The frontend, tmux parsing, host tasks and attention rules are shared. What differs is behind `#[cfg(target_os = "android")]` in Rust and a phone layout in the frontend.

**Built-in SSH client.** Android has no `ssh` binary and no `~/.ssh/config`, so there the app speaks SSH itself, with `russh` and the `ring` crypto backend (which cross-compiles without cmake or NASM). Desktop builds keep the system `ssh` and use the built-in client only for hosts set to `transport = "builtin"` under `[hosts.<alias>]`, which is how it is tested.

- **One SSH connection per host.** The host channel, a keeper channel and every attach are separate SSH channels on it, so opening a session costs no new handshake. Keepalive every 15 s, dropped after 2 missed. A channel that does not open within 10 s drops the connection, so the next attempt starts fresh after a network change.
- **What maps to what.** The host channel (5.1) is an `exec` of `/bin/sh`; the Windows probe and reading `keeper.json` are `exec`s whose output is collected; the keeper channel is an `exec` of `myterm-cli channel`. Attaching (5.3) requests a PTY (`xterm-256color`, the terminal's size) and `exec`s the same tmux or `myterm-cli attach` command; resizes are SSH window-change messages. Ephemeral sessions and interactive login request a PTY and a shell.
- **Destination.** `address` (default: the alias), `user` and `port` (default 22) under `[hosts.<alias>]`. On desktop, unset fields come from `ssh -G <alias>`, so an existing `~/.ssh/config` alias works as is. A host with no user cannot connect and says so.
- **Authentication.** Ordinary SSH keys. On desktop, first the unencrypted key files `ssh -G` lists for the host (there is no passphrase prompt). Then the app's own Ed25519 key, `ssh_ed25519` in the data folder, created on first use; this is the phone's key. The Hosts dialog shows its public key with a Copy button, for `authorized_keys`. Last, `none`, which Tailscale SSH accepts on hosts that have it turned on. No passwords and no ssh agent. A host that refuses all of them shows "needs key" and the public key, not "needs interactive login".
- **Host keys.** The app keeps its own `known_hosts` in the data folder and trusts a host's key the first time. A changed key refuses the connection and says so; "Forget host key" in the Hosts dialog clears it.
- **Hosts dialog.** For a host on the built-in client, ✎ opens its address, user and port; Save writes them under `[hosts.<alias>]` (keeping the rest of the file) and reconnects. On Android the add form asks for the same three, and the dialog shows the phone's public key with a Copy button. Clicking a host that says "needs key" opens the dialog at that host.

**Phone layout.** Below 700 px wide the sidebar becomes a full-screen list and opening a session shows the terminal full screen; Back returns to the list. Dialogs fill the screen, and Back closes the keyboard, then the dialog. A row of keys above the on-screen keyboard sends Esc, Tab, Shift+Tab, Ctrl and Alt (both sticky for one key), the arrows, and `~ | / -`. The switcher, palette and new-session flow open from buttons in the header as well as from their keys.

**Backgrounding.** Android suspends the app's connections in the background. On resume the host channels and attach PTYs reconnect through the existing backoff (5.4) and tmux redraws. There are no notifications on Android (8.4); they are not wanted there.

**Settings on Android.** There is no editor to hand config.toml to, so the settings button opens it in a text editor in the app. Save refuses text that does not parse and shows the error.

**Not on Android.** The `local` host and local sessions, the local keeper, Take over from (14.3), and opening the ssh config. Hooks install on hosts as on desktop.

**Order.** (1) The built-in client on desktop, behind `transport = "builtin"`. (2) The Android build with the list and terminal working. (3) Phone layout and keys row. (4) Key setup and host editing in the Hosts dialog. All four are done.

## 15. Resolved questions

1. Sessions created by the app hide the tmux status bar (section 5.5); configurable.
2. Killing a session never asks for confirmation.
3. The cap on background connections is 8.
