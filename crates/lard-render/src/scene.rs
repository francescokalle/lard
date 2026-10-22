//! Retained draw list. The shell writes logical-pixel commands; the renderer
//! converts them to physical pixels and GPU instances exactly once.

use lard_core::{Color, Point, Rect};
use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontFamily {
    Sans,
    Mono,
    /// Monochrome symbol font (Material Icons).
    Icon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    pub family: FontFamily,
    pub size: f32,
    pub weight: u16,
    pub align: TextAlign,
    /// Width available for wrapping/alignment, in logical pixels.
    pub max_width: Option<f32>,
    /// If false, text is shaped on one line even when it exceeds max_width.
    /// Used by bar titles and other cells that must never grow vertically.
    pub wrap: bool,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            family: FontFamily::Sans,
            size: 13.0,
            weight: 400,
            align: TextAlign::Left,
            max_width: None,
            wrap: true,
        }
    }
}

impl TextStyle {
    #[must_use]
    pub fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    #[must_use]
    pub fn weight(mut self, weight: u16) -> Self {
        self.weight = weight;
        self
    }

    #[must_use]
    pub fn mono(mut self) -> Self {
        self.family = FontFamily::Mono;
        self
    }

    #[must_use]
    pub fn icon(mut self) -> Self {
        self.family = FontFamily::Icon;
        self
    }

    #[must_use]
    pub fn align(mut self, align: TextAlign) -> Self {
        self.align = align;
        self
    }

    #[must_use]
    pub fn width(mut self, width: f32) -> Self {
        self.max_width = Some(width);
        self
    }

