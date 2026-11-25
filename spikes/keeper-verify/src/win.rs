//! Win32 helpers: who this process is (user SID, logon session, integrity)
//! and security attributes from SDDL.

use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenElevationType,
    TokenIntegrityLevel, TokenStatistics, TokenUser, SECURITY_ATTRIBUTES, TOKEN_INFORMATION_CLASS,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_STATISTICS, TOKEN_USER,
};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};

fn token_info(class: TOKEN_INFORMATION_CLASS) -> Vec<u64> {
    unsafe {
        let mut tok: HANDLE = std::ptr::null_mut();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok);
        let mut len = 0u32;
        GetTokenInformation(tok, class, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u64; (len as usize + 7) / 8 + 1];
        GetTokenInformation(tok, class, buf.as_mut_ptr() as *mut c_void, len, &mut len);
        CloseHandle(tok);
        buf
    }
}

fn wide_to_string(p: *const u16) -> String {
    unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

pub fn user_sid() -> String {
    let buf = token_info(TokenUser);
    unsafe {
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s: *mut u16 = std::ptr::null_mut();
        ConvertSidToStringSidW(tu.User.Sid, &mut s);
        wide_to_string(s)
    }
}

fn integrity() -> String {
    let buf = token_info(TokenIntegrityLevel);
    let rid = unsafe {
        let tml = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let n = *GetSidSubAuthorityCount(tml.Label.Sid);
        *GetSidSubAuthority(tml.Label.Sid, n as u32 - 1)
    };
    match rid {
        0x1000 => "low".into(),
        0x2000 => "medium".into(),
        0x2100 => "medium-plus".into(),
        0x3000 => "high".into(),
        0x4000 => "system".into(),
        r => format!("0x{r:x}"),
    }
}

fn elevation() -> &'static str {
    let buf = token_info(TokenElevationType);
    match buf[0] as u32 {
        1 => "default",
        2 => "full",
        3 => "limited",
        _ => "?",
    }
}

fn logon_id() -> String {
    let buf = token_info(TokenStatistics);
    let ts = unsafe { &*(buf.as_ptr() as *const TOKEN_STATISTICS) };
    format!("{:x}:{:08x}", ts.AuthenticationId.HighPart, ts.AuthenticationId.LowPart)
}

pub fn session_id() -> u32 {
    let mut s = 0u32;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut s) };
    s
}

pub fn identity() -> Value {
    json!({
        "pid": std::process::id(),
        "user": std::env::var("USERNAME").unwrap_or_default(),
        "sid": user_sid(),
        "session_id": session_id(),
        "logon_id": logon_id(),
        "integrity": integrity(),
        "elevation": elevation(),
        "under_ssh": std::env::var("SSH_CONNECTION").is_ok(),
    })
}

/// Security attributes from an SDDL string. Leaked: lives for the process.
pub fn security_attributes(sddl: &str) -> Result<*mut SECURITY_ATTRIBUTES> {
    let w: Vec<u16> = sddl.encode_utf16().chain([0]).collect();
    let mut sd: *mut c_void = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(w.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut())
    };
    if ok == 0 {
        bail!("bad SDDL {sddl}: {}", std::io::Error::last_os_error());
    }
    let sa = Box::new(SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    });
    Ok(Box::into_raw(sa))
}
