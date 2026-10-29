//! The niri configuration, edited with a form instead of raw text.
//!
//! niri's configuration is KDL. Instead of pretending to understand the whole
//! language, the settings center exposes the options people actually change
//! and writes them back into the file where they already live (or into the
//! LARD fragment), keeping a backup of the previous file.

use kdl::{KdlDocument, KdlValue};
use lard_core::Color;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A value of the niri configuration.
#[derive(Clone, Debug, PartialEq)]
pub enum NiriValue {
    Int(i128),
    Float(f64),
    Bool(bool),
    Text(String),
}

impl NiriValue {
    #[must_use]
    pub fn as_int(&self) -> Option<i128> {
        match self {
            Self::Int(value) => Some(*value),
            Self::Float(value) => Some(*value as i128),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(value) => Some(*value),
            Self::Int(value) => Some(*value as f64),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            _ => None,
        }
    }
}

/// The settings exposed by the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NiriSetting {
    Gaps,
    BorderWidth,
    BorderRadius,
    FocusRingWidth,
    FocusRingRadius,
    ColumnWidth,
    WindowHeight,
    CenterFocusedColumn,
    TabBarEnabled,
    TabBarPosition,
    TabBarTitle,
    StrutTop,
    StrutRight,
    StrutBottom,
    StrutLeft,
    OutputMode,
    OutputScale,
    OutputPositionX,
    OutputPositionY,
}

impl NiriSetting {
    /// The label shown in the settings center.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Gaps => "Window gaps",
            Self::BorderWidth => "Window border",
            Self::BorderRadius => "Border radius",
            Self::FocusRingWidth => "Focus ring",
            Self::FocusRingRadius => "Focus ring radius",
            Self::ColumnWidth => "Default width",
            Self::WindowHeight => "Default height",
            Self::CenterFocusedColumn => "Center column",
            Self::TabBarEnabled => "Tab bar",
            Self::TabBarPosition => "Tab bar position",
            Self::TabBarTitle => "Tab title",
            Self::StrutTop => "Reserved space at the top",
            Self::StrutRight => "Reserved space on the right",
            Self::StrutBottom => "Reserved space at the bottom",
            Self::StrutLeft => "Reserved space on the left",
            Self::OutputMode => "Screen resolution",
            Self::OutputScale => "Screen scale",
            Self::OutputPositionX => "Position X",
            Self::OutputPositionY => "Position Y",
        }
    }

    /// The KDL path of the setting inside the layout/output node.
    #[must_use]
    pub fn path(self) -> &'static [&'static str] {
        match self {
            Self::Gaps => &["layout", "gaps"],
            Self::BorderWidth => &["layout", "border", "width"],
            Self::BorderRadius => &["layout", "border", "corner-radius"],
            Self::FocusRingWidth => &["layout", "focus-ring", "width"],
            Self::FocusRingRadius => &["layout", "focus-ring", "corner-radius"],
            Self::ColumnWidth => &["layout", "default-column-width", "fraction"],
            Self::WindowHeight => &["layout", "default-window-height", "fraction"],
            Self::CenterFocusedColumn => &["layout", "center-focused-column"],
            Self::TabBarEnabled => &["layout", "tab-bar", "enabled"],
            Self::TabBarPosition => &["layout", "tab-bar", "position"],
            Self::TabBarTitle => &["layout", "tab-bar", "show-tab-title"],
            Self::StrutTop => &["layout", "struts", "top"],
            Self::StrutRight => &["layout", "struts", "right"],
            Self::StrutBottom => &["layout", "struts", "bottom"],
            Self::StrutLeft => &["layout", "struts", "left"],
            Self::OutputMode => &["output", "mode"],
            Self::OutputScale => &["output", "scale"],
            Self::OutputPositionX | Self::OutputPositionY => &["output", "position"],
        }
    }

    /// The file the setting is written to when it does not exist yet.
    #[must_use]
    pub fn preferred_file(self) -> &'static str {
        match self {
            Self::OutputMode | Self::OutputScale | Self::OutputPositionX | Self::OutputPositionY => "config.kdl",
            _ => "lard/layout.kdl",
        }
    }

    /// The allowed values of the enumerated settings.
    #[must_use]
    pub fn options(self) -> &'static [&'static str] {
        match self {
            Self::CenterFocusedColumn => &["never", "on-overflow", "always"],
            Self::TabBarPosition => &["top", "bottom", "left", "right"],
            _ => &[],
        }
    }
}

