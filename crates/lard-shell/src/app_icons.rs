//! Real application icons for the launcher and the dock.
//!
//! The desktop entry `Icon=` name is resolved against the freedesktop icon
//! theme directories (including the flatpak exports), then decoded with
//! `image` (raster) or `resvg` (SVG) and uploaded to the renderer atlas. When
//! no icon file can be found the caller falls back to the symbolic Material
//! glyph, so the UI never shows an empty slot.

use lard_core::desktop::DesktopEntry;
use lard_render::Scene;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// First atlas id used for app icons. The wallpaper carousel uses the lower
/// ids, so the two never share a cell.
const FIRST_ICON_ID: u32 = 128;
/// Last usable id (the atlas has 256 cells of 128x128).
const LAST_ICON_ID: u32 = 255;
/// Edge length of the decoded icon bitmap.
const ICON_PX: u32 = 64;
/// How many icons may be decoded per frame, so opening the launcher never
/// blocks on a dozen PNG/SVG decodes.
const DECODE_BUDGET: usize = 3;

/// Icon sizes we look for, best first.
const SIZES: &[&str] = &[
    "128x128", "96x96", "64x64", "48x48", "256x256", "512x512", "32x32", "16x16",
];

pub struct AppIcons {
    /// Icon name -> resolved file, cached (a miss is cached too).
    resolved: HashMap<String, Option<PathBuf>>,
    /// File -> atlas id, `None` when the file could not be decoded.
    loaded: HashMap<PathBuf, Option<u32>>,
    next_id: u32,
    budget: usize,
}

impl Default for AppIcons {
    fn default() -> Self {
        Self::new()
    }
}

impl AppIcons {
    pub fn new() -> Self {
        Self {
            resolved: HashMap::new(),
            loaded: HashMap::new(),
            next_id: FIRST_ICON_ID,
            budget: DECODE_BUDGET,
        }
    }

    /// Reset the per-frame decode budget.
    pub fn begin_frame(&mut self) {
        self.budget = DECODE_BUDGET;
    }

    /// Atlas id of the icon of `entry`, decoding it on first use.
    ///
    /// Returns `None` when the app has no icon file or when the decode budget
    /// for this frame is already spent: the caller then draws the symbolic
    /// glyph and the real icon appears a frame later.
    pub fn id_for(&mut self, entry: &DesktopEntry, scene: &mut Scene) -> Option<u32> {
        let key = icon_key(entry);
        if key.is_empty() {
            return None;
        }
        let path = match self.resolved.get(&key) {
            Some(path) => path.clone(),
            None => {
                let found = resolve_icon(&key);
                self.resolved.insert(key.clone(), found.clone());
                found
            }
        };
        let Some(path) = path else {
            return None;
        };
        if let Some(id) = self.loaded.get(&path) {
            return *id;
        }
        if self.budget == 0 || self.next_id > LAST_ICON_ID {
            return None;
        }
        self.budget -= 1;
        let id = decode_icon(&path).map(|rgba| {
            let id = self.next_id;
            self.next_id += 1;
            scene.upload_image(id, ICON_PX, ICON_PX, rgba);
            id
        });
        self.loaded.insert(path, id);
        id
    }
}

/// Icon name of a desktop entry: the `Icon=` key, or the desktop file stem.
fn icon_key(entry: &DesktopEntry) -> String {
    let raw = if entry.icon.trim().is_empty() {
        entry
            .file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
    } else {
        entry.icon.trim()
    };
    let name = raw.rsplit('/').next().unwrap_or(raw);
    let name = name
        .strip_suffix(".png")
        .or_else(|| name.strip_suffix(".svg"))
        .or_else(|| name.strip_suffix(".xpm"))
        .unwrap_or(name);
    name.to_string()
}

/// Directories that may hold the icon themes.
fn icon_bases() -> Vec<PathBuf> {
    let mut bases = Vec::new();
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    if let Some(home) = data_home {
        bases.push(home.join("icons"));
    }
    match std::env::var_os("XDG_DATA_DIRS") {
        Some(dirs) => {
            for dir in std::env::split_paths(&dirs) {
                if dir.as_os_str().is_empty() {
                    continue;
                }
                bases.push(dir.join("icons"));
            }
        }
        None => {
            bases.push(PathBuf::from("/usr/local/share/icons"));
            bases.push(PathBuf::from("/usr/share/icons"));
        }
    }
    // Sandboxed apps export their icons here.
    bases.push(PathBuf::from("/var/lib/flatpak/exports/share/icons"));
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        bases.push(home.join(".local/share/flatpak/exports/share/icons"));
        bases.push(home.join(".icons"));
    }
    bases.push(PathBuf::from("/usr/share/pixmaps"));
    bases
}

