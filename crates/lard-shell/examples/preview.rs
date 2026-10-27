//! Render shell views offscreen to PNG. Useful without a Wayland compositor:
//!   cargo run -p lard-shell --example preview

use lard_core::anim::SpringConfig;
use lard_core::desktop::DesktopEntry;
use lard_core::theme::{GreeterConfig, Theme};
use lard_core::{Color, Config, Point, Rect, Size};
use lard_greeter::AuthMode;
use lard_greeter::ui::GreeterView;
use lard_render::{Gpu, Renderer, Scene};
use lard_services::battery::BatteryState;
use lard_services::bluetooth::BluetoothState;
use lard_services::network::NetworkState;
use lard_services::niri::{NiriSnapshot, WindowView, WorkspaceView};
use lard_services::notifications::Notification;
use lard_shell::Shell;
use lard_shell::launcher::Launcher;
use lard_shell::osd::Osd;
use lard_shell::popover::PopoverKind;
use lard_shell::toasts::ToastStack;
use std::fs::File;
use std::io::BufWriter;

const W: u32 = 1280;
const H: u32 = 800;

fn main() -> anyhow::Result<()> {
    let gpu = Gpu::new(None)?;
    let mut config = Config::default();
    config.bar.show_bluetooth = true;
    config.bar.show_volume = true;
    std::fs::create_dir_all("preview")?;
    let mut renderer = Renderer::new(&gpu, &config.font)?;

    // ---- bar ----
    let mut shell = Shell::new(config.clone());
    shell.niri = demo_niri();
    shell.volume = 64;
    shell.battery = BatteryState {
        present: true,
        percent: 82,
        charging: true,
        external_power: true,
    };
    shell.network = NetworkState::Connected {
        ssid: Some("HomeNet".into()),
    };
    shell.bluetooth = BluetoothState {
        present: true,
        powered: true,
        discovering: false,
    };
    shell.clock = "14:32".into();
    shell.date = "mar 20 ott".into();
    shell.system = lard_services::system::SystemStats {
        cpu_percent: 25,
        memory_percent: 27,
    };
    let mut scene = Scene::new();
    wallpaper(&mut scene, 1280.0, 48.0, &config.theme, 12);
    shell.build_bar(&mut scene, "eDP-1", Size::new(1280.0, config.bar.height));
    save(&gpu, &mut renderer, "bar", 1280, 48, &scene)?;

    // ---- bar with the volume HUD in the centre island ----
    // The first event only arms the "seen" flag, the second one raises the HUD.
    shell.apply(lard_services::ServiceEvent::Volume {
        percent: 64,
        muted: false,
    });
    shell.apply(lard_services::ServiceEvent::Volume {
        percent: 50,
        muted: false,
    });
    shell.apply(lard_services::ServiceEvent::Volume {
        percent: 42,
        muted: false,
    });
    for _ in 0..24 {
        shell.update(1.0 / 60.0);
    }
    let mut scene = Scene::new();
    wallpaper(&mut scene, 1280.0, 48.0, &config.theme, 12);
    shell.build_bar(&mut scene, "eDP-1", Size::new(1280.0, config.bar.height));
    save(&gpu, &mut renderer, "bar-osd", 1280, 48, &scene)?;

    // ---- grouped Control Center popover ----
    let mut shell = Shell::new(config.clone());
    shell.niri = demo_niri();
    shell.volume = 64;
    shell.battery = BatteryState {
        present: true,
        percent: 82,
        charging: false,
        external_power: true,
    };
    shell.network = NetworkState::Connected {
        ssid: Some("HomeNet".into()),
    };
    shell.bluetooth = BluetoothState {
        present: true,
        powered: true,
        discovering: false,
    };
    shell.clock = "14:32".into();
    shell.date = "mar 20 ott".into();
    shell.brightness = 72;
    shell.brightness_present = true;
    shell.system = lard_services::system::SystemStats {
        cpu_percent: 25,
        memory_percent: 27,
    };
    let anchor = 1255.0;
    shell.open_popover(PopoverKind::ControlCenter, anchor, None);
    for _ in 0..40 {
        shell.update(1.0 / 60.0);
    }
    let card_w = shell.popover_card_size().map_or(360.0, |size| size.w);
    let left = (anchor - card_w * 0.5).clamp(
        config.bar.margin,
        (1280.0 - config.bar.margin - card_w).max(config.bar.margin),
    );
    let mut scene = Scene::new();
    wallpaper(&mut scene, W as f32, H as f32, &config.theme, 48);
    shell.build_popover(&mut scene, Size::new(W as f32, H as f32), left);
    save(&gpu, &mut renderer, "control-popover", W, H, &scene)?;

    let footer_y = config.bar.height + config.bar.margin + 6.0 + 500.0 - 14.0 - 17.0;
    shell.popover_press(Point::new(left + 250.0, footer_y));
    for _ in 0..8 {
        shell.update(1.0 / 60.0);
    }
    let mut scene = Scene::new();
    wallpaper(&mut scene, W as f32, H as f32, &config.theme, 48);
    shell.build_popover(&mut scene, Size::new(W as f32, H as f32), left);
    save(&gpu, &mut renderer, "control-popover-power", W, H, &scene)?;

    // ---- dock ----
    shell.update(1.0 / 60.0);
    if let Some(size) = shell.dock_size() {
        let mut scene = Scene::new();
        wallpaper(&mut scene, size.w + 80.0, size.h + 40.0, &config.theme, 12);
        let ox = 40.0;
        let oy = 20.0;
        scene.text(
            "",
            Point::new(0.0, 0.0),
            Default::default(),
            Color::TRANSPARENT,
        );
        // Draw a small wallpaper backdrop then translate by nesting the dock
        // scene via a save offset is not supported, so render at 0,0.
        shell.build_dock(&mut scene, size);
        let _ = (ox, oy);
        save(
            &gpu,
            &mut renderer,
            "dock",
            size.w.ceil() as u32,
            size.h.ceil() as u32,
            &scene,
        )?;
    }

    // ---- launcher ----
    let apps = demo_apps();
    let mut launcher = Launcher::new(SpringConfig::SNAPPY, false);
    launcher.open(&apps, 8);
    for _ in 0..40 {
        launcher.update(1.0 / 60.0);
    }
    launcher.pointer_move(Point::new(640.0, 300.0));
    let mut scene = Scene::new();
    wallpaper(&mut scene, W as f32, H as f32, &config.theme, 48);
    let mut icons = lard_shell::app_icons::AppIcons::new();
    launcher.build(
        &mut scene,
        Size::new(W as f32, H as f32),
        &apps,
        &config,
        &mut icons,
    );
    save(&gpu, &mut renderer, "launcher", W, H, &scene)?;

    // ---- control center ----
    let mut shell = Shell::new(config.clone());
    shell.niri = demo_niri();
    shell.volume = 64;
    shell.battery = BatteryState {
        present: true,
        percent: 82,
        charging: false,
        external_power: true,
    };
    shell.network = NetworkState::Connected {
        ssid: Some("HomeNet".into()),
    };
    shell.bluetooth = BluetoothState {
        present: true,
        powered: true,
        discovering: false,
    };
    shell.clock = "14:32".into();
    shell.date = "mar 20 ott".into();
    shell.brightness = 72;
    shell.brightness_present = true;
    shell.toggle_control();
    for _ in 0..40 {
        shell.update(1.0 / 60.0);
    }
    shell.control_move(Point::new(1100.0, 200.0));
    let mut scene = Scene::new();
    wallpaper(&mut scene, W as f32, H as f32, &config.theme, 48);
    shell.build_control(&mut scene, Size::new(W as f32, H as f32));
    save(&gpu, &mut renderer, "control", W, H, &scene)?;

    // ---- settings center ----
    let mut shell = Shell::new(config.clone());
    shell.open_settings();
    for _ in 0..40 {
        shell.update(1.0 / 60.0);
    }
    let mut scene = Scene::new();
    wallpaper(&mut scene, W as f32, H as f32, &config.theme, 48);
    shell.build_settings(&mut scene, Size::new(W as f32, H as f32));
    save(&gpu, &mut renderer, "settings", W, H, &scene)?;

    // Settings sub-tabs (panel is 1040x680 centered at 1280x800, sidebar
    // tabs start at panel.y + 80 with a 42 px pitch).
    for (name, y) in [
        ("settings-bar", 182.0),
        ("settings-dock", 224.0),
        ("settings-effects", 266.0),
        ("settings-screen", 308.0),
        ("settings-lock", 350.0),
        ("settings-niri", 392.0),
        ("settings-info", 434.0),
    ] {
        shell.settings_press(Point::new(300.0, y));
        for _ in 0..4 {
            shell.update(1.0 / 60.0);
        }
        let mut scene = Scene::new();
        wallpaper(&mut scene, W as f32, H as f32, &config.theme, 48);
        shell.build_settings(&mut scene, Size::new(W as f32, H as f32));
        save(&gpu, &mut renderer, name, W, H, &scene)?;
    }

    // ---- OSD ----
    let mut osd = Osd::new(SpringConfig::SNAPPY, false);
    osd.show_volume(64, false, &config.osd);
    for _ in 0..30 {
        osd.update(1.0 / 60.0);
    }
    let mut scene = Scene::new();
    wallpaper(&mut scene, 248.0, 56.0, &config.theme, 12);
    osd.build(
        &mut scene,
        Size::new(248.0, 56.0),
        &config.osd,
        &config.theme,
        &config.font,
    );
    save(&gpu, &mut renderer, "osd", 248, 56, &scene)?;

    let mut osd = Osd::new(SpringConfig::SNAPPY, false);
    osd.show_brightness(72, &config.osd);
    for _ in 0..30 {
        osd.update(1.0 / 60.0);
    }
    let mut scene = Scene::new();
    wallpaper(&mut scene, 248.0, 56.0, &config.theme, 12);
    osd.build(
        &mut scene,
        Size::new(248.0, 56.0),
        &config.osd,
        &config.theme,
        &config.font,
    );
    save(&gpu, &mut renderer, "osd-brightness", 248, 56, &scene)?;

    // ---- toasts ----
    let mut toasts = ToastStack::new(config.animation.notification, false);
    toasts.push(
        Notification {
            id: 1,
            app_name: "System".into(),
            app_icon: String::new(),
            summary: "Aggiornamenti disponibili".into(),
            body: "3 pacchetti pronti per l'installazione".into(),
            actions: vec![lard_services::notifications::NotificationAction {
                key: "update".into(),
                label: "Aggiorna".into(),
            }],
            timeout_ms: 0,
        },
        &config.notifications,
    );
    toasts.push(
        Notification {
            id: 2,
            app_name: "niri".into(),
            app_icon: String::new(),
            summary: "Workspace 3".into(),
            body: "Finestra spostata su un altro output".into(),
            actions: Vec::new(),
            timeout_ms: 0,
        },
        &config.notifications,
    );
    for _ in 0..30 {
        toasts.update(1.0 / 60.0);
    }
    let toast_size = toasts
        .preferred_size(&config.notifications)
        .unwrap_or(Size::new(360.0, 174.0));
    let mut scene = Scene::new();
    wallpaper(&mut scene, toast_size.w, toast_size.h, &config.theme, 16);
    toasts.build(
        &mut scene,
        toast_size,
        &config.notifications,
        &config.theme,
        &config.font,
    );
    save(
        &gpu,
        &mut renderer,
        "toasts",
        toast_size.w as u32,
        toast_size.h as u32,
        &scene,
    )?;

    // ---- lock screen ----
    let mut lock = GreeterView::new(AuthMode::Lock, &GreeterConfig::default(), false);
    lock.set_clock("14:32".into(), "martedì 20 ottobre".into());
    for ch in "hunter2".chars() {
        lock.key(lard_greeter::GreeterKey::Char(ch.to_string()));
    }
    for _ in 0..50 {
        lock.update(1.0 / 60.0);
    }
    let mut scene = Scene::new();
    let mut hits = Vec::new();
    lock.build_scene(
        &mut scene,
        &mut hits,
        Size::new(W as f32, H as f32),
        &config.theme,
        &config.font,
    );
    save(&gpu, &mut renderer, "lock", W, H, &scene)?;

    lock.on_auth_event(lard_greeter::AuthEvent::Failure {
        message: "Credenziali non valide".into(),
    });
    for _ in 0..18 {
        lock.update(1.0 / 60.0);
    }
    let mut scene = Scene::new();
    let mut hits = Vec::new();
    lock.build_scene(
        &mut scene,
        &mut hits,
        Size::new(W as f32, H as f32),
        &config.theme,
        &config.font,
    );
    save(&gpu, &mut renderer, "lock-error", W, H, &scene)?;

    // ---- greeter ----
    let mut greeter = GreeterView::new(AuthMode::Preview, &GreeterConfig::default(), false);
    greeter.set_clock("09:41".into(), "martedì 20 ottobre".into());
    for ch in "correct horse".chars() {
        greeter.key(lard_greeter::GreeterKey::Char(ch.to_string()));
    }
    for _ in 0..50 {
        greeter.update(1.0 / 60.0);
    }
    greeter.pointer_move(Point::new(850.0, 700.0));
    let mut scene = Scene::new();
    let mut hits = Vec::new();
    greeter.build_scene(
        &mut scene,
        &mut hits,
        Size::new(W as f32, H as f32),
        &config.theme,
        &config.font,
    );
    save(&gpu, &mut renderer, "greeter", W, H, &scene)?;

    // ---- built-in screensaver, one frame per scenario ----
    for (effect, name) in [
        (lard_core::theme::ScreensaverEffect::Liquid, "screensaver-liquid"),
        (lard_core::theme::ScreensaverEffect::Mercury, "screensaver-mercury"),
        (lard_core::theme::ScreensaverEffect::Aurora, "screensaver-aurora"),
        (lard_core::theme::ScreensaverEffect::Gas, "screensaver-gas"),
    ] {
        let mut preview_config = config.clone();
        preview_config.screensaver.effect = effect;
        preview_config.screensaver.idle_minutes = 5;
        // Deterministic previews: no random composition.
        preview_config.screensaver.random = false;
        // The burn-in runs on the wall clock now, so the preview waits it out
        // instead of counting frames.
        preview_config.screensaver.entrance_ms = 300;
        let mut shell = Shell::new(preview_config);
        shell.clock = "21:47".into();
        shell.date = "lun 26 ott".into();
        shell.date_long = "lunedì 26 ottobre".into();
        shell.open_screensaver();
        // The burn-in clock starts on the first drawn frame: build once, then
        // wait the entrance out and build again for the real preview.
        {
            let mut warmup = Scene::new();
            shell.build_screensaver(&mut warmup, Size::new(1280.0, 800.0));
        }
        std::thread::sleep(std::time::Duration::from_millis(420));
        shell.update(1.0 / 60.0);
        let mut scene = Scene::new();
        shell.build_screensaver(&mut scene, Size::new(1280.0, 800.0));
        save(&gpu, &mut renderer, name, 1280, 800, &scene)?;
    }

    // ---- screensaver burn-in, sampled while it takes over the desktop ----
    {
        let mut preview_config = config.clone();
        preview_config.screensaver.random = false;
        preview_config.screensaver.entrance_ms = 1200;
        let mut shell = Shell::new(preview_config);
        shell.clock = "21:47".into();
        shell.date_long = "lunedì 26 ottobre".into();
        shell.open_screensaver();
        let steps = [(24u32, "burn-1"), (40, "burn-2"), (56, "burn-3"), (80, "burn-4")];
        let mut frames = 0;
        for (target, name) in steps {
            while frames < target {
                shell.update(1.0 / 60.0);
                frames += 1;
            }
            let mut scene = Scene::new();
            shell.build_screensaver(&mut scene, Size::new(1280.0, 800.0));
            save(&gpu, &mut renderer, name, 1280, 800, &scene)?;
        }
    }

    println!("preview scritte in ./preview");
    Ok(())
}