/// The niri configuration directory.
#[must_use]
pub fn niri_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return PathBuf::from(dir).join("niri");
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".config/niri"))
        .unwrap_or_else(|| PathBuf::from(".config/niri"))
}

/// The configuration file niri is currently running with: the systemd drop-in

/// One editable file of the niri configuration.
#[derive(Clone, Debug)]
pub struct NiriFile {
    /// Short name shown in the list.
    pub label: String,
    pub path: PathBuf,
}

/// wins, then the LARD variant, then the default one.
#[must_use]
pub fn active_niri_config() -> PathBuf {
    let dropin = std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".config/systemd/user/niri.service.d/10-lard.conf"))
        .unwrap_or_default();
    if let Ok(text) = std::fs::read_to_string(&dropin)
        && let Some(value) = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("Environment=NIRI_CONFIG="))
    {
        let path = PathBuf::from(value.trim_matches('"'));
        if path.is_file() {
            return path;
        }
    }
    let dir = niri_config_dir();
    for name in ["config-lard.kdl", "config.kdl"] {
        let path = dir.join(name);
        if path.is_file() {
            return path;
        }
    }
    dir.join("config.kdl")
}

/// One parsed configuration file.
struct Doc {
    path: PathBuf,
    doc: KdlDocument,
    /// The original text: the values are patched in place, so the rest of the
    /// file (comments included) stays exactly as it was.
    text: String,
    dirty: bool,
}

/// The theme values that belong to niri: the shell follows them instead of
/// keeping a second copy in its own configuration.
#[derive(Clone, Copy, Debug, Default)]
pub struct NiriTheme {
    /// `layout.background-color`
    pub background: Option<Color>,
    /// `layout.focus-ring.color`
    pub accent: Option<Color>,
    /// `layout.border.urgent-color`
    pub danger: Option<Color>,
}

/// Read the theme values niri owns from its configuration files.
#[must_use]
pub fn niri_theme(dir: &Path) -> NiriTheme {
    let mut theme = NiriTheme::default();
    let mut files: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir.join("lard")) {
        let mut fragments: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "kdl"))
            .collect();
        fragments.sort();
        files.extend(fragments);
    }
    for name in ["config-lard.kdl", "config.kdl"] {
        let path = dir.join(name);
        if path.is_file() {
            files.push(path);
            break;
        }
    }
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = KdlDocument::parse_v1(&text) else {
            continue;
        };
        if theme.background.is_none()
            && let Some(color) = read_value(&doc, &["layout", "background-color"])
                .and_then(|value| value.as_text().map(str::to_string))
                .and_then(|text| Color::from_hex(&text).ok())
                // niri leaves the background transparent to show the wallpaper.
                .filter(|color| color.a > 0.0)
        {
            theme.background = Some(color);
        }
        if theme.accent.is_none()
            && let Some(color) = read_value(&doc, &["layout", "focus-ring", "color"])
                .and_then(|value| value.as_text().map(str::to_string))
                .and_then(|text| Color::from_hex(&text).ok())
        {
            theme.accent = Some(color);
        }
        if theme.danger.is_none()
            && let Some(color) = read_value(&doc, &["layout", "border", "urgent-color"])
                .and_then(|value| value.as_text().map(str::to_string))
                .and_then(|text| Color::from_hex(&text).ok())
        {
            theme.danger = Some(color);
        }
    }
    theme
}

/// The niri configuration as a form.
pub struct NiriConfig {
    docs: Vec<Doc>,
    values: HashMap<NiriSetting, NiriValue>,
    /// Where each setting was found, so the write goes back to the same file.
    origins: HashMap<NiriSetting, PathBuf>,
    output: Option<String>,
}

