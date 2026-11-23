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

struct State {
    p: Parsed,
    /// Aliases from `Host` blocks marked with `# myterm: ignore`.
    ignored: Vec<String>,
}

impl State {
    fn new() -> Self {
        State { p: Parsed { hosts: vec![], files: vec![] }, ignored: vec![] }
    }

    fn finish(mut self) -> Parsed {
        let ignored = self.ignored;
        self.p.hosts.retain(|h| !ignored.contains(h));
        self.p
    }
}

pub fn parse() -> Parsed {
    let mut s = State::new();
    parse_file(&ssh_dir().join("config"), &mut s, 0);
    s.finish()
}

#[cfg(test)]
pub fn parse_str(text: &str) -> Vec<String> {
    let mut s = State::new();
    parse_text(text, &mut s, 0);
    s.finish().hosts
}

/// True for a comment line such as `# myterm: ignore` or `#myterm ignore`.
fn is_ignore_marker(line: &str) -> bool {
    let Some(rest) = line.strip_prefix('#') else { return false };
    let rest = rest.trim().to_ascii_lowercase();
    let Some(rest) = rest.strip_prefix("myterm") else { return false };
    let rest = rest.trim_start();
    let rest = rest.strip_prefix(':').unwrap_or(rest).trim();
    rest == "ignore"
}

fn parse_file(path: &Path, out: &mut State, depth: usize) {
    if depth > 16 || out.p.files.iter().any(|f| f == path) {
        return;
    }
    out.p.files.push(path.to_path_buf());
    if let Ok(text) = std::fs::read_to_string(path) {
        parse_text(&text, out, depth);
    }
}

fn parse_text(text: &str, out: &mut State, depth: usize) {
    // Aliases named by the `Host` line of the block we are currently in.
    let mut block: Vec<String> = vec![];
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            if is_ignore_marker(line) {
                out.ignored.extend(block.iter().cloned());
            }
            continue;
        }
        let (key, rest) = split_keyword(line);
        let args = split_args(rest);
        match key.to_ascii_lowercase().as_str() {
            "host" => {
                block.clear();
                for a in args {
                    if a.contains(['*', '?', '!']) {
                        continue;
                    }
                    block.push(a.clone());
                    if !out.p.hosts.contains(&a) {
                        out.p.hosts.push(a);
                    }
                }
            }
            "match" => block.clear(),
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

    #[test]
    fn ignore_marker() {
        let text = "\
# myterm: ignore
Host a
  HostName x
Host b c
  # myterm: ignore
  User q
Host d
  #MyTerm ignore
Host e
  # myterm: ignored-not-really
Match host f
  # myterm: ignore
Host g
";
        assert_eq!(parse_str(text), vec!["a", "e", "g"]);
        assert!(is_ignore_marker("#myterm:ignore"));
        assert!(!is_ignore_marker("# not myterm: ignore"));
    }
}
