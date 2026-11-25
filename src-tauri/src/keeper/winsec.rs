//! Win32 helpers for the keeper: who it runs as, pipe security, and the
//! user's default environment for new sessions.

use anyhow::{bail, Result};
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
    SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    CheckTokenMembership, GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel,
    TokenUser, SECURITY_ATTRIBUTES, TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_INFORMATION_CLASS,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcessToken};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe {
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }
}

fn token(access: u32) -> HANDLE {
    let mut t: HANDLE = std::ptr::null_mut();
    unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut t) };
    t
}

fn token_info(class: TOKEN_INFORMATION_CLASS) -> Vec<u64> {
    unsafe {
        let tok = token(TOKEN_QUERY);
        let mut len = 0u32;
        GetTokenInformation(tok, class, std::ptr::null_mut(), 0, &mut len);
        let mut buf = vec![0u64; (len as usize).div_ceil(8) + 1];
        GetTokenInformation(tok, class, buf.as_mut_ptr() as *mut c_void, len, &mut len);
        CloseHandle(tok);
        buf
    }
}

pub fn user_sid() -> String {
    let buf = token_info(TokenUser);
    unsafe {
        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s: *mut u16 = std::ptr::null_mut();
        ConvertSidToStringSidW(tu.User.Sid, &mut s);
        from_wide(s)
    }
}

pub fn integrity() -> String {
    let buf = token_info(TokenIntegrityLevel);
    let rid = unsafe {
        let tml = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let n = *GetSidSubAuthorityCount(tml.Label.Sid);
        *GetSidSubAuthority(tml.Label.Sid, n as u32 - 1)
    };
    match rid {
        0..=0x1fff => "low",
        0x2000..=0x2fff => "medium",
        0x3000..=0x3fff => "high",
        _ => "system",
    }
    .into()
}

pub fn session_id() -> u32 {
    let mut s = 0u32;
    unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut s) };
    s
}

/// Whether the token is in NT AUTHORITY\INTERACTIVE (a desktop logon).
pub fn interactive() -> bool {
    unsafe {
        let mut sid: *mut c_void = std::ptr::null_mut();
        if ConvertStringSidToSidW(wide("S-1-5-4").as_ptr(), &mut sid) == 0 {
            return false;
        }
        let mut member = 0;
        CheckTokenMembership(std::ptr::null_mut(), sid, &mut member);
        member != 0
    }
}

pub fn windows_build() -> String {
    let key = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let name = wide("CurrentBuildNumber");
    let mut buf = [0u16; 64];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut c_void, &mut size)
    };
    if r != 0 {
        return String::new();
    }
    from_wide(buf.as_ptr())
}

/// Security attributes from SDDL. Leaked: they live for the process.
pub fn security_attributes(sddl: &str) -> Result<*mut SECURITY_ATTRIBUTES> {
    let w = wide(sddl);
    let mut sd: *mut c_void = std::ptr::null_mut();
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(w.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut())
    };
    if ok == 0 {
        bail!("bad SDDL {sddl}: {}", std::io::Error::last_os_error());
    }
    Ok(Box::into_raw(Box::new(SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    })))
}

/// The user's default environment, freshly built from the registry, so a
/// session does not inherit whatever environment started the keeper.
pub fn user_environment() -> Vec<(String, String)> {
    let mut vars = vec![];
    unsafe {
        let tok = token(TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_IMPERSONATE);
        let mut block: *mut c_void = std::ptr::null_mut();
        let ok = CreateEnvironmentBlock(&mut block, tok, 0);
        CloseHandle(tok);
        if ok == 0 || block.is_null() {
            return std::env::vars().collect();
        }
        let mut p = block as *const u16;
        loop {
            let s = from_wide(p);
            if s.is_empty() {
                break;
            }
            p = p.add(s.encode_utf16().count() + 1);
            // Entries like "=C:=C:\" are per-drive cwd markers; keep them out.
            if let Some((k, v)) = s.split_once('=').filter(|(k, _)| !k.is_empty()) {
                vars.push((k.to_string(), v.to_string()));
            }
        }
        DestroyEnvironmentBlock(block);
    }
    vars
}

/// A CREATE_UNICODE_ENVIRONMENT block, sorted case-insensitively as Windows expects.
pub fn environment_block(mut vars: Vec<(String, String)>) -> Vec<u16> {
    vars.sort_by_key(|(k, _)| k.to_uppercase());
    let mut b = vec![];
    for (k, v) in vars {
        b.extend(format!("{k}={v}").encode_utf16());
        b.push(0);
    }
    b.push(0);
    b
}

/// Sets `key` in `vars`, replacing any case-insensitive match.
pub fn set_var(vars: &mut Vec<(String, String)>, key: &str, value: &str) {
    vars.retain(|(k, _)| !k.eq_ignore_ascii_case(key));
    vars.push((key.to_string(), value.to_string()));
}

pub fn get_var<'a>(vars: &'a [(String, String)], key: &str) -> Option<&'a str> {
    vars.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
}
