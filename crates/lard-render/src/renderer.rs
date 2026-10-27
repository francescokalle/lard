use crate::scene::{Fluid, FluidMode, Scene, TextAlign};
use crate::surface::RenderSurface;
use crate::text::physical_style;
use crate::text::{TextEngine, align_offset};
use anyhow::Result;
use bytemuck::{Pod, Zeroable};
use lard_core::Point;
use lard_core::theme::FontConfig;
use std::collections::HashMap;

const RECT_ATTRS: [wgpu::VertexAttribute; 5] = wgpu::vertex_attr_array![
    0 => Float32x4,
    1 => Float32x4,
    2 => Float32x4,
    3 => Float32x4,
    4 => Float32x4,
];

const TEXT_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];

const IMAGE_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4];

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct RectGpu {
    rect: [f32; 4],
    color: [f32; 4],
    radii: [f32; 4],
    params: [f32; 4],
    border_color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct TextGpu {
    rect: [f32; 4],
    uv: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct ImageGpu {
    rect: [f32; 4],
    uv: [f32; 4],
    radii: [f32; 4],
    alpha: [f32; 4],
}

/// Uniform block of the fluid shader. Field order must match `fluid.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct FluidGpu {
    screen: [f32; 2],
    time: f32,
    mode: u32,
    /// seed_a, seed_b, crossfade, reveal level
    variants: [f32; 4],
    /// mode_a, mode_b, unused, unused
    modes: [u32; 4],
    deep: [f32; 4],
    accent: [f32; 4],
    light: [f32; 4],
    params: [f32; 4],
    /// dust, unused, unused, unused
    extra: [f32; 4],
}

struct Pipelines {
    rect: wgpu::RenderPipeline,
    text: wgpu::RenderPipeline,
    image: wgpu::RenderPipeline,
    /// Full-screen fluid, drawn straight into the output.
    fluid: wgpu::RenderPipeline,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    pub presented: bool,
    pub rect_instances: u32,
    pub glyph_instances: u32,
    pub draw_calls: u32,
    pub uploaded_bytes: u64,
}

/// Atlas cells are square so they can hold both wallpapers (128x72, drawn
/// in the top-left of the cell) and square app icons.
const IMAGE_CELL_W: u32 = 128;
const IMAGE_CELL_H: u32 = 128;
const IMAGE_COLS: u32 = 16;
const IMAGE_ROWS: u32 = 16;
const IMAGE_ATLAS_W: u32 = IMAGE_CELL_W * IMAGE_COLS;
const IMAGE_ATLAS_H: u32 = IMAGE_CELL_H * IMAGE_ROWS;

struct ImageAtlas {
    _texture: wgpu::Texture,
    _view: wgpu::TextureView,
    _sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
    /// Uploaded images with their pixel size, so the sampled sub-rect matches
    /// the real bitmap inside the cell.
    uploaded: std::collections::HashMap<u32, (u32, u32)>,
}

impl ImageAtlas {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("image-atlas"),
            size: wgpu::Extent3d {
                width: IMAGE_ATLAS_W,
                height: IMAGE_ATLAS_H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("image-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("image-bind-group"),
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
            _view: view,
            _sampler: sampler,
            bind_group,
            uploaded: std::collections::HashMap::new(),
        }
    }

    fn upload(&mut self, queue: &wgpu::Queue, id: u32, width: u32, height: u32, rgba: &[u8]) {
        if self.uploaded.contains_key(&id) {
            return;
        }
        let cell = (id as usize) % (IMAGE_COLS * IMAGE_ROWS) as usize;
        let col = (cell % IMAGE_COLS as usize) as u32;
        let row = (cell / IMAGE_COLS as usize) as u32;
        let w = width.min(IMAGE_CELL_W);
        let h = height.min(IMAGE_CELL_H);
        let bytes_per_row = width.min(IMAGE_CELL_W) * 4;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self._texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: col * IMAGE_CELL_W,
                    y: row * IMAGE_CELL_H,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &rgba[..(bytes_per_row as usize * h as usize).min(rgba.len())],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.uploaded.insert(id, (w, h));
    }

    fn has(&self, id: u32) -> bool {
        self.uploaded.contains_key(&id)
    }

    fn uv(&self, id: u32) -> [f32; 4] {
        let cell = (id as usize) % (IMAGE_COLS * IMAGE_ROWS) as usize;
        let col = (cell % IMAGE_COLS as usize) as u32;
        let row = (cell / IMAGE_COLS as usize) as u32;
        // Only the part of the cell that actually holds the bitmap is sampled,
        // so a 64x64 icon and a 128x72 wallpaper both keep their aspect.
        let (w, h) = self
            .uploaded
            .get(&id)
            .copied()
            .unwrap_or((IMAGE_CELL_W, IMAGE_CELL_H));
        let x = col as f32 * IMAGE_CELL_W as f32;
        let y = row as f32 * IMAGE_CELL_H as f32;
        let u0 = x / IMAGE_ATLAS_W as f32;
        let v0 = y / IMAGE_ATLAS_H as f32;
        let u1 = (x + w as f32) / IMAGE_ATLAS_W as f32;
        let v1 = (y + h as f32) / IMAGE_ATLAS_H as f32;
        // Half-texel inset so linear filtering never bleeds between cells.
        let iu = 0.5 / IMAGE_ATLAS_W as f32;
        let iv = 0.5 / IMAGE_ATLAS_H as f32;
        [u0 + iu, v0 + iv, u1 - iu, v1 - iv]
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    globals_buffer: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    rect_pipeline_layout: wgpu::PipelineLayout,
    text_pipeline_layout: wgpu::PipelineLayout,
    rect_shader: wgpu::ShaderModule,
    text_shader: wgpu::ShaderModule,
    image_shader: wgpu::ShaderModule,
    fluid_shader: wgpu::ShaderModule,
    /// Texture + filtering sampler layout, shared by the text and image atlases.
    atlas_layout: wgpu::BindGroupLayout,
    image_pipeline_layout: wgpu::PipelineLayout,
    fluid_pipeline_layout: wgpu::PipelineLayout,
    fluid_buffer: wgpu::Buffer,
    fluid_bind_group: wgpu::BindGroup,
    /// Parameters of the fluid in the frame being prepared.
    fluid_frame: Option<Fluid>,
    pipelines: HashMap<wgpu::TextureFormat, Pipelines>,
    text: TextEngine,
    image_atlas: ImageAtlas,
    rects: Vec<RectGpu>,
    glyphs: Vec<TextGpu>,
    images: Vec<ImageGpu>,
    rect_buffer: wgpu::Buffer,
    rect_capacity: u64,
    text_buffer: wgpu::Buffer,
    text_capacity: u64,
    image_buffer: wgpu::Buffer,
    image_capacity: u64,
}

impl Renderer {
    pub fn new(gpu: &crate::Gpu, fonts: &FontConfig) -> Result<Self> {
        let device = gpu.device();
        let queue = gpu.queue();

        let globals_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals-bind-group"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals_buffer.as_entire_binding(),
            }],
        });

        let atlas_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("atlas-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let rect_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rect-pipeline-layout"),
            bind_group_layouts: &[Some(&globals_layout)],
            immediate_size: 0,
        });

        let text_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text-pipeline-layout"),
            bind_group_layouts: &[Some(&globals_layout), Some(&atlas_layout)],
            immediate_size: 0,
        });

        // Image atlas (RGBA thumbnails) shares the atlas bind-group layout
        // with the text pipeline (texture + sampler).
        let image_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("image-pipeline-layout"),
                bind_group_layouts: &[Some(&globals_layout), Some(&atlas_layout)],
                immediate_size: 0,
            });

        // The fluid pass owns a single uniform (its own group 0) and the
        // upscale pass only needs the texture+sampler pair.
        let fluid_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fluid-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<FluidGpu>() as u64
                    ),
                },
                count: None,
            }],
        });
        let fluid_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fluid-uniform"),
            size: std::mem::size_of::<FluidGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let fluid_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fluid-bind-group"),
            layout: &fluid_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: fluid_buffer.as_entire_binding(),
            }],
        });
        let fluid_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("fluid-pipeline-layout"),
                bind_group_layouts: &[Some(&fluid_layout)],
                immediate_size: 0,
            });

        let rect_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rect-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/rect.wgsl").into()),
        });
        let text_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("text-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/text.wgsl").into()),
        });
        let image_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/image.wgsl").into()),
        });
        let fluid_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fluid-shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/fluid.wgsl").into()),
        });

        let text = TextEngine::new(device, queue, fonts, &atlas_layout);
        let image_atlas = ImageAtlas::new(device, &atlas_layout);

        let mut renderer = Self {
            device: device.clone(),
            queue: queue.clone(),
            globals_buffer,
            globals_bind_group,
            rect_pipeline_layout,
            text_pipeline_layout,
            rect_shader,
            text_shader,
            image_shader,
            fluid_shader,
            atlas_layout: atlas_layout.clone(),
            image_pipeline_layout,
            fluid_pipeline_layout,
            fluid_buffer,
            fluid_bind_group,
            fluid_frame: None,
            pipelines: HashMap::with_capacity(2),
            text,
            image_atlas,
            rects: Vec::with_capacity(64),
            glyphs: Vec::with_capacity(256),
            images: Vec::with_capacity(16),
            rect_buffer: gpu.create_vertex_buffer("rect-instances", 4),
            rect_capacity: 4,
            text_buffer: gpu.create_vertex_buffer("text-instances", 4),
            text_capacity: 4,
            image_buffer: gpu.create_vertex_buffer("image-instances", 4),
            image_capacity: 4,
        };
        // Compile the WGSL and build pipelines once at startup, for the two
        // sRGB formats Wayland compositors actually advertise. A shader error
        // then fails immediately instead of on the first frame.
        renderer.ensure_pipelines(wgpu::TextureFormat::Bgra8UnormSrgb);
        renderer.ensure_pipelines(wgpu::TextureFormat::Rgba8UnormSrgb);
        Ok(renderer)
    }

    fn ensure_pipelines(&mut self, format: wgpu::TextureFormat) {
        if self.pipelines.contains_key(&format) {
            return;
        }
        let pipelines = create_pipelines(
            &self.device,
            format,
            &self.rect_pipeline_layout,
            &self.text_pipeline_layout,
            &self.image_pipeline_layout,
            &self.fluid_pipeline_layout,
            &self.rect_shader,
            &self.text_shader,
            &self.image_shader,
            &self.fluid_shader,
        );
        self.pipelines.insert(format, pipelines);
    }

    /// Draw a scene into a configured Wayland surface and present it.
    pub fn render(
        &mut self,
        gpu: &crate::Gpu,
        target: &mut RenderSurface,
        scene: &Scene,
    ) -> RenderStats {
        let mut stats = RenderStats::default();
        let Some(frame) = target.acquire(gpu) else {
            return stats;
        };

        let scale = target.scale();
        let screen = target.physical_size();
        if screen.0 == 0 || screen.1 == 0 {
            return stats;
        }

        stats = self.prepare(scene, scale, screen);
        let format = target.format();
        self.ensure_pipelines(format);
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        stats.draw_calls = self.paint_and_submit(&view, format);
        self.queue.present(frame);
        stats.presented = true;
        stats
    }

    /// Render into an arbitrary texture view. Used by the headless smoke test
    /// and future screenshot tooling.
    pub fn render_offscreen(
        &mut self,
        view: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        screen: (u32, u32),
        scale: f32,
        scene: &Scene,
    ) -> RenderStats {
        let mut stats = self.prepare(scene, scale, screen);
        self.ensure_pipelines(format);
        stats.draw_calls = self.paint_and_submit(view, format);
        stats
    }

    fn prepare(&mut self, scene: &Scene, scale: f32, screen: (u32, u32)) -> RenderStats {
        let mut stats = RenderStats::default();
        let (screen_w, screen_h) = screen;
        self.rects.clear();
        self.glyphs.clear();


        for cmd in &scene.rects {
            let rect = cmd.rect;
            if rect.w <= 0.0 || rect.h <= 0.0 {
                continue;
            }

            if cmd.glow {
                let physical = rect.scaled(scale);
                self.rects.push(RectGpu {
                    rect: [physical.x, physical.y, physical.w, physical.h],
                    color: cmd.color.linear_straight(),
                    radii: [0.0; 4],
                    params: [0.0, 0.0, 1.0, 2.0],
                    border_color: [0.0; 4],
                });
                continue;
            }

            if let Some(shadow) = cmd.shadow {
                let expand = shadow.spread.max(0.0);
                // The quad must include the soft falloff, otherwise the
                // shadow is cut off at its own edge and looks like a hard
                // ring (very visible around rounded corners).
                let pad = (shadow.blur.max(1.0) * 2.0).clamp(2.0, 96.0);
                let outer = expand + pad;
                let shadow_rect = lard_core::Rect::new(
                    (rect.x - outer) * scale,
                    (rect.y - outer + shadow.offset_y) * scale,
                    (rect.w + outer * 2.0) * scale,
                    (rect.h + outer * 2.0) * scale,
                );
                let color = shadow.color.linear_straight();
                self.rects.push(RectGpu {
                    rect: [shadow_rect.x, shadow_rect.y, shadow_rect.w, shadow_rect.h],
                    color,
                    // Grow the shadow radius with the spread so the halo
                    // keeps the panel's curvature instead of pinching into
                    // sharper corners when it expands.
                    radii: [
                        (cmd.corners.tl + expand) * scale,
                        (cmd.corners.tr + expand) * scale,
                        (cmd.corners.br + expand) * scale,
                        (cmd.corners.bl + expand) * scale,
                    ],
                    params: [pad * scale, (shadow.blur * scale).max(1.0), 1.0, 1.0],
                    border_color: [0.0; 4],
                });
            }

            if cmd.color.a <= 0.0 {
                continue;
            }

            let physical = rect.scaled(scale);
            self.rects.push(RectGpu {
                rect: [physical.x, physical.y, physical.w, physical.h],
                color: cmd.color.linear_straight(),
                radii: [
                    cmd.corners.tl * scale,
                    cmd.corners.tr * scale,
                    cmd.corners.br * scale,
                    cmd.corners.bl * scale,
                ],
                params: [cmd.border_width * scale, 0.0, 1.0, 0.0],
                border_color: cmd.border_color.linear_straight(),
            });
        }

        for cmd in &scene.texts {
            let (size, max_width) = physical_style(&cmd.style, scale);
            let layout = self.text.layout(
                scene.text_of(cmd),
                cmd.style.family,
                size,
                cmd.style.weight,
                max_width,
                cmd.style.wrap,
            );
            let box_width = cmd.style.max_width.map(|w| w * scale);
            let (dx, dy) = run_origin(cmd.origin, scale, cmd.style.align, box_width, layout.width);
            let color = cmd.color.linear_straight();

            for glyph in &layout.glyphs {
                let rect = [
                    dx + glyph.rect[0],
                    dy + glyph.rect[1],
                    glyph.rect[2],
                    glyph.rect[3],
                ];
                if rect[2] <= 0.0 || rect[3] <= 0.0 {
                    continue;
                }
                self.glyphs.push(TextGpu {
                    rect,
                    uv: glyph.uv,
                    color,
                });
            }
        }

        self.images.clear();
        for up in &scene.image_uploads {
            self.image_atlas
                .upload(&self.queue, up.id, up.width, up.height, &up.rgba);
        }

        self.fluid_frame = scene.fluid;
        if let Some(fluid) = self.fluid_frame {
            // Straight into the output, at full resolution: the screensaver is
            // mostly this shader, and an upscaled buffer looked visibly soft.
            self.write_fluid_uniform(&fluid, screen);
        }
        for cmd in &scene.images {
            if !self.image_atlas.has(cmd.id) {
                continue;
            }
            let physical = cmd.rect.scaled(scale);
            let uv = self.image_atlas.uv(cmd.id);
            self.images.push(ImageGpu {
                rect: [physical.x, physical.y, physical.w, physical.h],
                uv,
                radii: [
                    cmd.corners.tl * scale,
                    cmd.corners.tr * scale,
                    cmd.corners.br * scale,
                    cmd.corners.bl * scale,
                ],
                alpha: [1.0, 0.0, 0.0, 0.0],
            });
        }

        let globals = [screen_w as f32, screen_h as f32, 0.0, 0.0];
        self.queue
            .write_buffer(&self.globals_buffer, 0, bytemuck::cast_slice(&globals));

        let rect_bytes = bytemuck::cast_slice(&self.rects);
        if !rect_bytes.is_empty() {
            if rect_bytes.len() as u64 > self.rect_capacity {
                self.rect_capacity = (rect_bytes.len() as u64).next_power_of_two();
                self.rect_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("rect-instances"),
                    size: self.rect_capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            self.queue.write_buffer(&self.rect_buffer, 0, rect_bytes);
            stats.uploaded_bytes += rect_bytes.len() as u64;
        }

        let text_bytes = bytemuck::cast_slice(&self.glyphs);
        if !text_bytes.is_empty() {
            if text_bytes.len() as u64 > self.text_capacity {
                self.text_capacity = (text_bytes.len() as u64).next_power_of_two();
                self.text_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("text-instances"),
                    size: self.text_capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            self.queue.write_buffer(&self.text_buffer, 0, text_bytes);
            stats.uploaded_bytes += text_bytes.len() as u64;
        }

        let image_bytes = bytemuck::cast_slice(&self.images);
        if !image_bytes.is_empty() {
            if image_bytes.len() as u64 > self.image_capacity {
                self.image_capacity = (image_bytes.len() as u64).next_power_of_two();
                self.image_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("image-instances"),
                    size: self.image_capacity,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            self.queue.write_buffer(&self.image_buffer, 0, image_bytes);
            stats.uploaded_bytes += image_bytes.len() as u64;
        }

        stats.rect_instances = self.rects.len() as u32;
        stats.glyph_instances = self.glyphs.len() as u32;
        stats
    }

    fn write_fluid_uniform(&self, fluid: &Fluid, screen: (u32, u32)) {
        let deep = fluid.deep.linear_straight();
        let accent = fluid.accent.linear_straight();
        let light = fluid.light.linear_straight();
        let mode_of = |mode: FluidMode| match mode {
            FluidMode::Liquid => 0u32,
            FluidMode::Mercury => 1u32,
            FluidMode::Aurora => 2u32,
            FluidMode::Waves => 3u32,
            FluidMode::Grid => 4u32,
            FluidMode::Dust => 5u32,
            FluidMode::Gas => 6u32,
            FluidMode::Empty => 7u32,
        };
        let data = FluidGpu {
            screen: [screen.0 as f32, screen.1 as f32],
            time: fluid.time,
            mode: mode_of(fluid.mode),
            variants: [
                fluid.seed_a,
                fluid.seed_b,
                fluid.mix.clamp(0.0, 1.0),
                fluid.reveal.clamp(0.0, 1.25),
            ],
            modes: [mode_of(fluid.mode), mode_of(fluid.mode_b), 0, 0],
            deep,
            accent,
            light,
            params: [
                fluid.speed.clamp(0.05, 3.0),
                fluid.intensity.clamp(0.0, 1.0),
                fluid.fade.clamp(0.0, 1.0),
                fluid.veil.clamp(0.0, 0.95),
            ],
            extra: [fluid.dust.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
        };
        self.queue
            .write_buffer(&self.fluid_buffer, 0, bytemuck::bytes_of(&data));
    }

    fn paint_and_submit(&self, view: &wgpu::TextureView, format: wgpu::TextureFormat) -> u32 {
        let pipelines = &self.pipelines[&format];
        let mut draw_calls = 0;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame-encoder"),
            });

        {
            let color_attachment = Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            });

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui-pass"),
                color_attachments: &[color_attachment],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            // The fluid (screensaver) is the backdrop: drawn first, straight
            // into the output.
            if self.fluid_frame.is_some() {
                pass.set_pipeline(&pipelines.fluid);
                pass.set_bind_group(0, &self.fluid_bind_group, &[]);
                pass.draw(0..3, 0..1);
                draw_calls += 1;
            }

            if !self.rects.is_empty() {
                pass.set_pipeline(&pipelines.rect);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_vertex_buffer(0, self.rect_buffer.slice(..));
                pass.draw(0..6, 0..self.rects.len() as u32);
                draw_calls += 1;
            }

            if !self.images.is_empty() {
                pass.set_pipeline(&pipelines.image);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_bind_group(1, &self.image_atlas.bind_group, &[]);
                pass.set_vertex_buffer(0, self.image_buffer.slice(..));
                pass.draw(0..6, 0..self.images.len() as u32);
                draw_calls += 1;
            }
            if !self.glyphs.is_empty() {
                pass.set_pipeline(&pipelines.text);
                pass.set_bind_group(0, &self.globals_bind_group, &[]);
                pass.set_bind_group(1, self.text.atlas_bind_group(), &[]);
                pass.set_vertex_buffer(0, self.text_buffer.slice(..));
                pass.draw(0..6, 0..self.glyphs.len() as u32);
                draw_calls += 1;
            }
        }

        self.queue.submit(Some(encoder.finish()));
        draw_calls
    }
}