impl NiriConfig {
    /// Read the niri configuration files.
    #[must_use]
    pub fn load(dir: &Path, output: Option<&str>) -> Self {
        let mut config = Self {
            docs: Vec::new(),
            values: HashMap::new(),
            origins: HashMap::new(),
            output: output.map(str::to_string),
        };
        // The fragments are included last, so they win over the values of the
        // main configuration: they come first in the search order.
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(dir.join("lard")) {
            let mut fragments: Vec<PathBuf> = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().is_some_and(|ext| ext == "kdl"))
                .collect();
            fragments.sort();
            files.extend(fragments);
        }
        // The configuration niri is running with.
        for name in ["config-lard.kdl", "config.kdl"] {
            let path = dir.join(name);
            if path.is_file() {
                files.push(path);
                break;
            }
        }
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // niri uses KDL v1 (bare identifiers as properties).
            let Ok(doc) = KdlDocument::parse_v1(&text) else {
                continue;
            };
            config.docs.push(Doc {
                path,
                doc,
                text,
                dirty: false,
            });
        }
        config.read_values();
        config
    }

    /// The current value of a setting.
    #[must_use]
    pub fn value(&self, setting: NiriSetting) -> Option<&NiriValue> {
        self.values.get(&setting)
    }

    /// The file a setting came from.
    #[must_use]
    pub fn origin(&self, setting: NiriSetting) -> Option<&Path> {
        self.origins.get(&setting).map(PathBuf::as_path)
    }

    /// Set a value; it is written to disk when the form is applied.
    pub fn set(&mut self, setting: NiriSetting, value: NiriValue) {
        self.values.insert(setting, value);
    }

    /// Add or subtract from a numeric setting.
    pub fn step(&mut self, setting: NiriSetting, delta: f64) {
        let current = self.values.get(&setting).cloned();
        let next = match current {
            Some(NiriValue::Int(value)) => NiriValue::Int(value + delta as i128),
            Some(NiriValue::Float(value)) => NiriValue::Float((value + delta).clamp(0.0, 10.0)),
            _ => return,
        };
        self.values.insert(setting, next);
    }

    /// Flip a boolean setting.
    pub fn toggle(&mut self, setting: NiriSetting) {
        if let Some(NiriValue::Bool(value)) = self.values.get(&setting).cloned() {
            self.values.insert(setting, NiriValue::Bool(!value));
        }
    }

    /// Move to the next allowed value of an enumerated setting.
    pub fn cycle(&mut self, setting: NiriSetting) {
        let options = setting.options();
        if options.is_empty() {
            return;
        }
        let current = self
            .values
            .get(&setting)
            .and_then(NiriValue::as_text)
            .unwrap_or_default()
            .to_string();
        let index = options
            .iter()
            .position(|option| *option == current)
            .unwrap_or(0);
        let next = options[(index + 1) % options.len()];
        self.values
            .insert(setting, NiriValue::Text(next.to_string()));
    }

    /// True when something changed and the files have to be written.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.docs.iter().any(|doc| doc.dirty)
    }

    /// Write the changed files, keeping a `.bak` copy. Returns the paths.
    pub fn save(&mut self) -> Vec<PathBuf> {
        let mut written = Vec::new();
        for doc in &mut self.docs {
            if !doc.dirty {
                continue;
            }
            if doc.path.exists() {
                let backup = doc.path.with_extension("kdl.bak");
                let _ = std::fs::copy(&doc.path, &backup);
            }
            let tmp = doc.path.with_extension("kdl.tmp");
            if std::fs::write(&tmp, &doc.text).is_ok() && std::fs::rename(&tmp, &doc.path).is_ok() {
                written.push(doc.path.clone());
                doc.dirty = false;
            }
        }
        written
    }

    /// Read every setting from the parsed files.
    fn read_values(&mut self) {
        for setting in [
            NiriSetting::Gaps,
            NiriSetting::BorderWidth,
            NiriSetting::BorderRadius,
            NiriSetting::FocusRingWidth,
            NiriSetting::FocusRingRadius,
            NiriSetting::ColumnWidth,
            NiriSetting::WindowHeight,
            NiriSetting::CenterFocusedColumn,
            NiriSetting::TabBarEnabled,
            NiriSetting::TabBarPosition,
            NiriSetting::TabBarTitle,
            NiriSetting::StrutTop,
            NiriSetting::StrutRight,
            NiriSetting::StrutBottom,
            NiriSetting::StrutLeft,
        ] {
            for doc in &self.docs {
                if let Some(value) = read_value(&doc.doc, setting.path()) {
                    self.values.insert(setting, value);
                    self.origins.insert(setting, doc.path.clone());
                    break;
                }
            }
        }
        // The output settings live in the block of the focused output.
        if self.output.is_some() {
            if let Some(output) = self.output.clone() {
                for doc in &self.docs {
                    let Some(node) = doc.doc.get("output") else {
                        continue;
                    };
                    let is_output = node.name().value() == "output"
                        && node
                            .entries()
                            .iter()
                            .any(|entry| entry.value().as_string() == Some(output.as_str()));
                    if !is_output {
                        continue;
                    }
                    if let Some(value) = node
                        .get("mode")
                        .and_then(|value| value.as_string().map(str::to_string))
                    {
                        self.values
                            .insert(NiriSetting::OutputMode, NiriValue::Text(value));
                        self.origins
                            .insert(NiriSetting::OutputMode, doc.path.clone());
                    }
                    if let Some(value) = node.get("scale").and_then(|value| value.as_float()) {
                        self.values
                            .insert(NiriSetting::OutputScale, NiriValue::Float(value));
                        self.origins
                            .insert(NiriSetting::OutputScale, doc.path.clone());
                    }
                    if let Some(pos) = node.children().and_then(|c| c.get("position")) {
                        for entry in pos.entries() {
                            let name = entry.name().map(|n| n.value()).unwrap_or("");
                            let val = match entry.value() {
                                KdlValue::Integer(v) => *v as i64,
                                _ => 0,
                            };
                            if name == "x" {
                                self.values.insert(
                                    NiriSetting::OutputPositionX,
                                    NiriValue::Int(val as i128),
                                );
                                self.origins.insert(
                                    NiriSetting::OutputPositionX,
                                    doc.path.clone(),
                                );
                            } else if name == "y" {
                                self.values.insert(
                                    NiriSetting::OutputPositionY,
                                    NiriValue::Int(val as i128),
                                );
                                self.origins.insert(
                                    NiriSetting::OutputPositionY,
                                    doc.path.clone(),
                                );
                            }
                        }
                    }
                    break;
                }
            }
        }
    }

    /// Write the pending values into the documents.
    pub fn apply(&mut self) {
        for (setting, value) in self.values.clone() {
            let path = setting.path();
            let target = self.origins.get(&setting).cloned().or_else(|| {
                self.docs
                    .iter()
                    .find(|doc| {
                        doc.path
                            .file_name()
                            .is_some_and(|name| name == setting.preferred_file())
                    })
                    .map(|doc| doc.path.clone())
            });
            let Some(target) = target else {
                continue;
            };
            let Some(doc) = self.docs.iter_mut().find(|doc| doc.path == target) else {
                continue;
            };
            if setting.path()[0] == "output" {
                if self.output.is_none() {
                    continue;
                }
                // Position is a node with x/y properties, handled specially.
                if setting == NiriSetting::OutputPositionX
                    || setting == NiriSetting::OutputPositionY
                {
                    let output_name = self.output.clone().unwrap_or_default();
                    let x = if setting == NiriSetting::OutputPositionX {
                        match value {
                            NiriValue::Int(v) => v as i32,
                            _ => 0,
                        }
                    } else {
                        self.values
                            .get(&NiriSetting::OutputPositionX)
                            .and_then(NiriValue::as_int)
                            .unwrap_or(0) as i32
                    };
                    let y = if setting == NiriSetting::OutputPositionY {
                        match value {
                            NiriValue::Int(v) => v as i32,
                            _ => 0,
                        }
                    } else {
                        self.values
                            .get(&NiriSetting::OutputPositionY)
                            .and_then(NiriValue::as_int)
                            .unwrap_or(0) as i32
                    };
                    if let Some(text) =
                        set_position_in_text(&doc.text, &output_name, x, y)
                    {
                        doc.text = text;
                        doc.dirty = true;
                    }
                    continue;
                }
                let key = if matches!(value, NiriValue::Text(_)) {
                    "mode"
                } else {
                    "scale"
                };
                let path: Vec<&str> = if key == "mode" {
                    vec!["output", "mode"]
                } else {
                    vec!["output", "scale"]
                };
                if let Some(text) = set_in_text(&doc.text, &path, &value_text(&value)) {
                    doc.text = text;
                    doc.dirty = true;
                }
                continue;
            }
            if let Some(text) = set_in_text(&doc.text, path, &value_text(&value)) {
                doc.text = text;
                doc.dirty = true;
            }
        }
    }
}

