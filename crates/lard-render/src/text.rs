//! Text shaping, glyph rasterization and a shelf-packed R8 atlas.
//!
//! Layouts are cached by a 64-bit FNV hash plus a full key comparison, so a
//! steady UI performs no shaping at all: only one hash and one string compare
//! per text command per frame. The atlas uploads each glyph exactly once.

use crate::scene::{FontFamily, TextAlign, TextStyle};
use cosmic_text::{
    Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache, SwashContent, Weight,
};
use fontdb::Source;
use lard_core::theme::FontConfig;
use std::collections::HashMap;
use std::sync::Arc;

const ATLAS_SIZE: u32 = 2048;
// Two transparent texels around every glyph keep linear sampling away from
// neighbouring bitmaps even when a glyph is drawn at a fractional position.
const PADDING: u32 = 2;
const MAX_CACHE_ENTRIES: usize = 512;

#[derive(Clone, Copy, Debug)]
pub struct GlyphQuad {
    /// x, y, w, h in physical pixels, relative to the text origin.
    pub rect: [f32; 4],
    /// Normalized atlas coordinates.
    pub uv: [f32; 4],
}

#[derive(Clone, Copy, Debug)]
struct CachedGlyph {
    uv: [f32; 4],
    left: i32,
    top: i32,
    width: u32,
    height: u32,
}

#[derive(Debug)]
pub struct CachedText {
    text: String,
    family: u8,
    size_bits: u32,
    weight: u16,
    width_bits: u32,
    wrap: bool,
    pub glyphs: Vec<GlyphQuad>,
    pub width: f32,
    pub height: f32,
}

impl CachedText {
    fn matches(
        &self,
        text: &str,
        family: u8,
        size_bits: u32,
        weight: u16,
        width_bits: u32,
        wrap: bool,
    ) -> bool {
        self.text == text
            && self.family == family
            && self.size_bits == size_bits
            && self.weight == weight
            && self.width_bits == width_bits
            && self.wrap == wrap
    }
}

pub struct TextEngine {
    font_system: FontSystem,
    swash: SwashCache,
    buffer: Buffer,
    cache: HashMap<u64, Vec<CachedText>>,
    cache_entries: usize,
    /// Rasterized glyphs are shared by every layout that uses them. The atlas
    /// is append-only, so inserting the same glyph for every clock tick or
    /// window title would eventually exhaust it.
    glyph_cache: HashMap<cosmic_text::CacheKey, CachedGlyph>,
    atlas: Atlas,
    sans: String,
    mono: String,
    icons: String,
}

