//! Headless smoke test for device creation, WGSL compilation and font loading.
//! Run with: cargo run -p lard-render --example headless_smoke

use lard_core::theme::FontConfig;
use lard_core::{Color, Point, Rect};
use lard_render::scene::FontFamily;
use lard_render::text::TextEngine;
use lard_render::{Gpu, Renderer, Scene, TextStyle};

fn main() -> anyhow::Result<()> {
    let gpu = Gpu::new(None)?;
    let info = gpu.adapter().get_info();
    println!("adapter: {} ({:?})", info.name, info.backend);

    let fonts = FontConfig::default();
    let mut renderer = Renderer::new(&gpu, &fonts)?;
    println!("pipelines compiled (BGRA/RGBA sRGB)");

    // Exercise shaping, rasterization and the atlas upload path too.
    let layout = gpu
        .device()
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("smoke-atlas-layout"),
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
    let mut text = TextEngine::new(gpu.device(), gpu.queue(), &fonts, &layout);
    let cached = text.layout(
        "lard — 0123456789",
        FontFamily::Sans,
        28.0,
        600,
        800.0,
        true,
    );
    assert!(
        cached.glyphs.len() > 10,
        "expected rasterized glyphs, got {}",
        cached.glyphs.len()
    );
    println!(
        "text layout ok: {} glyphs, width {:.1}px",
        cached.glyphs.len(),
        cached.width
    );

    // Full offscreen pass: shapes + borders + shadow + glyphs, then read back.
    let size = (512u32, 512u32);
    let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("smoke-target"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

    let mut scene = Scene::new();
    scene.vertical_gradient(
        Rect::new(0.0, 0.0, 512.0, 512.0),
        Color::from_hex("#0B1020").unwrap(),
        Color::from_hex("#18243A").unwrap(),
        32,
    );
    scene.panel(
        Rect::new(48.0, 96.0, 416.0, 240.0),
        Color::from_hex("#14171CE8").unwrap(),
        24.0,
        Some(lard_render::Shadow {
            color: Color::from_hex("#00000088").unwrap(),
            blur: 28.0,
            offset_y: 10.0,
            spread: 2.0,
        }),
    );
    scene.bordered(
        Rect::new(48.0, 96.0, 416.0, 240.0),
        Color::TRANSPARENT,
        24.0,
        1.0,
        Color::from_hex("#FFFFFF22").unwrap(),
    );
    scene.rect(
        Rect::new(48.0, 96.0, 4.0, 240.0),
        Color::from_hex("#9BC1FF").unwrap(),
        2.0,
    );
    scene.text(
        "lard",
        Point::new(84.0, 124.0),
        TextStyle {
            size: 30.0,
            weight: 600,
            ..Default::default()
        },
        Color::WHITE,
    );
    scene.text(
        "headless wgpu smoke test — 0123456789",
        Point::new(84.0, 172.0),
        TextStyle {
            size: 16.0,
            ..Default::default()
        },
        Color::from_hex("#9AA4B2").unwrap(),
    );

    // Exercised the RGBA atlas + image pipeline (wallpaper thumbnails).
    let mut raster = vec![0u8; 128 * 72 * 4];
    for px in raster.chunks_exact_mut(4) {
        px.copy_from_slice(&[40, 220, 90, 255]);
    }
    scene.upload_image(0, 128, 72, raster);
    scene.image(Rect::new(84.0, 244.0, 128.0, 72.0), 0, 12.0);

    let stats = renderer.render_offscreen(
        &view,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        size,
        1.0,
        &scene,
    );
    assert!(stats.presented || stats.draw_calls == 3, "{stats:?}");

    let padded_row = 512u32 * 4;
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("smoke-readback"),
        size: u64::from(padded_row) * u64::from(size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("smoke-copy"),
        });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue().submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    gpu.device().poll(wgpu::PollType::wait_indefinitely())?;
    rx.recv().expect("map callback")?;
    let pixels = slice.get_mapped_range()?;
    let non_empty = pixels.chunks_exact(4).any(|px| px[3] > 0);
    let bright_text = pixels
        .chunks_exact(4)
        .any(|px| px[0] > 200 && px[1] > 200 && px[2] > 200);
    let green_image = pixels
        .chunks_exact(4)
        .any(|px| px[1] > 150 && px[0] < 120 && px[2] < 140);
    assert!(non_empty, "offscreen render produced an empty image");
    assert!(bright_text, "expected white glyph pixels in the readback");
    assert!(
        green_image,
        "expected sampled thumbnail pixels in the readback"
    );
    // Soft shadows must actually paint outside the panel. Before the quad was
    // padded, the falloff was clipped at the box edge, producing a hard ring.
    let pixel_sum = |x: u32, y: u32| {
        let i = (y * padded_row) as usize + x as usize * 4;
        i32::from(pixels[i]) + i32::from(pixels[i + 1]) + i32::from(pixels[i + 2])
    };
    let shadow = pixel_sum(256, 350);
    let background = pixel_sum(500, 350);
    assert!(
        shadow + 12 < background,
        "expected the shadow to darken pixels below the panel ({shadow} vs {background})"
    );
    println!(
        "offscreen render ok: {} shapes, {} glyphs, {} draw calls (thumbnail ok)",
        stats.rect_instances, stats.glyph_instances, stats.draw_calls
    );
    Ok(())
}