/// Top-left corner of a text run, in physical pixels.
///
/// Every glyph quad is a 1:1 copy of its coverage bitmap, so the run has to
/// start on the pixel grid. With a fractional origin the linear sampler reads
/// the outermost antialiased rows of the glyph halfway into the transparent
/// atlas padding, which visually shaves them off the top and bottom of the
/// text. Rounding the origin (alignment included) keeps the whole bitmap
/// inside its quad and renders the antialiasing at full strength.
#[must_use]
fn run_origin(
    origin: Point,
    scale: f32,
    align: TextAlign,
    box_width: Option<f32>,
    measured: f32,
) -> (f32, f32) {
    (
        (origin.x * scale + align_offset(align, box_width, measured)).round(),
        (origin.y * scale).round(),
    )
}

#[allow(clippy::too_many_arguments)]
fn create_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    rect_layout: &wgpu::PipelineLayout,
    text_layout: &wgpu::PipelineLayout,
    image_layout: &wgpu::PipelineLayout,
    fluid_layout: &wgpu::PipelineLayout,
    rect_shader: &wgpu::ShaderModule,
    text_shader: &wgpu::ShaderModule,
    image_shader: &wgpu::ShaderModule,
    fluid_shader: &wgpu::ShaderModule,
) -> Pipelines {
    let blend = Some(wgpu::BlendState {
        color: wgpu::BlendComponent::OVER,
        alpha: wgpu::BlendComponent::OVER,
    });

    let target = [Some(wgpu::ColorTargetState {
        format,
        blend,
        write_mask: wgpu::ColorWrites::ALL,
    })];

    let rect_vertex = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<RectGpu>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &RECT_ATTRS,
    };
    let text_vertex = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<TextGpu>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &TEXT_ATTRS,
    };

    let rect = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("rect-pipeline"),
        layout: Some(rect_layout),
        vertex: wgpu::VertexState {
            module: rect_shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(rect_vertex)],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: rect_shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &target,
        }),
        multiview_mask: None,
        cache: None,
    });

    let text = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("text-pipeline"),
        layout: Some(text_layout),
        vertex: wgpu::VertexState {
            module: text_shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(text_vertex)],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: text_shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &target,
        }),
        multiview_mask: None,
        cache: None,
    });

    let image_vertex = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<ImageGpu>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &IMAGE_ATTRS,
    };
    let image = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("image-pipeline"),
        layout: Some(image_layout),
        vertex: wgpu::VertexState {
            module: image_shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(image_vertex)],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: image_shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &target,
        }),
        multiview_mask: None,
        cache: None,
    });

    let fluid = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("fluid-pipeline"),
        layout: Some(fluid_layout),
        vertex: wgpu::VertexState {
            module: fluid_shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: fluid_shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &target,
        }),
        multiview_mask: None,
        cache: None,
    });

    Pipelines {
        rect,
        text,
        image,
        fluid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_runs_start_on_whole_pixels() {
        // The bar title is centred at y = 19 with size_small: the layout top
        // lands on 12.18, and drawing it there shaved the glyph edges.
        assert_eq!(
            run_origin(Point::new(10.3, 12.18), 1.0, TextAlign::Left, None, 120.0),
            (10.0, 12.0)
        );
        // A half-pixel offset is the worst case: pick the nearest row.
        assert_eq!(
            run_origin(Point::new(0.0, 11.5), 1.0, TextAlign::Left, None, 10.0),
            (0.0, 12.0)
        );
        // Centring is applied before snapping, so the glyphs stay inside the box.
        let (x, _) = run_origin(Point::new(0.0, 0.0), 1.0, TextAlign::Center, Some(101.0), 100.0);
        assert_eq!(x, 1.0);
        // Fractional scales snap in physical pixels too.
        let (x, y) = run_origin(Point::new(10.0, 10.0), 1.5, TextAlign::Left, None, 10.0);
        assert_eq!((x, y), (15.0, 15.0));
    }
}