/// Find the icon file of `name` in the icon theme directories.
fn resolve_icon(name: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if name.contains('/') {
        let direct = PathBuf::from(name);
        if direct.is_file() {
            return Some(direct);
        }
    }
    let png = format!("{name}.png");
    let svg = format!("{name}.svg");
    for base in icon_bases() {
        for size in SIZES {
            for sub in ["", "apps"] {
                let mut candidate = base.join("hicolor").join(size);
                if !sub.is_empty() {
                    candidate = candidate.join(sub);
                }
                let candidate = candidate.join(&png);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        for sub in ["", "apps"] {
            let mut candidate = base.join("hicolor").join("scalable");
            if !sub.is_empty() {
                candidate = candidate.join(sub);
            }
            let candidate = candidate.join(&svg);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        // Other themes (Adwaita, AdwaitaLegacy, ...).
        if let Ok(themes) = std::fs::read_dir(&base) {
            for theme in themes.flatten() {
                let theme = theme.path();
                if !theme.is_dir() {
                    continue;
                }
                for size in SIZES {
                    for sub in ["", "apps"] {
                        let mut candidate = theme.join(size);
                        if !sub.is_empty() {
                            candidate = candidate.join(sub);
                        }
                        let candidate = candidate.join(&png);
                        if candidate.is_file() {
                            return Some(candidate);
                        }
                    }
                }
                for sub in ["", "apps"] {
                    let mut candidate = theme.join("scalable");
                    if !sub.is_empty() {
                        candidate = candidate.join(sub);
                    }
                    let candidate = candidate.join(&svg);
                    if candidate.is_file() {
                        return Some(candidate);
                    }
                }
            }
        }
        let flat = base.join(&png);
        if flat.is_file() {
            return Some(flat);
        }
    }
    None
}

/// Decode an icon file into a square RGBA bitmap.
fn decode_icon(path: &Path) -> Option<Vec<u8>> {
    let data = std::fs::read(path).ok()?;
    let is_svg = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("svg"))
        .unwrap_or(false);
    if is_svg {
        decode_svg(&data)
    } else {
        decode_raster(&data)
    }
}

/// Raster icon: fit inside the square, then pad with transparency.
fn decode_raster(data: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(data).ok()?;
    let resized = image.resize(ICON_PX, ICON_PX, image::imageops::FilterType::Triangle);
    let rgba = resized.to_rgba8();
    Some(pad_to_square(&rgba))
}

/// SVG icon: rasterize with resvg, then un-premultiply onto a square canvas.
fn decode_svg(data: &[u8]) -> Option<Vec<u8>> {
    let options = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(data, &options).ok()?;
    let size = tree.size();
    if size.width() <= 0.0 || size.height() <= 0.0 {
        return None;
    }
    let scale = (ICON_PX as f32 / size.width().max(size.height())).clamp(0.01, 8.0);
    let width = (size.width() * scale).ceil().max(1.0) as u32;
    let height = (size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    let source = pixmap.data();
    let mut out = vec![0u8; (ICON_PX * ICON_PX * 4) as usize];
    let offset_x = (ICON_PX.saturating_sub(width)) / 2;
    let offset_y = (ICON_PX.saturating_sub(height)) / 2;
    for y in 0..height as usize {
        for x in 0..width as usize {
            let src = (y * width as usize + x) * 4;
            let alpha = source[src + 3];
            if alpha == 0 {
                continue;
            }
            let dst = ((y + offset_y as usize) * ICON_PX as usize + (x + offset_x as usize)) * 4;
            for channel in 0..3 {
                // tiny-skia stores premultiplied pixels, the atlas wants
                // straight alpha.
                out[dst + channel] =
                    ((source[src + channel] as u32 * 255) / alpha as u32).min(255) as u8;
            }
            out[dst + 3] = alpha;
        }
    }
    Some(out)
}

/// Center a (possibly smaller) RGBA image on a transparent square.
fn pad_to_square(rgba: &image::RgbaImage) -> Vec<u8> {
    let mut out = vec![0u8; (ICON_PX * ICON_PX * 4) as usize];
    let offset_x = (ICON_PX.saturating_sub(rgba.width())) / 2;
    let offset_y = (ICON_PX.saturating_sub(rgba.height())) / 2;
    for y in 0..rgba.height() as usize {
        for x in 0..rgba.width() as usize {
            let src = (y * rgba.width() as usize + x) * 4;
            let dst = ((y + offset_y as usize) * ICON_PX as usize + (x + offset_x as usize)) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba.as_raw()[src..src + 4]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn entry(icon: &str) -> DesktopEntry {
        DesktopEntry {
            file: PathBuf::from("/usr/share/applications/demo.desktop"),
            name: "Demo".into(),
            generic_name: String::new(),
            comment: String::new(),
            exec: "demo".into(),
            icon: icon.into(),
            terminal: false,
            categories: Vec::new(),
        }
    }

    #[test]
    fn icon_key_falls_back_to_the_desktop_stem() {
        assert_eq!(icon_key(&entry("firefox")), "firefox");
        assert_eq!(icon_key(&entry("firefox.png")), "firefox");
        assert_eq!(
            icon_key(&entry("/usr/share/icons/hicolor/48x48/apps/x.png")),
            "x"
        );
        assert_eq!(icon_key(&entry("")), "demo");
    }

    #[test]
    fn missing_icons_have_no_id() {
        let mut icons = AppIcons::new();
        let mut scene = Scene::new();
        assert!(
            icons
                .id_for(&entry("definitely-not-an-app-icon"), &mut scene)
                .is_none()
        );
    }
}
