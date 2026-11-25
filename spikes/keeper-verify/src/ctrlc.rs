//! Does Ctrl+C typed into a ConPTY stop a program running in it, when the
//! process that owns the ConPTY was started the way the keeper is
//! (DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)?
//!
//!   ctrlc [--detached] [--reenable]
//!
//! --detached   re-runs this check in a child started with those flags and prints its result
//! --reenable   calls SetConsoleCtrlHandler(NULL, FALSE) before starting the ConPTY child

use crate::has;
use crate::repaint::Pty;
use anyhow::{bail, Result};
use std::os::windows::process::CommandExt;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

pub fn run(rest: &[String]) -> Result<()> {
    if has(rest, "--detached") {
        let out = std::env::temp_dir().join(format!("kv-ctrlc-{}.txt", std::process::id()));
        let mut args: Vec<String> = vec!["ctrlc".into()];
        args.extend(rest.iter().filter(|a| *a != "--detached").cloned());
        args.push("--out".into());
        args.push(out.display().to_string());
        let st = std::process::Command::new(std::env::current_exe()?)
            .args(&args)
            .creation_flags(0x0000_0008 | 0x0000_0200) // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
            .status()?;
        let text = std::fs::read_to_string(&out).unwrap_or_default();
        let _ = std::fs::remove_file(&out);
        print!("{text}");
        if !st.success() {
            bail!("child exited {:?}", st.code());
        }
        return Ok(());
    }
    if has(rest, "--reenable") {
        unsafe { SetConsoleCtrlHandler(None, 0) };
    }
    let mut p = Pty::spawn("ping -n 30 127.0.0.1", 100, 30, 0)?;
    p.settle(Duration::from_millis(1500), Duration::from_secs(5));
    p.send(b"\x03")?;
    let t = Instant::now();
    let stopped = unsafe { WaitForSingleObject(p.con.process as HANDLE, 5000) } == 0;
    let line = if stopped {
        format!("ctrl+c stopped ping after {} ms\n", t.elapsed().as_millis())
    } else {
        p.con.kill();
        "ctrl+c IGNORED: ping still running after 5 s\n".to_string()
    };
    match crate::opt(rest, "--out") {
        Some(f) => std::fs::write(f, line)?,
        None => print!("{line}"),
    }
    Ok(())
}
