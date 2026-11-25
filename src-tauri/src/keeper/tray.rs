//! Notification-area icon for the keeper (SPEC.md 14.1): shows that it is
//! running and how many sessions it holds. Its menu starts (or brings forward)
//! the myterm UI, and quits the keeper, which ends its sessions.
//!
//! Only shown when the keeper runs in the desktop logon; a keeper started
//! through WMI (session 0) has no desktop to show it on.

use super::winsec::wide;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::Shell::{
    ExtractIconExW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW, EnumWindows,
    GetCursorPos, GetMessageW, GetWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible, LoadIconW, MessageBoxW,
    PostMessageW, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetTimer, ShowWindow, TrackPopupMenu,
    TranslateMessage, GW_OWNER, HICON, IDI_APPLICATION, IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL,
    MB_SETFOREGROUND, MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SW_RESTORE, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON,
    WM_COMMAND, WM_CONTEXTMENU, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
};

/// What the tray needs from the keeper.
pub struct Hooks {
    /// Number of running sessions.
    pub count: Box<dyn Fn() -> usize + Send + Sync>,
    /// Ends every session and exits the process.
    pub quit: Box<dyn Fn() + Send + Sync>,
    /// Path of myterm.exe (the UI), if known.
    pub app: Box<dyn Fn() -> Option<PathBuf> + Send + Sync>,
    pub log: fn(&str),
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();
/// Sent to top-level windows when Explorer (and the taskbar) restarts.
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);
static ICON: AtomicUsize = AtomicUsize::new(0);
/// Session count shown in the tooltip, so it is only updated on change.
static SHOWN: AtomicUsize = AtomicUsize::new(usize::MAX);
/// When myterm was last started from here: its window takes a moment to
/// appear, and a second click meanwhile would start another copy.
static LAUNCHED: Mutex<Option<Instant>> = Mutex::new(None);

const WM_TRAY: u32 = 0x8000 + 1; // WM_APP + 1
const ID_START: usize = 1;
const ID_QUIT: usize = 2;
const TITLE: &str = "myterm keeper";

/// Shows the icon on its own thread, which runs the window message loop.
pub fn spawn(hooks: Hooks) {
    if HOOKS.set(hooks).is_err() {
        return;
    }
    std::thread::spawn(|| unsafe { run() });
}

fn hooks() -> &'static Hooks {
    HOOKS.get().expect("tray hooks")
}

unsafe fn run() {
    let hinst = GetModuleHandleW(null());
    let class = wide("myterm-keeper-tray");
    let mut wc: WNDCLASSW = std::mem::zeroed();
    wc.lpfnWndProc = Some(wndproc);
    wc.hInstance = hinst;
    wc.lpszClassName = class.as_ptr();
    RegisterClassW(&wc);
    TASKBAR_CREATED.store(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()), Ordering::SeqCst);
    // A hidden top-level window, not a message-only one: those miss the
    // TaskbarCreated broadcast and the icon would vanish after an Explorer restart.
    let hwnd = CreateWindowExW(0, class.as_ptr(), wide(TITLE).as_ptr(), 0, 0, 0, 0, 0, null_mut(), null_mut(), hinst, null());
    if hwnd.is_null() {
        (hooks().log)("tray: could not create its window");
        return;
    }
    ICON.store(load_icon() as usize, Ordering::SeqCst);
    if !add_icon(hwnd) {
        // Explorer may not be up yet (keeper started at logon): TaskbarCreated adds it later.
        (hooks().log)("tray: icon not added yet");
    }
    SetTimer(hwnd, 1, 2000, None);
    let mut msg: MSG = std::mem::zeroed();
    while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
        TranslateMessage(&msg);
        DispatchMessageW(&msg);
    }
}

/// The exe's own icon (the keeper is a copy of myterm.exe).
unsafe fn load_icon() -> HICON {
    if let Ok(exe) = std::env::current_exe() {
        let path = wide(&exe.to_string_lossy());
        let mut small: HICON = null_mut();
        if ExtractIconExW(path.as_ptr(), 0, null_mut(), &mut small, 1) > 0 && !small.is_null() {
            return small;
        }
    }
    LoadIconW(null_mut(), IDI_APPLICATION)
}

unsafe fn data(hwnd: HWND) -> NOTIFYICONDATAW {
    let mut d: NOTIFYICONDATAW = std::mem::zeroed();
    d.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    d.hWnd = hwnd;
    d.uID = 1;
    d
}

fn tooltip(n: usize) -> String {
    match n {
        0 => format!("{TITLE}: no sessions"),
        1 => format!("{TITLE}: 1 session"),
        n => format!("{TITLE}: {n} sessions"),
    }
}

fn set_tip(d: &mut NOTIFYICONDATAW, text: &str) {
    let w: Vec<u16> = text.encode_utf16().take(d.szTip.len() - 1).collect();
    d.szTip[..w.len()].copy_from_slice(&w);
    d.szTip[w.len()] = 0;
}

