//! Clipboard images and dropped files: saved locally, or uploaded to a remote
//! host through its channel, so a program there (e.g. Claude Code) can open
//! them by path.

use crate::hosts::App;
use anyhow::{anyhow, bail, Result};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri_plugin_clipboard_manager::ClipboardExt;

/// Largest file sent through a channel; base64 over ssh is slow beyond this.
const MAX_UPLOAD: usize = 50 * 1024 * 1024;
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(180);
/// Uploaded and pasted files older than this are removed.
const KEEP_SECS: u64 = 24 * 3600;

fn millis() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

/// The clipboard image as PNG, or None when the clipboard holds no image.
pub fn clipboard_png(app: &App) -> Result<Option<Vec<u8>>> {
    let img = match app.handle.clipboard().read_image() {
        Ok(i) => i,
        Err(_) => return Ok(None),
    };
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Ok(None);
    }
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header()?;
        wr.write_image_data(img.rgba())?;
    }
    Ok(Some(out))
}

/// Keeps a file name to characters that need no quoting in any shell.
pub fn safe_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
        .collect();
    let s = s.trim_start_matches('.').to_string();
    if s.is_empty() { "file".into() } else { s }
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len() / 3 * 4 + data.len() / 57 + 8);
    for (i, c) in data.chunks(3).enumerate() {
        if i > 0 && i % 19 == 0 {
            out.push('\n'); // 76-column lines
        }
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn local_dir() -> PathBuf {
    std::env::temp_dir().join("myterm")
}

fn clean_local(dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|d| d.as_secs() > KEEP_SECS);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Shell script that writes base64 `body` to a private temp file named `name`
/// and prints its path.
pub fn upload_script(name: &str, body: &str) -> String {
    let delim = format!("MYTERM_B64_{}", millis());
    format!(
        r#"d="${{TMPDIR:-/tmp}}"; d="${{d%/}}/myterm-$(id -u)"
mkdir -p "$d" && chmod 700 "$d" || exit 1
find "$d" -type f -mmin +{mins} -exec rm -f {{}} + 2>/dev/null
if printf 'YQ==' | base64 -d >/dev/null 2>&1; then B='base64 -d'; else B='base64 -D'; fi
f="$d/{name}"
$B > "$f" <<'{delim}' || exit 1
{body}
{delim}
[ -s "$f" ] || {{ echo "upload failed"; exit 1; }}
printf '%s\n' "$f""#,
        mins = KEEP_SECS / 60,
    )
}

impl App {
    /// Puts `data` where programs on `host` can read it; returns the path there.
    pub async fn place_file(&self, host: &str, name: &str, data: &[u8]) -> Result<String> {
        let h = self.host(host)?;
        let name = format!("{}-{}", millis(), safe_name(name));
        if h.is_local {
            let dir = local_dir();
            std::fs::create_dir_all(&dir)?;
            clean_local(&dir);
            let p = dir.join(&name);
            std::fs::write(&p, data)?;
            return Ok(p.to_string_lossy().into_owned());
        }
        if data.len() > MAX_UPLOAD {
            bail!("{name} is larger than {} MB", MAX_UPLOAD / 1024 / 1024);
        }
        let chan = h
            .channel
            .lock()
            .unwrap()
            .clone()
            .filter(|c| !c.is_dead())
            .ok_or_else(|| anyhow!("Can't upload files to {host}: it has no shell channel"))?;
        let r = chan.run(&upload_script(&name, &base64(data)), UPLOAD_TIMEOUT).await?;
        let path = r.out.lines().last().unwrap_or("").trim().to_string();
        if r.code != 0 || !path.starts_with('/') {
            bail!("Upload to {host} failed: {}", r.out.trim());
        }
        Ok(path)
    }

    pub async fn paste_image(&self, host: &str) -> Result<Option<String>> {
        let Some(png) = clipboard_png(self)? else { return Ok(None) };
        Ok(Some(self.place_file(host, "paste.png", &png).await?))
    }

    /// Paths on `host` for dropped local files; local hosts use them as they are.
    pub async fn drop_files(&self, host: &str, paths: &[String]) -> Result<Vec<String>> {
        if self.host(host)?.is_local {
            return Ok(paths.to_vec());
        }
        let mut out = vec![];
        for p in paths {
            let pb = PathBuf::from(p);
            if pb.is_dir() {
                bail!("Folders can't be uploaded: {p}");
            }
            let data = std::fs::read(&pb)?;
            let name = pb.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push(self.place_file(host, &name, &data).await?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_base64() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        let long = base64(&[0u8; 120]);
        assert!(long.lines().all(|l| l.len() <= 76));
    }

    #[test]
    fn safe_names() {
        assert_eq!(safe_name("my shot (1).png"), "my_shot__1_.png");
        assert_eq!(safe_name("../x"), "_x");
        assert_eq!(safe_name(""), "file");
    }

    /// Runs the upload script in a real sh and checks the bytes round-trip.
    #[cfg(unix)]
    #[test]
    fn upload_roundtrip() {
        let data: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(upload_script("t.bin", &base64(&data)))
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        let _ = std::fs::remove_file(path);
    }
}
