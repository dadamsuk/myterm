# myterm

One list of every long-running terminal session across all your machines. See `SPEC.md` for the design.

## Develop

```
npm install
npx tauri dev        # run the app
npx tauri build      # installers in src-tauri/target/release/bundle
npm test             # frontend unit tests
cd src-tauri && cargo test                                   # backend unit tests
cd src-tauri && cargo test live -- --ignored --nocapture     # live check against ~/.ssh/config hosts
                                                             # (MYTERM_HOST=a,b limits the hosts)
```

## Layout

| Path | Contents |
|---|---|
| `src-tauri/src/channel.rs` | Host channel: one `ssh -T host /bin/sh` per host, sentinel-framed requests, Windows probe |
| `src-tauri/src/hosts.rs` | Host tasks (connect, bootstrap, 1 s polling, backoff), session ops, ephemeral sessions, hook install |
| `src-tauri/src/tmux.rs` | tmux snippets and output parsers |
| `src-tauri/src/pty.rs` | PTYs; batched binary output with ack-based backpressure |
| `src-tauri/src/hooks.rs` | Claude Code hook scripts and `settings.json` merge |
| `src-tauri/src/sshconfig.rs` | `~/.ssh/config` parsing with `Include` |
| `src-tauri/src/config.rs` | `config.toml` and `state.json` |
| `src/lib/terminals.ts` | xterm.js instances, attach/reconnect, open-session cap, keys inside the terminal |
| `src/lib/store.svelte.ts` | App state, attention rules, list ordering |
| `src/components/` | Sidebar, header, switcher, palette, new-session flow, dialogs |

Settings live in `%APPDATA%\myterm\config.toml` (Windows) or `~/Library/Application Support/myterm/config.toml` (macOS); the file is created with commented defaults on first run and reloaded on change.