unsafe fn add_icon(hwnd: HWND) -> bool {
    let n = (hooks().count)();
    let mut d = data(hwnd);
    d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    d.uCallbackMessage = WM_TRAY;
    d.hIcon = ICON.load(Ordering::SeqCst) as HICON;
    set_tip(&mut d, &tooltip(n));
    SHOWN.store(n, Ordering::SeqCst);
    Shell_NotifyIconW(NIM_ADD, &d) != 0
}

unsafe fn update_tip(hwnd: HWND) {
    let n = (hooks().count)();
    if SHOWN.swap(n, Ordering::SeqCst) == n {
        return;
    }
    let mut d = data(hwnd);
    d.uFlags = NIF_TIP;
    set_tip(&mut d, &tooltip(n));
    Shell_NotifyIconW(NIM_MODIFY, &d);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TRAY => {
            // Without NOTIFYICON_VERSION_4, lParam is the mouse message.
            match lp as u32 & 0xffff {
                WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                WM_LBUTTONUP => start_app(hwnd),
                _ => {}
            }
            0
        }
        WM_COMMAND => {
            match wp & 0xffff {
                ID_START => start_app(hwnd),
                ID_QUIT => quit(hwnd),
                _ => {}
            }
            0
        }
        WM_TIMER => {
            update_tip(hwnd);
            0
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::SeqCst) => {
            add_icon(hwnd);
            0
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe fn show_menu(hwnd: HWND) {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    let start_flags = if (hooks().app)().is_some() { MF_STRING } else { MF_STRING | MF_GRAYED };
    AppendMenuW(menu, start_flags, ID_START, wide("Start myterm").as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    AppendMenuW(menu, MF_STRING, ID_QUIT, wide("Quit").as_ptr());
    let mut pt = POINT { x: 0, y: 0 };
    GetCursorPos(&mut pt);
    // Without these two steps the menu does not close when clicking elsewhere.
    SetForegroundWindow(hwnd);
    TrackPopupMenu(menu, TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, null());
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);
}

/// Brings an open myterm window forward, or starts myterm.
unsafe fn start_app(_hwnd: HWND) {
    let Some(app) = (hooks().app)() else { return };
    if let Some(w) = find_window(&app) {
        if IsIconic(w) != 0 {
            ShowWindow(w, SW_RESTORE);
        }
        SetForegroundWindow(w);
        return;
    }
    let mut last = LAUNCHED.lock().unwrap();
    if last.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
        return;
    }
    *last = Some(Instant::now());
    if let Err(e) = std::process::Command::new(&app).spawn() {
        (hooks().log)(&format!("tray: could not start {}: {e}", app.display()));
    }
}

struct Search {
    exe: Vec<u16>,
    found: HWND,
}

/// A visible, unowned top-level window of a process running `app`.
unsafe fn find_window(app: &std::path::Path) -> Option<HWND> {
    let canon = |p: &std::path::Path| p.to_string_lossy().to_lowercase();
    let mut s = Search { exe: canon(app).encode_utf16().collect(), found: null_mut() };
    EnumWindows(Some(enum_proc), &mut s as *mut Search as LPARAM);
    if s.found.is_null() { None } else { Some(s.found) }
}

unsafe extern "system" fn enum_proc(w: HWND, lp: LPARAM) -> windows_sys::core::BOOL {
    let s = &mut *(lp as *mut Search);
    if IsWindowVisible(w) == 0 || !GetWindow(w, GW_OWNER).is_null() {
        return 1;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(w, &mut pid);
    let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if h.is_null() {
        return 1;
    }
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len) != 0;
    CloseHandle(h);
    if ok && String::from_utf16_lossy(&buf[..len as usize]).to_lowercase().encode_utf16().eq(s.exe.iter().copied()) {
        s.found = w;
        return 0;
    }
    1
}

unsafe fn quit(hwnd: HWND) {
    let n = (hooks().count)();
    if n > 0 {
        let text = if n == 1 {
            "Quitting ends the running session and the programs in it.".to_string()
        } else {
            format!("Quitting ends the {n} running sessions and the programs in them.")
        };
        let r = MessageBoxW(
            hwnd,
            wide(&text).as_ptr(),
            wide(TITLE).as_ptr(),
            MB_OKCANCEL | MB_ICONWARNING | MB_DEFBUTTON2 | MB_SETFOREGROUND,
        );
        if r != IDOK {
            return;
        }
    }
    let d = data(hwnd);
    Shell_NotifyIconW(NIM_DELETE, &d);
    (hooks().quit)();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tooltips() {
        assert_eq!(tooltip(0), "myterm keeper: no sessions");
        assert_eq!(tooltip(1), "myterm keeper: 1 session");
        assert_eq!(tooltip(3), "myterm keeper: 3 sessions");
    }

    #[test]
    fn long_tip_is_truncated() {
        let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        set_tip(&mut d, &"x".repeat(500));
        assert_eq!(d.szTip[126], 'x' as u16);
        assert_eq!(d.szTip[127], 0);
    }
}