    #[must_use]
    pub fn no_wrap(mut self) -> Self {
        self.wrap = false;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corners {
    pub tl: f32,
    pub tr: f32,
    pub br: f32,
    pub bl: f32,
}

impl Corners {
    #[must_use]
    pub const fn all(r: f32) -> Self {
        Self {
            tl: r,
            tr: r,
            br: r,
            bl: r,
        }
    }

    #[must_use]
    pub const fn new(tl: f32, tr: f32, br: f32, bl: f32) -> Self {
        Self { tl, tr, br, bl }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shadow {
    pub color: Color,
    /// Blur radius in logical pixels.
    pub blur: f32,
    pub offset_y: f32,
    /// Expands the shadow rectangle, useful for glows.
    pub spread: f32,
}

impl Shadow {
    #[must_use]
    pub fn soft(color: Color, blur: f32) -> Self {
        Self {
            color,
            blur,
            offset_y: 6.0,
            spread: 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RectCmd {
    pub rect: Rect,
    pub color: Color,
    pub corners: Corners,
    pub border_width: f32,
    pub border_color: Color,
    pub shadow: Option<Shadow>,
    /// Radial falloff blob (wallpaper glow) instead of a filled shape.
    pub glow: bool,
}

/// Position in the scene command buffers, see [`Scene::marker`].
#[derive(Clone, Copy, Debug)]
pub struct SceneMarker {
    rects: usize,
    texts: usize,
}

#[derive(Clone, Debug)]
pub struct TextCmd {
    /// Byte range into the scene's text arena.
    pub range: Range<usize>,
    pub origin: Point,
    pub style: TextStyle,
    pub color: Color,
}

/// An RGBA image (thumbnail) drawn as a rounded, sampled quad.
#[derive(Clone, Copy, Debug)]
pub struct ImageCmd {
    pub rect: Rect,
    pub id: u32,
    pub corners: Corners,
}

/// A decoded RGBA image to (re)upload into the image atlas. The renderer
/// deduplicates by `id`, so a view may push the same data every frame.
#[derive(Clone, Debug)]
pub struct ImageUpload {
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A frame's draw commands. Reusing one `Scene` between frames keeps all
/// vectors allocated.
#[derive(Debug, Default)]
pub struct Scene {
    pub rects: Vec<RectCmd>,
    pub texts: Vec<TextCmd>,
    pub images: Vec<ImageCmd>,
    pub image_uploads: Vec<ImageUpload>,
    /// Text is copied into one reusable arena instead of borrowing from UI
    /// state. This keeps `Scene` lifetime-free and makes steady-state frames
    /// allocation-free.
    arena: String,
}

impl Scene {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.rects.clear();
        self.texts.clear();
        self.images.clear();
        self.image_uploads.clear();
        self.arena.clear();
    }

    /// Marker of the current command counts, used to fade a group of
    /// commands (an island) without touching every one of them.
    #[must_use]
    pub fn marker(&self) -> SceneMarker {
        SceneMarker {
            rects: self.rects.len(),
            texts: self.texts.len(),
        }
    }

    /// Scale the alpha of every command added after `marker`. Used when a
    /// panel grows out of an island: the island fades away while it morphs.
    pub fn fade_since(&mut self, marker: SceneMarker, alpha: f32) {
        let alpha = alpha.clamp(0.0, 1.0);
        if alpha >= 0.999 {
            return;
        }
        for cmd in &mut self.rects[marker.rects..] {
            cmd.color = cmd.color.scale_alpha(alpha);
            cmd.border_color = cmd.border_color.scale_alpha(alpha);
            if let Some(shadow) = &mut cmd.shadow {
                shadow.color = shadow.color.scale_alpha(alpha);
            }
        }
        for cmd in &mut self.texts[marker.texts..] {
            cmd.color = cmd.color.scale_alpha(alpha);
        }
    }

    #[must_use]
    pub fn text_of(&self, cmd: &TextCmd) -> &str {
        &self.arena[cmd.range.clone()]
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rects.is_empty() && self.texts.is_empty()
    }

    pub fn rect(&mut self, rect: Rect, color: Color, radius: f32) {
        self.rects.push(RectCmd {
            rect,
            color,
            corners: Corners::all(radius),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            shadow: None,
            glow: false,
        });
    }

    pub fn rect_corners(&mut self, rect: Rect, color: Color, corners: Corners) {
        self.rects.push(RectCmd {
            rect,
            color,
            corners,
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            shadow: None,
            glow: false,
        });
    }

    pub fn bordered(
        &mut self,
        rect: Rect,
        color: Color,
        radius: f32,
        border_width: f32,
        border_color: Color,
    ) {
        self.rects.push(RectCmd {
            rect,
            color,
            corners: Corners::all(radius),
            border_width,
            border_color,
            shadow: None,
            glow: false,
        });
    }

    pub fn shadow(&mut self, rect: Rect, radius: f32, shadow: Shadow) {
        self.rects.push(RectCmd {
            rect,
            color: Color::TRANSPARENT,
            corners: Corners::all(radius),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            shadow: Some(shadow),
            glow: false,
        });
    }

    /// Draw a rounded image quad referencing a previously uploaded image.
    pub fn image(&mut self, rect: Rect, id: u32, radius: f32) {
        self.images.push(ImageCmd {
            rect,
            id,
            corners: Corners::all(radius),
        });
    }

    /// Queue an RGBA image for upload into the atlas.
    pub fn upload_image(&mut self, id: u32, width: u32, height: u32, rgba: Vec<u8>) {
        self.image_uploads.push(ImageUpload {
            id,
            width,
            height,
            rgba,
        });
    }

    /// A rounded panel with an optional soft shadow.
    pub fn panel(&mut self, rect: Rect, color: Color, radius: f32, shadow: Option<Shadow>) {
        if let Some(shadow) = shadow {
            self.rect(rect, color, radius);
            // Replace with a shadow-only command, then keep the panel on top.
            if let Some(cmd) = self.rects.last_mut() {
                cmd.color = Color::TRANSPARENT;
                cmd.shadow = Some(shadow);
            }
        }
        if color.a > 0.0 {
            self.rect(rect, color, radius);
        }
    }

    /// Soft elliptical blob used for wallpaper glows.
    ///
    /// Unlike `shadow`, the shader uses a radial falloff inside the rectangle,
    /// so a large low-alpha glow does not appear as a translucent block.
    /// `blur` is kept for API compatibility; the falloff is normalized to the
    /// rectangle.
    pub fn glow(&mut self, rect: Rect, color: Color, _blur: f32) {
        self.rects.push(RectCmd {
            rect,
            color,
            corners: Corners::all(0.0),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
            shadow: None,
            glow: true,
        });
    }

    /// Fake a smooth vertical gradient with a handful of 1-px bands.
    ///
    /// Cheap (one instanced draw call for all bands) and avoids a third
    /// pipeline for the greeter wallpaper.
    pub fn vertical_gradient(&mut self, rect: Rect, top: Color, bottom: Color, bands: u32) {
        let bands = bands.max(1);
        let band_h = rect.h / bands as f32;
        for i in 0..bands {
            let t = if bands == 1 {
                0.0
            } else {
                i as f32 / (bands - 1) as f32
            };
            self.rect(
                Rect::new(rect.x, rect.y + i as f32 * band_h, rect.w, band_h + 0.5),
                top.mix(bottom, t),
                0.0,
            );
        }
    }

    pub fn text(&mut self, text: impl AsRef<str>, origin: Point, style: TextStyle, color: Color) {
        let start = self.arena.len();
        self.arena.push_str(text.as_ref());
        let end = self.arena.len();
        self.texts.push(TextCmd {
            range: start..end,
            origin,
            style,
            color,
        });
    }

    /// Draw text optically centered vertically at `y` (the y is the center,
    /// not the top). `x` is the left edge of the alignment box.
    pub fn text_center_y(
        &mut self,
        text: impl AsRef<str>,
        x: f32,
        y: f32,
        style: TextStyle,
        color: Color,
    ) {
        let top = y - style.size * 0.62;
        self.text(text, Point::new(x, top), style, color);
    }
}
