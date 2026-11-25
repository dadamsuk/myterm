# keeper-verify

Throwaway checks for the Windows keeper design (SPEC.md 14.1, "Verified"). It is
a standalone crate and not part of the app build.

```
cargo build --release
set KV_INBOX=1            # use the inbox ConPTY (kernel32), not a conpty.dll found on PATH
```

| Command | Check |
|---|---|
| `serve [--inbox] [--cmd "pwsh -NoLogo -NoProfile"] [--flags none\|inherit+quirk+win32] [--trace-input]` | Minimal keeper: `\\.\pipe\myterm-spike-<SID>` with a user-only DACL, one ConPTY session. Logs to `%LOCALAPPDATA%\myterm-spike\` |
| `ping` | Connects to the pipe and prints both ends' session, logon and integrity (check 2) |
| `attach [--trace FILE]` | The `myterm-cli attach` relay; run it under `ssh -t` (check 4) |
| `repaint --cmd CMD --keys 'dir\r' --mode nudge:0\|size:100x40 [--flags F] [--show]` | Does a resize repaint the whole screen? (check 3) |
| `harness [--target CMD \| --exe PATH --ssh-opts ...] [--script FILE]` | Runs `ssh -t localhost <exe> attach` in a local ConPTY and checks UTF-8, keys and resize (check 4) |
| `whoami` | This process's identity |

Checks 1 and 2 also need ssh to this machine. An administrator's key goes in
`C:\ProgramData\ssh\administrators_authorized_keys`.
