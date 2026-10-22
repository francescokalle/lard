//! Minimal freedesktop `.desktop` parsing shared by the launcher and greeter.
//!
//! Only the keys the suite needs are exposed. The parser is deliberately
//! allocation-friendly and never executes a shell: `Exec` is tokenized into an
//! argv vector.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct DesktopEntry {
    pub file: PathBuf,
    pub name: String,
    pub generic_name: String,
    pub comment: String,
    pub exec: String,
    pub icon: String,
    pub terminal: bool,
    pub categories: Vec<String>,
}

impl DesktopEntry {
    #[must_use]
    pub fn argv(&self) -> Vec<String> {
        split_exec(&self.exec)
    }

    /// Two-letter-ish monogram used instead of decoding icon images.
    #[must_use]
    pub fn initials(&self) -> String {
        let source = if self.name.is_empty() {
            &self.exec
        } else {
            &self.name
        };
        let mut words = source.split_whitespace();
        let first = words.next().and_then(|w| w.chars().next()).unwrap_or('?');
        let second = words.next().and_then(|w| w.chars().next());
        match second {
            Some(c) => format!("{}{}", first.to_uppercase(), c.to_uppercase()),
            None => first.to_uppercase().to_string(),
        }
    }

    #[must_use]
    pub fn subtitle(&self) -> String {
        if !self.generic_name.is_empty() {
            self.generic_name.clone()
        } else if !self.comment.is_empty() {
            self.comment.clone()
        } else {
            self.categories.first().cloned().unwrap_or_default()
        }
    }
}

/// Parse the `[Desktop Entry]` group. Unrelated groups and keys are ignored.
#[must_use]
pub fn parse_desktop(text: &str, file: &Path) -> Option<DesktopEntry> {
    let mut entry = DesktopEntry {
        file: file.to_path_buf(),
        ..Default::default()
    };
    let mut in_group = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_group = line.eq_ignore_ascii_case("[Desktop Entry]");
            continue;
        }
        if !in_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = unescape(value.trim());
        match key.trim() {
            "Name" => entry.name = value,
            "GenericName" => entry.generic_name = value,
            "Comment" => entry.comment = value,
            "Exec" => entry.exec = value,
            "Icon" => entry.icon = value,
            "Terminal" => entry.terminal = value.eq_ignore_ascii_case("true"),
            "NoDisplay" => {
                if value.eq_ignore_ascii_case("true") {
                    return None;
                }
            }
            "Hidden" => {
                if value.eq_ignore_ascii_case("true") {
                    return None;
                }
            }
            "Categories" => {
                entry.categories = value
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            _ => {}
        }
    }
    (!entry.name.is_empty() && !entry.exec.is_empty()).then_some(entry)
}

/// Parse all `*.desktop` files in `dir`, skipping hidden/no-display entries.
pub fn scan_applications() -> Vec<DesktopEntry> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(home).join("applications"));
    } else if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for dir in std::env::split_paths(&data_dirs) {
            dirs.push(dir.join("applications"));
        }
    } else {
        dirs.push(PathBuf::from("/usr/local/share/applications"));
        dirs.push(PathBuf::from("/usr/share/applications"));
    }

    let mut seen = std::collections::HashSet::new();
    let mut entries = Vec::new();
    for dir in dirs {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in read.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !seen.insert(name.to_string()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(entry) = parse_desktop(&text, &path) {
                entries.push(entry);
            }
        }
    }
    entries.sort_by_cached_key(|entry| entry.name.to_lowercase());
    entries
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionEntry {
    pub name: String,
    pub exec: Vec<String>,
    pub file: PathBuf,
}

/// Scan Wayland session desktop files (`niri`, `Hyprland`, `sway`, …).
#[must_use]
pub fn scan_wayland_sessions() -> Vec<SessionEntry> {
    let mut dirs = vec![
        PathBuf::from("/usr/local/share/wayland-sessions"),
        PathBuf::from("/usr/share/wayland-sessions"),
    ];
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        for dir in std::env::split_paths(&data_dirs) {
            dirs.push(dir.join("wayland-sessions"));
        }
    }

    let mut sessions = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for dir in dirs {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for file in read.flatten() {
            let path = file.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !seen.insert(stem.to_string()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if let Some(entry) = parse_desktop(&text, &path) {
                let exec = entry.argv();
                if !exec.is_empty() {
                    sessions.push(SessionEntry {
                        name: entry.name,
                        exec,
                        file: path,
                    });
                }
            }
        }
    }
    sessions.sort_by_cached_key(|session| session.name.to_lowercase());
    sessions
}

/// Tokenize a freedesktop `Exec` line, expanding `%%` and dropping `%f`,
/// `%F`, `%u`, `%U`, `%i`, `%c`, `%k`.
#[must_use]
pub fn split_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut chars = exec.chars().peekable();

    while let Some(c) = chars.next() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' if quote != Some('\'') => escaped = true,
            '"' | '\'' => {
                if quote == Some(c) {
                    quote = None;
                } else if quote.is_none() {
                    quote = Some(c);
                } else {
                    current.push(c);
                }
            }
            c if c.is_whitespace() && quote.is_none() => {
                if !current.is_empty() {
                    let arg = current.trim();
                    if !arg.is_empty() {
                        args.push(arg.to_string());
                    }
                    current.clear();
                }
            }
            '%' => match chars.next() {
                Some('%') => current.push('%'),
                Some('f' | 'F' | 'u' | 'U' | 'i' | 'c' | 'k') => {}
                Some(other) => {
                    current.push('%');
                    current.push(other);
                }
                None => current.push('%'),
            },
            _ => current.push(c),
        }
    }
    let arg = current.trim();
    if !arg.is_empty() {
        args.push(arg.to_string());
    }
    args
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('s') => out.push(' '),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entry() {
        let text =
            "[Desktop Entry]\nType=Application\nName=Firefox\nExec=firefox %u\nIcon=firefox\n";
        let entry = parse_desktop(text, Path::new("firefox.desktop")).unwrap();
        assert_eq!(entry.name, "Firefox");
        assert_eq!(entry.argv(), vec!["firefox"]);
    }

    #[test]
    fn hides_nodisplay() {
        let text = "[Desktop Entry]\nName=X\nExec=x\nNoDisplay=true\n";
        assert!(parse_desktop(text, Path::new("x.desktop")).is_none());
    }

    #[test]
    fn splits_quotes_and_codes() {
        assert_eq!(
            split_exec("foot -e \"htop --sort-key %U\" %F"),
            vec!["foot", "-e", "htop --sort-key"]
        );
        assert_eq!(split_exec("app %% --flag"), vec!["app", "%", "--flag"]);
    }
}
