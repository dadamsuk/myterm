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
                                                             # (MYTERM_HOST=a,b limits the hosts;
                                                             #  MYTERM_BUILTIN=1 uses the built-in SSH client)
```

Android (spec 14.4). Needs JDK 17, the Android SDK and NDK, `rustup target add aarch64-linux-android`, and on Windows Developer Mode (Tauri links the library with a symlink):

```
export ANDROID_HOME="$LOCALAPPDATA/Android/Sdk" NDK_HOME="$ANDROID_HOME/ndk/<version>"
npx tauri android build --target aarch64 --apk
adb install -r src-tauri/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk
```

The release build is signed with the debug key unless `src-tauri/gen/android/keystore.properties` (`storeFile`, `storePassword`, `keyAlias`, `keyPassword`; not committed) names another. Either build then installs over the other and keeps the app's data; a different key would mean uninstalling, which deletes the phone's SSH key and config.

On the phone, config and keys live in the app's private folder. A debug build (`--debug`, path `debug/app-universal-debug.apk`) can read them with `adb shell run-as net.myterm.app ls`; a release build cannot.

## Layout

| Path | Contents |
|---|---|
| `src-tauri/src/channel.rs` | Host channel: one `ssh -T host /bin/sh` per host, sentinel-framed requests, Windows probe |
| `src-tauri/src/hosts.rs` | Host tasks (connect, bootstrap, 1 s polling, backoff), session ops, ephemeral sessions, hook install |
| `src-tauri/src/tmux.rs` | tmux snippets and output parsers |
| `src-tauri/src/pty.rs` | PTYs; batched binary output with ack-based backpressure |
| `src-tauri/src/hooks.rs` | Claude Code hook scripts and `settings.json` merge |
| `src-tauri/src/sshconfig.rs` | `~/.ssh/config` alias parsing with `Include`, for host suggestions |
| `src-tauri/src/config.rs` | `config.toml` and `state.json` |
| `src/lib/terminals.ts` | xterm.js instances, attach/reconnect, open-session cap, keys inside the terminal |
| `src/lib/store.svelte.ts` | App state, attention rules, list ordering |
| `src/components/` | Sidebar, header, switcher, palette, new-session flow, dialogs |

Settings live in `%APPDATA%\myterm\config.toml` (Windows) or `~/Library/Application Support/myterm/config.toml` (macOS); the file is created with commented defaults on first run and reloaded on change.
