//! Windows keeper (SPEC.md 14.1): a headless `myterm.exe --keeper` per user per
//! machine that owns ConPTY sessions, so they survive the UI and ssh connections.
//!
//! Wire protocol on `\\.\pipe\myterm-keeper-<SID>`: one JSON request per line,
//! one JSON reply per line (`{"ok":true,...}` or `{"ok":false,"error":..}`).
//! `attach` switches the connection to a stream: raw output from the keeper,
//! and frames from the client (`[kind u8][len u32 LE][payload]`).

pub mod client;

#[cfg(windows)]
pub mod cli;
#[cfg(windows)]
mod conpty;
#[cfg(windows)]
pub mod server;
#[cfg(windows)]
mod winsec;

use serde::{Deserialize, Serialize};

/// Bumped on incompatible protocol changes; `hello` reports it.
pub const PROTO: u32 = 1;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const FRAME_INPUT: u8 = 0;
pub const FRAME_RESIZE: u8 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct KSession {
    pub id: String,
    pub name: String,
    /// "claude" or "shell".
    pub kind: String,
    pub cwd: String,
    pub created: i64,
    pub last_attached: i64,
    pub attached: u32,
    pub state: Option<String>,
    pub state_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Hello {
    pub version: String,
    pub proto: u32,
    pub pid: u32,
    /// Windows session: 0 when started through WMI over ssh, else the desktop's.
    pub session_id: u32,
    /// True when the keeper runs in an interactive (desktop) logon, so its
    /// sessions see Credential Manager and network credentials.
    pub interactive: bool,
    pub integrity: String,
    pub windows_build: String,
    pub home: String,
    pub user: String,
}

pub fn frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = Vec::with_capacity(5 + payload.len());
    f.push(kind);
    f.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    f.extend_from_slice(payload);
    f
}

pub fn resize_frame(cols: u16, rows: u16) -> Vec<u8> {
    let mut p = cols.to_le_bytes().to_vec();
    p.extend_from_slice(&rows.to_le_bytes());
    frame(FRAME_RESIZE, &p)
}

/// Decodes standard base64, ignoring whitespace.
pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let bytes: Vec<u8> = s.bytes().filter(|c| !c.is_ascii_whitespace()).collect();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for c in bytes.chunks(4) {
        let pad = c.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for &b in &c[..4 - pad] {
            n = n << 6 | val(b)?;
        }
        n <<= 6 * pad as u32;
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        for len in 0..40 {
            let data: Vec<u8> = (0..len as u8).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(base64_decode(&crate::files::base64(&data)).unwrap(), data);
        }
        assert!(base64_decode("abc").is_none());
    }

    #[test]
    fn frames() {
        assert_eq!(frame(FRAME_INPUT, b"hi"), vec![0, 2, 0, 0, 0, b'h', b'i']);
        assert_eq!(resize_frame(80, 24), vec![1, 4, 0, 0, 0, 80, 0, 24, 0]);
    }
}
