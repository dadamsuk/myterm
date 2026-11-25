//! The keeper's own ConPTY: always the inbox one (linked from kernel32, never
//! a conpty.dll next to the exe or on PATH) and created with no flags.
//! INHERIT_CURSOR would block with no client attached, and the bundled
//! ConPTY 1.25 never repaints on resize, which attaching relies on.

use super::winsec::wide;
use anyhow::{bail, Result};
use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::FromRawHandle;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Console::{ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess, InitializeProcThreadAttributeList,
    TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, INFINITE, LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

pub struct Con {
    hpc: Mutex<Option<usize>>,
    process: usize,
    pub pid: u32,
    pub input: File,
    pub output: File,
}

// The handles are plain kernel handles, usable from any thread.
unsafe impl Send for Con {}
unsafe impl Sync for Con {}

fn coord(cols: u16, rows: u16) -> COORD {
    COORD { X: cols.max(10) as i16, Y: rows.max(2) as i16 }
}

impl Con {
    /// Starts `cmdline` in a new pseudoconsole. `env` is a CREATE_UNICODE_ENVIRONMENT block.
    pub fn spawn(cmdline: &str, cwd: &str, env: &[u16], cols: u16, rows: u16) -> Result<Con> {
        unsafe {
            let (mut in_r, mut in_w, mut out_r, mut out_w): (HANDLE, HANDLE, HANDLE, HANDLE) =
                (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            if CreatePipe(&mut in_r, &mut in_w, std::ptr::null(), 0) == 0 {
                bail!("CreatePipe: {}", std::io::Error::last_os_error());
            }
            if CreatePipe(&mut out_r, &mut out_w, std::ptr::null(), 0) == 0 {
                CloseHandle(in_r);
                CloseHandle(in_w);
                bail!("CreatePipe: {}", std::io::Error::last_os_error());
            }
            let mut hpc: HPCON = 0 as HPCON;
            let hr = CreatePseudoConsole(coord(cols, rows), in_r, out_w, 0, &mut hpc);
            CloseHandle(in_r);
            CloseHandle(out_w);
            if hr != 0 {
                CloseHandle(in_w);
                CloseHandle(out_r);
                bail!("CreatePseudoConsole: HRESULT 0x{hr:08x}");
            }

            let mut size = 0usize;
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
            let mut attrs = vec![0u64; size / 8 + 1];
            let list = attrs.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
            InitializeProcThreadAttributeList(list, 1, 0, &mut size);
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpc as *const c_void,
                std::mem::size_of::<HPCON>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            // Null std handles, or a child of a redirected parent writes there.
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.lpAttributeList = list;
            let mut cmd = wide(cmdline);
            let cwd = wide(cwd);
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessW(
                std::ptr::null(),
                cmd.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
                env.as_ptr() as *const c_void,
                cwd.as_ptr(),
                &si.StartupInfo,
                &mut pi,
            );
            DeleteProcThreadAttributeList(list);
            if ok == 0 {
                let e = std::io::Error::last_os_error();
                ClosePseudoConsole(hpc);
                CloseHandle(in_w);
                CloseHandle(out_r);
                bail!("can't start {cmdline}: {e}");
            }
            CloseHandle(pi.hThread);
            Ok(Con {
                hpc: Mutex::new(Some(hpc as usize)),
                process: pi.hProcess as usize,
                pid: pi.dwProcessId,
                input: File::from_raw_handle(in_w as _),
                output: File::from_raw_handle(out_r as _),
            })
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        if let Some(h) = *self.hpc.lock().unwrap() {
            let hr = unsafe { ResizePseudoConsole(h as HPCON, coord(cols, rows)) };
            if hr != 0 {
                bail!("ResizePseudoConsole: HRESULT 0x{hr:08x}");
            }
        }
        Ok(())
    }

    /// Blocks until the child exits; returns its exit code.
    pub fn wait(&self) -> u32 {
        unsafe {
            WaitForSingleObject(self.process as HANDLE, INFINITE);
            let mut code = 0u32;
            GetExitCodeProcess(self.process as HANDLE, &mut code);
            code
        }
    }

    pub fn kill(&self) {
        unsafe { TerminateProcess(self.process as HANDLE, 1) };
    }

    /// Closes the pseudoconsole, which ends the output stream.
    pub fn close(&self) {
        if let Some(h) = self.hpc.lock().unwrap().take() {
            unsafe { ClosePseudoConsole(h as HPCON) };
        }
    }
}

impl Drop for Con {
    fn drop(&mut self) {
        self.close();
        unsafe { CloseHandle(self.process as HANDLE) };
    }
}