impl TextEngine {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        fonts: &FontConfig,
        layout: &wgpu::BindGroupLayout,
    ) -> Self {
        let mut font_system = FontSystem::new();

        // Guarantee the intended typography even on minimal systems, while
        // still loading system fonts for fallback scripts.
        for data in [
            include_bytes!("../../../assets/fonts/Inter-Regular.ttf").as_slice(),
            include_bytes!("../../../assets/fonts/Inter-Medium.ttf").as_slice(),
            include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf").as_slice(),
            include_bytes!("../../../assets/fonts/Inter-Bold.ttf").as_slice(),
            include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf").as_slice(),
            include_bytes!("../../../assets/fonts/JetBrainsMono-Medium.ttf").as_slice(),
            include_bytes!("../../../assets/fonts/JetBrainsMono-SemiBold.ttf").as_slice(),
            include_bytes!("../../../assets/fonts/MaterialIcons-Regular.ttf").as_slice(),
        ] {
            font_system
                .db_mut()
                .load_font_source(Source::Binary(Arc::new(data)));
        }
        font_system.db_mut().set_sans_serif_family(&fonts.sans);
        font_system.db_mut().set_monospace_family(&fonts.mono);

        let metrics = Metrics::new(fonts.size_body.max(6.0), fonts.size_body.max(6.0) * 1.3);
        let buffer = Buffer::new_empty(metrics);

        Self {
            font_system,
            swash: SwashCache::new(),
            buffer,
            cache: HashMap::with_capacity(64),
            cache_entries: 0,
            glyph_cache: HashMap::with_capacity(512),
            atlas: Atlas::new(device, queue, layout),
            sans: fonts.sans.clone(),
            mono: fonts.mono.clone(),
            icons: fonts.icons.clone(),
        }
    }

    pub fn atlas_bind_group(&self) -> &wgpu::BindGroup {
        self.atlas.bind_group()
    }

    /// Shape (or fetch from cache) a run of text at a *physical* font size.
    pub fn layout(
        &mut self,
        text: &str,
        family: FontFamily,
        size: f32,
        weight: u16,
        max_width: f32,
        wrap: bool,
    ) -> &CachedText {
        let size = size.max(4.0);
        let size_bits = size.to_bits();
        let width_bits = max_width.max(1.0).to_bits();
        let family_id = match family {
            FontFamily::Sans => 0,
            FontFamily::Mono => 1,
            FontFamily::Icon => 2,
        };
        let hash = hash_text(text, family_id, size_bits, weight, width_bits);

        let hit = self.cache.get(&hash).and_then(|bucket| {
            bucket
                .iter()
                .position(|e| e.matches(text, family_id, size_bits, weight, width_bits, wrap))
        });
        if let Some(index) = hit {
            return &self.cache[&hash][index];
        }

        // Bound memory for pathological text (e.g. a clock that changes every
        // second for hours). Clearing only drops layout metadata; the glyph
        // atlas remains valid and is reused.
        if self.cache_entries >= MAX_CACHE_ENTRIES {
            self.cache.clear();
            self.cache_entries = 0;
        }

        let entry = self.shape(text, family_id, size, weight, max_width, wrap);
        self.cache_entries += 1;
        self.cache.entry(hash).or_default().push(entry);
        self.cache
            .get(&hash)
            .and_then(|bucket| bucket.last())
            .expect("entry was just inserted")
    }

    fn shape(
        &mut self,
        text: &str,
        family_id: u8,
        size: f32,
        weight: u16,
        max_width: f32,
        wrap: bool,
    ) -> CachedText {
        let line_height = size * 1.3;
        self.buffer.set_metrics_and_size(
            Metrics::new(size, line_height),
            wrap.then_some(max_width),
            None,
        );
        let family = match family_id {
            0 => Family::Name(&self.sans),
            1 => Family::Name(&self.mono),
            _ => Family::Name(&self.icons),
        };
        let attrs = Attrs::new().family(family).weight(Weight(weight));
        self.buffer.set_text(text, &attrs, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(&mut self.font_system, false);

        let mut glyphs = Vec::with_capacity(text.len() / 2 + 4);
        let mut width: f32 = 0.0;
        let mut height: f32 = line_height;

        for run in self.buffer.layout_runs() {
            width = width.max(run.line_w);
            height = height.max(run.line_top + run.line_height);
            for glyph in run.glyphs {
                let physical = glyph.physical((0.0, run.line_y), 1.0);
                let key = physical.cache_key;
                let cached = self.glyph_cache.get(&key).copied();
                let glyph = if let Some(cached) = cached {
                    cached
                } else {
                    let Some(image) = self
                        .swash
                        .get_image(&mut self.font_system, physical.cache_key)
                        .as_ref()
                    else {
                        continue;
                    };
                    if image.placement.width == 0 || image.placement.height == 0 {
                        continue;
                    }
                    let data: std::borrow::Cow<'_, [u8]> = match image.content {
                        SwashContent::Mask => std::borrow::Cow::Borrowed(&image.data),
                        // The atlas is R8, but a monochrome tint of a color emoji
                        // is still far better than dropping the glyph: Wi-Fi SSIDs
                        // and window titles can legitimately contain emoji. Keep
                        // silhouettes visible while preserving lighter details.
                        SwashContent::Color => {
                            std::borrow::Cow::Owned(color_glyph_mask(image.data.as_slice()))
                        }
                        SwashContent::SubpixelMask => continue,
                    };
                    let Some(uv) =
                        self.atlas
                            .insert(image.placement.width, image.placement.height, &data)
                    else {
                        tracing::warn!("glyph atlas full; skipping glyph");
                        continue;
                    };
                    let glyph = CachedGlyph {
                        uv,
                        left: image.placement.left,
                        top: image.placement.top,
                        width: image.placement.width,
                        height: image.placement.height,
                    };
                    self.glyph_cache.insert(key, glyph);
                    glyph
                };
                glyphs.push(GlyphQuad {
                    rect: [
                        (physical.x + glyph.left) as f32,
                        (physical.y - glyph.top) as f32,
                        glyph.width as f32,
                        glyph.height as f32,
                    ],
                    uv: glyph.uv,
                });
            }
        }

        CachedText {
            text: text.to_owned(),
            family: family_id,
            size_bits: size.to_bits(),
            weight,
            width_bits: max_width.max(1.0).to_bits(),
            wrap,
            glyphs,
            width,
            height,
        }
    }
}

/// Alignment-aware horizontal offset for a cached layout.
#[must_use]
pub fn align_offset(align: TextAlign, max_width: Option<f32>, measured: f32) -> f32 {
    match align {
        TextAlign::Left => 0.0,
        TextAlign::Center => (max_width.unwrap_or(measured) - measured).max(0.0) * 0.5,
        TextAlign::Right => (max_width.unwrap_or(measured) - measured).max(0.0),
    }
}

/// Default max width used when a text command has no explicit box.
pub const UNBOUNDED_TEXT_WIDTH: f32 = 4096.0;

pub fn physical_style(style: &TextStyle, scale: f32) -> (f32, f32) {
    (
        (style.size * scale).max(4.0),
        style
            .max_width
            .map_or(UNBOUNDED_TEXT_WIDTH * scale, |w| w * scale),
    )
}

/// Collapse premultiplied RGBA color-glyph pixels into the single-channel
/// mask consumed by the R8 atlas. A plain alpha silhouette loses internal
/// detail (faces become circles), so mix in luminance with a floor that keeps
/// dark glyphs visible on the dark panels.
fn color_glyph_mask(data: &[u8]) -> Vec<u8> {
    debug_assert_eq!(data.len() % 4, 0);
    data.chunks_exact(4)
        .map(|pixel| {
            let luma =
                (u32::from(pixel[0]) * 299 + u32::from(pixel[1]) * 587 + u32::from(pixel[2]) * 114)
                    / 1000;
            let alpha = u32::from(pixel[3]);
            ((alpha * 89 + luma * 166) / 255) as u8
        })
        .collect()
}