/// The KDL text of a value.
fn value_text(value: &NiriValue) -> String {
    match value {
        NiriValue::Int(number) => number.to_string(),
        NiriValue::Float(number) => format!("{number:.2}"),
        NiriValue::Bool(flag) => flag.to_string(),
        NiriValue::Text(text) => format!("\"{text}\""),
    }
}

/// Insert or update the `position x=.. y=..` line inside the output block
/// named `output`. Returns the new document text.
fn set_position_in_text(text: &str, output: &str, x: i32, y: i32) -> Option<String> {
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    // Find the output block for this monitor.
    let mut start_idx = None;
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if t.starts_with("output ") && t.contains(output) {
            start_idx = Some(i);
            break;
        }
    }
    let start = start_idx?;
    // Find the closing brace.
    let mut end_idx = None;
    for (i, line) in lines[start + 1..].iter().enumerate() {
        if line.trim() == "}" {
            end_idx = Some(start + 1 + i);
            break;
        }
    }
    let end = end_idx.unwrap_or(lines.len());

    // Find an existing position line.
    let mut pos_idx = None;
    for i in (start + 1)..end {
        if lines[i].trim().starts_with("position") {
            pos_idx = Some(i);
            break;
        }
    }
    let indent = "    ";
    let new_line = format!("{indent}position x={x} y={y}\n");
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    if let Some(idx) = pos_idx {
        out[idx] = new_line;
    } else {
        out.insert(end, new_line);
    }
    Some(out.join(""))
}