fn demo_niri() -> NiriSnapshot {
    NiriSnapshot {
        connected: true,
        windows: vec![WindowView {
            id: 42,
            title: "mimo v2.6 flash vs DeepSeek v4.1 flash".into(),
            app_id: "zen".into(),
        }],
        workspaces: vec![
            WorkspaceView {
                id: 1,
                idx: 0,
                name: None,
                output: Some("eDP-1".into()),
                is_active: false,
                is_focused: false,
                is_urgent: false,
                has_windows: true,
            },
            WorkspaceView {
                id: 2,
                idx: 1,
                name: Some("web".into()),
                output: Some("eDP-1".into()),
                is_active: true,
                is_focused: true,
                is_urgent: false,
                has_windows: true,
            },
            WorkspaceView {
                id: 3,
                idx: 2,
                name: None,
                output: Some("eDP-1".into()),
                is_active: false,
                is_focused: false,
                is_urgent: true,
                has_windows: false,
            },
            WorkspaceView {
                id: 4,
                idx: 3,
                name: None,
                output: Some("eDP-1".into()),
                is_active: false,
                is_focused: false,
                is_urgent: false,
                has_windows: false,
            },
        ],
        focused_window: Some(WindowView {
            id: 10,
            title: "LARD — architecture.md".into(),
            app_id: "foot".into(),
        }),
        keyboard_layout: Some("us".into()),
        focused_output: Some("eDP-1".into()),
    }
}