fn hash_text(text: &str, family: u8, size_bits: u32, weight: u16, width_bits: u32) -> u64 {
    // FNV-1a: fast on short UI strings, and does not need a RandomState.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    for word in [
        u64::from(family),
        u64::from(size_bits),
        u64::from(weight),
        u64::from(width_bits),
    ] {
        hash ^= word;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

struct Shelf {
    y: u32,
    x: u32,
    /// Vertical space reserved for this shelf. A shelf starts with the height
    /// of its first glyph; taller glyphs must go on a new shelf, otherwise
    /// they would overwrite the shelf below.
    height: u32,
}

struct Atlas {
    // Kept alive for the lifetime of the bind group.
    _texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    queue: wgpu::Queue,
    shelves: Vec<Shelf>,
    next_y: u32,
}

impl Atlas {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout) -> Self {
        let size = wgpu::Extent3d {
            width: ATLAS_SIZE,
            height: ATLAS_SIZE,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph-atlas"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("glyph-bind-group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        Self {
            _texture: texture,
            bind_group,
            queue: queue.clone(),
            shelves: Vec::new(),
            next_y: 0,
        }
    }

    fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Allocate a padded box. Returns its top-left corner.
    fn allocate(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        let boxed_w = width + PADDING * 2;
        let boxed_h = height + PADDING * 2;
        if boxed_w > ATLAS_SIZE || boxed_h > ATLAS_SIZE {
            return None;
        }

        if let Some(shelf) = self.shelves.last_mut()
            && boxed_h <= shelf.height
            && shelf.x + boxed_w <= ATLAS_SIZE
        {
            let x = shelf.x;
            shelf.x += boxed_w;
            return Some((x, shelf.y));
        }

        if self.next_y + boxed_h > ATLAS_SIZE {
            return None;
        }
        let y = self.next_y;
        self.next_y += boxed_h;
        self.shelves.push(Shelf {
            y,
            x: boxed_w,
            height: boxed_h,
        });
        Some((0, y))
    }

    fn insert(&mut self, width: u32, height: u32, data: &[u8]) -> Option<[f32; 4]> {
        debug_assert_eq!(data.len(), (width * height) as usize);
        let (x, y) = self.allocate(width, height)?;
        let px = x + PADDING;
        let py = y + PADDING;

        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self._texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: px, y: py, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        let u0 = px as f32 / ATLAS_SIZE as f32;
        let v0 = py as f32 / ATLAS_SIZE as f32;
        let u1 = (px + width) as f32 / ATLAS_SIZE as f32;
        let v1 = (py + height) as f32 / ATLAS_SIZE as f32;
        Some([u0, v0, u1, v1])
    }
}

/// Advance widths of the printable ASCII range of the bundled UI font,
/// expressed for a 100px font. The shell uses them to size panels around
/// their labels: a rough per-character guess cuts text that would actually
/// fit, while the real advances keep the ellipsis for the text that really
/// does not fit.
///
/// The table is measured once, on first use, with the same font the renderer
/// ships with.
pub fn sans_advances() -> &'static [f32; 95] {
    static ADVANCES: std::sync::OnceLock<[f32; 95]> = std::sync::OnceLock::new();
    ADVANCES.get_or_init(|| {
        let mut font_system = FontSystem::new();
        font_system
            .db_mut()
            .load_font_source(Source::Binary(Arc::new(
                include_bytes!("../../../assets/fonts/Inter-Regular.ttf").as_slice(),
            )));
        let mut buffer = Buffer::new_empty(Metrics::new(100.0, 130.0));
        let attrs = Attrs::new().family(Family::Name("Inter"));
        let mut advances = [0.0f32; 95];
        for (index, slot) in advances.iter_mut().enumerate() {
            let ch = char::from_u32(32 + index as u32).unwrap_or(' ');
            buffer.set_text(&ch.to_string(), &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut font_system, false);
            *slot = buffer
                .layout_runs()
                .map(|run| run.line_w)
                .fold(0.0f32, f32::max);
        }
        advances
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_glyphs_keep_a_visible_silhouette() {
        let rgba = [
            255, 255, 255, 255, // white opaque
            0, 0, 0, 255, // black opaque: still visible
            0, 0, 0, 0, // transparent
        ];
        assert_eq!(color_glyph_mask(&rgba), vec![255, 89, 0]);
    }

    #[test]
    fn ascii_advances_match_the_real_font() {
        let advances = sans_advances();
        // A space is narrower than an "M" and both are narrower than a "W".
        assert!(advances[0] < advances['M' as usize - 32]);
        assert!(advances['M' as usize - 32] < advances['W' as usize - 32]);
        // The table is expressed for a 100px font.
        assert!((advances['W' as usize - 32] - 94.0).abs() < 12.0);
    }
}