/// Patch the value of `key` inside the blocks of `path`, leaving the rest of
/// the file (and its comments) untouched.
fn set_in_text(text: &str, path: &[&str], value: &str) -> Option<String> {
    let key = *path.last()?;
    let mut stack: Vec<&str> = Vec::new();
    let mut out = String::with_capacity(text.len() + 32);
    let mut done = false;
    for line in text.split_inclusive('\n') {
        let body = line.trim_end_matches(['\n', '\r']);
        let indent_len = body.len() - body.trim_start().len();
        let indent = &body[..indent_len];
        let trimmed = body.trim_start();
        if !done && stack.len() + 1 == path.len() && trimmed.starts_with(key) {
            let rest = trimmed[key.len()..].trim_start();
            let is_value = rest.starts_with('=')
                || rest.starts_with('"')
                || rest
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit() || c == '-' || c == '+');
            if is_value {
                // Keep the trailing comment of the line.
                let comment = rest.find("//").map(|index| &rest[index..]).unwrap_or("");
                out.push_str(indent);
                out.push_str(key);
                out.push(' ');
                out.push_str(value);
                if !comment.is_empty() && !comment.starts_with(char::is_whitespace) {
                    out.push(' ');
                }
                out.push_str(comment);
                if line.ends_with('\n') {
                    out.push('\n');
                }
                done = true;
                continue;
            }
        }
        // Track the blocks we are inside of.
        if let Some(open) = trimmed.find('{') {
            let name = trimmed[..open].trim();
            let expected = path.get(stack.len()).copied().unwrap_or("\u{0}");
            stack.push(if name == expected { name } else { "\u{0}" });
        } else if trimmed.starts_with('}') {
            stack.pop();
        }
        out.push_str(line);
    }
    done.then_some(out)
}

/// Read a value from a KDL document.
fn read_value(doc: &KdlDocument, path: &[&str]) -> Option<NiriValue> {
    let (head, rest) = path.split_first()?;
    let node = doc.get(head)?;
    if rest.is_empty() {
        return node.entries().first().map(|entry| to_value(entry.value()));
    }
    let child = node.children()?.get(rest[0])?;
    if rest.len() == 1 {
        return child.entries().first().map(|entry| to_value(entry.value()));
    }
    let leaf = child.children()?.get(rest[1])?;
    leaf.entries().first().map(|entry| to_value(entry.value()))
}