fn demo_apps() -> Vec<DesktopEntry> {
    [
        "Firefox",
        "Terminal",
        "Files",
        "Settings",
        "Music",
        "Editor",
        "Calculator",
        "Screenshot",
    ]
    .into_iter()
    .map(|name| DesktopEntry {
        file: format!("/tmp/{name}.desktop").into(),
        name: name.into(),
        generic_name: match name {
            "Firefox" => "Web browser".into(),
            "Terminal" => "Terminal emulator".into(),
            "Files" => "File manager".into(),
            "Settings" => "System settings".into(),
            "Music" => "Audio player".into(),
            "Editor" => "Text editor".into(),
            "Calculator" => "Calculator".into(),
            _ => "Screenshot tool".into(),
        },
        comment: String::new(),
        exec: name.to_lowercase(),
        icon: String::new(),
        terminal: false,
        categories: vec!["Utility".into()],
    })
    .collect()
}

fn wallpaper(scene: &mut Scene, w: f32, h: f32, theme: &Theme, bands: u32) {
    scene.vertical_gradient(
        Rect::new(0.0, 0.0, w, h),
        Color::from_hex("#070B16").unwrap(),
        Color::from_hex("#14223A").unwrap(),
        bands,
    );
    scene.glow(
        Rect::new(-w * 0.15, h * 0.35, w * 0.75, h * 0.9),
        theme.accent.with_alpha(0.10),
        160.0,
    );
    scene.glow(
        Rect::new(w * 0.62, -h * 0.3, w * 0.5, h * 0.7),
        Color::from_hex("#365A9A").unwrap().with_alpha(0.13),
        140.0,
    );
}

fn save(
    gpu: &Gpu,
    renderer: &mut Renderer,
    name: &str,
    width: u32,
    height: u32,
    scene: &Scene,
) -> anyhow::Result<()> {
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = gpu.device().create_texture(&wgpu::TextureDescriptor {
        label: Some(name),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    renderer.render_offscreen(
        &view,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        (width, height),
        1.0,
        scene,
    );

    let row = width * 4;
    // copy_texture_to_buffer requires bytes_per_row to be a multiple of 256.
    let padded_row = (row + 255) & !255;
    let buffer = gpu.device().create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded_row) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("copy"),
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
                rows_per_image: Some(height),
            },
        },
        size,
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
    let mut compact = Vec::with_capacity((row * height) as usize);
    for line in 0..height {
        let start = (line * padded_row) as usize;
        compact.extend_from_slice(&pixels[start..start + row as usize]);
    }

    let file = File::create(format!("preview/{name}.png"))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(&compact)?;

    Ok(())
}
