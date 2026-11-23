//! Host discovery from `~/.ssh/config` (section 9).

use std::path::{Path, PathBuf};

pub fn ssh_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".ssh")
}

pub struct Parsed {
    pub hosts: Vec<String>,
    /// Every config file read, for the watcher.
    pub files: Vec<PathBuf>,
}

pub fn parse() -> Parsed {
    let mut p = Parsed { hosts: vec![], files: vec![] };
    parse_file(&ssh_dir().join("config"), &mut p, 0);
    p
}

#[cfg(test)]
pub fn parse_str(text: &str) -> Vec<String> {
    let mut p = Parsed { hosts: vec![], files: vec![] };
    parse_text(text, &mut p, 0);
    p.hosts
}

fn parse_file(path: &Path, out: &mut Parsed, depth: usize) {
    if depth > 16 || out.files.iter().any(|f| f == path) {
        return;
    }
    out.files.push(path.to_path_buf());
    if let Ok(text) = std::fs::read_to_string(path) {
        parse_text(&text, out, depth);
    }
}

fn parse_text(text: &str, out: &mut Parsed, depth: usize) {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, rest) = split_keyword(line);
        let args = split_args(rest);
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                for a in args {
                    if a.contains(['*', '?', '!']) || out.hosts.contains(&a) {
                        continue;
                    }
                    out.hosts.push(a);
                }
            }
            "include" => {
                for a in args {
                    for path in expand_include(&a) {
                        parse_file(&path, out, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }
}

fn split_keyword(line: &str) -> (&str, &str) {
    let end = line
        .find(|c: char| c.is_whitespace() || c == '=')
        .unwrap_or(line.len());
    let key = &line[..end];
    let rest = line[end..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    (key, rest)
}

fn split_args(s: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

fn expand_include(arg: &str) -> Vec<PathBuf> {
    let home = dirs::home_dir().unwrap_or_default();
    let p = if let Some(rest) = arg.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(arg).is_absolute() {
        PathBuf::from(arg)
    } else {
        ssh_dir().join(arg)
    };
    let pattern = p.to_string_lossy().replace('\\', "/");
    let mut found: Vec<PathBuf> = glob::glob(&pattern)
        .map(|g| g.filter_map(Result::ok).filter(|p| p.is_file()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hosts() {
        let text = "Host a b\n  HostName x\nHost *.corp c?\nHost=d\nHost \"e f\" !g\nMatch host z\n  User q\nhost a\n";
        assert_eq!(parse_str(text), vec!["a", "b", "d", "e f"]);
    }
}
