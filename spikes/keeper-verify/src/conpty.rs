//! Direct ConPTY, so the creation flags can be chosen. portable-pty 0.9 always
//! passes INHERIT_CURSOR | RESIZE_QUIRK | WIN32_INPUT_MODE.

use anyhow::{bail, Result};
use std::ffi::c_void;
use std::fs::File;
use std::os::windows::io::FromRawHandle;
use std::sync::OnceLock;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, GetExitCodeProcess, InitializeProcThreadAttributeList, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject, EXTENDED_STARTUPINFO_PRESENT, INFINITE,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

pub const INHERIT_CURSOR: u32 = 0x1;
pub const RESIZE_QUIRK: u32 = 0x2;
pub const WIN32_INPUT_MODE: u32 = 0x4;

#[repr(C)]
#[derive(Clone, Copy)]
struct Coord {
    x: i16,
    y: i16,
}

type Hpc = *mut c_void;
type CreateFn = unsafe extern "system" fn(Coord, HANDLE, HANDLE, u32, *mut Hpc) -> i32;
type ResizeFn = unsafe extern "system" fn(Hpc, Coord) -> i32;

struct Funcs {
    create: CreateFn,
    resize: ResizeFn,
    which: String,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn funcs() -> &'static Funcs {
    static F: OnceLock<Funcs> = OnceLock::new();
    F.get_or_init(|| unsafe {
        // Same preference as portable-pty: a conpty.dll next to the exe, else the inbox one.
        let side = if std::env::var("KV_INBOX").is_ok() { std::ptr::null_mut() } else { LoadLibraryW(wide("conpty.dll").as_ptr()) };
        let (m, which) = if !side.is_null() && GetProcAddress(side, c"CreatePseudoConsole".as_ptr() as _).is_some() {
            (side, "conpty.dll")
        } else {
            (GetModuleHandleW(wide("kernel32.dll").as_ptr()), "kernel32")
        };
        let mut path = [0u16; 512];
        let n = GetModuleFileNameW(m, path.as_mut_ptr(), 512) as usize;
        let which = format!("{which} @ {}", String::from_utf16_lossy(&path[..n]));
        Funcs {
            create: std::mem::transmute(GetProcAddress(m, c"CreatePseudoConsole".as_ptr() as _).unwrap()),
            resize: std::mem::transmute(GetProcAddress(m, c"ResizePseudoConsole".as_ptr() as _).unwrap()),
            which,
        }
    })
}

pub fn which() -> &'static str {
    &funcs().which
}

pub struct Con {
    hpc: usize,
    process: usize,
    pub pid: u32,
    pub input: File,
    pub output: File,
}

unsafe impl Send for Con {}
unsafe impl Sync for Con {}

impl Con {
    pub fn spawn(cmdline: &str, cols: u16, rows: u16, flags: u32) -> Result<Con> {
        let f = funcs();
        unsafe {
            let (mut in_r, mut in_w, mut out_r, mut out_w): (HANDLE, HANDLE, HANDLE, HANDLE) =
                (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
            if CreatePipe(&mut in_r, &mut in_w, std::ptr::null(), 0) == 0
                || CreatePipe(&mut out_r, &mut out_w, std::ptr::null(), 0) == 0
            {
                bail!("CreatePipe: {}", std::io::Error::last_os_error());
            }
            let mut hpc: Hpc = std::ptr::null_mut();
            let hr = (f.create)(Coord { x: cols as i16, y: rows as i16 }, in_r, out_w, flags, &mut hpc);
            CloseHandle(in_r);
            CloseHandle(out_w);
            if hr != 0 {
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
                std::mem::size_of::<Hpc>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            );
            let mut si: STARTUPINFOEXW = std::mem::zeroed();
            si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            // Null std handles: otherwise a child of a redirected parent writes there, not to the console.
            si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            si.lpAttributeList = list;
            let mut cmd = wide(cmdline);
            let cwd = std::env::var("KV_CWD").or_else(|_| std::env::var("USERPROFILE")).map(|h| wide(&h)).ok();
            let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
            let ok = CreateProcessW(
                std::ptr::null(),
                cmd.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT,
                std::ptr::null(),
                cwd.as_ref().map(|c| c.as_ptr()).unwrap_or(std::ptr::null()),
                &si.StartupInfo,
                &mut pi,
            );
            if ok == 0 {
                bail!("CreateProcessW {cmdline}: {}", std::io::Error::last_os_error());
            }
            CloseHandle(pi.hThread);
            Ok(Con {
                hpc: hpc as usize,
                process: pi.hProcess as usize,
                pid: pi.dwProcessId,
                input: File::from_raw_handle(in_w as _),
                output: File::from_raw_handle(out_r as _),
            })
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        let hr = unsafe { (funcs().resize)(self.hpc as Hpc, Coord { x: cols as i16, y: rows as i16 }) };
        if hr != 0 {
            bail!("ResizePseudoConsole: HRESULT 0x{hr:08x}");
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
}

/// Parses "--flags": a number, or names joined by '+' (inherit, quirk, win32), or "none".
pub fn parse_flags(s: Option<String>, default: u32) -> u32 {
    let Some(s) = s else { return default };
    if let Ok(n) = s.parse() {
        return n;
    }
    s.split('+')
        .map(|p| match p {
            "inherit" => INHERIT_CURSOR,
            "quirk" => RESIZE_QUIRK,
            "win32" => WIN32_INPUT_MODE,
            _ => 0,
        })
        .fold(0, |a, b| a | b)
}