fn to_value(value: &KdlValue) -> NiriValue {
    if let Some(number) = value.as_integer() {
        return NiriValue::Int(number);
    }
    if let Some(number) = value.as_float() {
        return NiriValue::Float(number);
    }
    if let Some(flag) = value.as_bool() {
        return NiriValue::Bool(flag);
    }
    NiriValue::Text(value.as_string().unwrap_or_default().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_values_back() {
        let dir = std::env::temp_dir().join(format!("lard-niri-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("lard")).unwrap();
        std::fs::write(
            dir.join("lard/layout.kdl"),
            "// a comment\nlayout {\n    gaps 4\n}\n",
        )
        .unwrap();
        let mut config = NiriConfig::load(&dir, None);
        assert_eq!(config.value(NiriSetting::Gaps), Some(&NiriValue::Int(4)));
        config.set(NiriSetting::Gaps, NiriValue::Int(12));
        config.apply();
        let written = config.save();
        assert_eq!(written.len(), 1);
        let text = std::fs::read_to_string(dir.join("lard/layout.kdl")).unwrap();
        assert!(text.contains("gaps 12"), "{text}");
        // The comments and the layout survive the round trip.
        assert!(text.contains("a comment"), "{text}");
        assert!(text.contains("layout {"), "{text}");
        // And the previous file is kept as a backup.
        assert!(dir.join("lard/layout.kdl.bak").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_nested_layout_values() {
        let text = "// ! DO NOT EDIT !\n\nlayout {\n    gaps 4\n\n    border {\n        width 2\n    }\n\n    focus-ring {\n        width 2\n    }\n}\n";
        let doc = KdlDocument::parse_v1(text).unwrap();
        assert_eq!(
            read_value(&doc, &["layout", "gaps"]),
            Some(NiriValue::Int(4))
        );
        assert_eq!(
            read_value(&doc, &["layout", "border", "width"]),
            Some(NiriValue::Int(2))
        );
        assert_eq!(
            read_value(&doc, &["layout", "focus-ring", "width"]),
            Some(NiriValue::Int(2))
        );
    }

    #[test]
    fn reads_the_values_niri_owns() {
        let dir = std::env::temp_dir().join(format!("lard-niri-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.kdl"),
            "layout {\n    background-color \"#102030\"\n    focus-ring {\n        color \"#ff8800\"\n    }\n    border {\n        urgent-color \"#cc4444\"\n    }\n}\n",
        )
        .unwrap();
        let theme = niri_theme(&dir);
        assert_eq!(
            theme.background.map(|c| c.to_hex().to_lowercase()),
            Some("#102030".to_string())
        );
        assert_eq!(
            theme.accent.map(|c| c.to_hex().to_lowercase()),
            Some("#ff8800".to_string())
        );
        assert_eq!(
            theme.danger.map(|c| c.to_hex().to_lowercase()),
            Some("#cc4444".to_string())
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_transparent_background_is_ignored() {
        let dir = std::env::temp_dir().join(format!("lard-niri-theme2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.kdl"),
            "layout {\n    background-color \"transparent\"\n}\n",
        )
        .unwrap();
        let theme = niri_theme(&dir);
        assert!(theme.background.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn patches_a_value_in_place() {
        let text = "layout {\n    gaps 4\n}\n";
        let patched = set_in_text(text, &["layout", "gaps"], "12").expect("patched");
        assert!(patched.contains("gaps 12"), "{patched}");
        // The indentation and the rest of the file are untouched.
        assert!(patched.contains("    gaps 12"), "{patched}");
        // A key that is not there is not invented.
        assert!(set_in_text(text, &["layout", "border", "width"], "3").is_none());
        // The trailing comment survives.
        let with_comment = "layout {\n    gaps 4 // the spacing\n}\n";
        let patched = set_in_text(with_comment, &["layout", "gaps"], "8").expect("patched");
        assert!(patched.contains("gaps 8 // the spacing"), "{patched}");
    }
}
