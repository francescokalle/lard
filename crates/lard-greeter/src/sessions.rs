//! Login session discovery and user enumeration for the greeter.

pub use lard_core::desktop::SessionEntry;
use lard_core::desktop::scan_wayland_sessions;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct User {
    pub name: String,
    pub display_name: String,
    pub uid: u32,
    pub home: PathBuf,
}

impl User {
    #[must_use]
    pub fn initials(&self) -> String {
        let mut words = self.display_name.split_whitespace();
        let first = words.next().and_then(|w| w.chars().next()).unwrap_or('?');
        match words.next().and_then(|w| w.chars().next()) {
            Some(second) => format!("{}{}", first.to_uppercase(), second.to_uppercase()),
            None => first.to_uppercase().to_string(),
        }
    }
}

#[must_use]
pub fn discover_users() -> Vec<User> {
    let mut users = Vec::new();
    if let Ok(text) = std::fs::read_to_string("/etc/passwd") {
        for line in text.lines() {
            let fields: Vec<&str> = line.split(':').collect();
            if fields.len() < 7 {
                continue;
            }
            let Ok(uid) = fields[2].parse::<u32>() else {
                continue;
            };
            let name = fields[0];
            let shell = fields[6];
            if !(1000..60000).contains(&uid)
                || name == "nobody"
                || name == "greeter"
                || shell.ends_with("nologin")
                || shell.ends_with("false")
            {
                continue;
            }
            let gecos = fields[4].split(',').next().unwrap_or("").trim();
            users.push(User {
                name: name.to_string(),
                display_name: if gecos.is_empty() {
                    capitalize(name)
                } else {
                    gecos.to_string()
                },
                uid,
                home: PathBuf::from(fields[5]),
            });
        }
    }

    if users.is_empty()
        && let Some(name) = std::env::var_os("USER").or_else(|| std::env::var_os("LOGNAME"))
    {
        let name = name.to_string_lossy().to_string();
        users.push(User {
            name: name.clone(),
            display_name: capitalize(&name),
            uid: 1000,
            home: PathBuf::from("/"),
        });
    }
    users.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
    });
    users
}

#[must_use]
pub fn current_user() -> User {
    let name = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_else(|_| "user".into());
    let known = discover_users().into_iter().find(|u| u.name == name);
    known.unwrap_or_else(|| User {
        display_name: capitalize(&name),
        name,
        uid: 1000,
        home: PathBuf::from("/"),
    })
}

/// Session list + index of the configured default (niri when present).
#[must_use]
pub fn discover_sessions(default: &str) -> (Vec<SessionEntry>, usize) {
    let mut sessions = scan_wayland_sessions();
    if sessions.is_empty() {
        sessions.push(SessionEntry {
            name: "Niri".into(),
            exec: vec!["niri-session".into()],
            file: PathBuf::new(),
        });
    }
    let needle = default.to_lowercase();
    let selected = sessions
        .iter()
        .position(|s| {
            s.name.to_lowercase().contains(&needle)
                || s.file
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .is_some_and(|stem| stem.to_lowercase().contains(&needle))
        })
        .unwrap_or(0);
    (sessions, selected)
}

fn capitalize(input: &str) -> String {
    let mut chars = input.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
