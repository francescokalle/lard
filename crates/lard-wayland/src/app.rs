//! Wayland client, surface roles, input routing and the vsync-driven render
//! loop.

use crate::Mode;
use crate::config_watch;
use crate::ipc::{self, IpcCommand};
use anyhow::{Context, Result};
use calloop::timer::{Timer, TimeoutAction};
use calloop::{EventLoop, LoopHandle};
use calloop_wayland_source::WaylandSource;
use chrono::{Local, Timelike};
use lard_core::{Config, Point, Rect, Size};
use lard_greeter::{
    AuthController, AuthEvent, AuthMode, GreeterAction, GreeterHitArea, GreeterKey, GreeterView,
};
use lard_render::{Gpu, RenderSurface, Renderer, Scene};
use lard_services::Services;
use lard_services::power::{self, PowerAction};
use lard_shell::{OverlayKind, Shell, ShellAction, ShellKey};
use smithay_client_toolkit::background_effect::{BackgroundEffectHandler, BackgroundEffectState};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState, FrameCallbackData};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::protocols::ext::background_effect::v1::client::ext_background_effect_surface_v1::ExtBackgroundEffectSurfaceV1;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::registry_handlers;
use smithay_client_toolkit::seat::keyboard::{
    KeyEvent, KeyboardHandler, Keysym, Modifiers, RawModifiers,
};
use smithay_client_toolkit::seat::pointer::{
    AxisScroll, BTN_LEFT, BTN_RIGHT, PointerEvent, PointerEventKind, PointerHandler,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};
use smithay_client_toolkit::session_lock::{
    SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
    SessionLockSurfaceConfigure,
};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerShellHandler,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use std::collections::HashMap;
use std::ffi::c_void;
use std::time::{Duration, Instant};
use wayland_client::backend::ObjectId;
use wayland_client::globals::registry_queue_init;
use wayland_client::protocol::{wl_keyboard, wl_output, wl_pointer, wl_region, wl_seat, wl_surface};
use wayland_client::{Connection, Proxy, QueueHandle};
use smithay_client_toolkit::dispatch2::Dispatch2;
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

/// User data of the raw `ext-idle-notify-v1` objects. SCTK routes every
/// protocol through `Dispatch2`, so a tiny marker type is all that is needed.
struct IdleData;

#[derive(Clone, Debug)]
enum Role {
    Bar {
        name: String,
    },
    Dock,
    Launcher,
    ControlCenter,
    Settings,
    Popover,
    Wallpapers,
    Osd,
    Toasts,
    /// Expanded centre island with an incoming notification. It lives on the
    /// overlay layer so it can grow over the other windows.
    Notification,
    Greeter,
    Lock,
    /// Built-in screensaver: a full-screen fluid over the session.
    Screensaver,
}

struct OutputEntry {
    output: wl_output::WlOutput,
    name: String,
    scale: i32,
    logical: Option<(u32, u32)>,
}

/// Field order is intentional: the wgpu surface must be destroyed before the
/// Wayland role objects and the `wl_surface`.
struct SurfaceEntry {
    render: Option<RenderSurface>,
    effect: Option<ExtBackgroundEffectSurfaceV1>,
    layer: Option<LayerSurface>,
    #[allow(dead_code)] // kept alive for the lifetime of the lock surface
    lock: Option<SessionLockSurface>,
    wl_surface: wl_surface::WlSurface,
    role: Role,
    output_id: Option<ObjectId>,
    logical: (u32, u32),
    requested_size: (u32, u32),
    scale: f32,
    configured: bool,
    frame_pending: bool,
    needs_redraw: bool,
    /// Per-surface hit map: lock surfaces on multiple outputs must not share
    /// the geometry of whichever surface happened to be built last.
    greeter_hits: Vec<GreeterHitArea>,
    /// Last blur regions (surface-local logical px + radius) applied to this
    /// surface. Kept to avoid recreating the wl_region on every redraw.
    blur_rect: Option<Vec<[i32; 5]>>,
}

struct LayerSpec {
    role: Role,
    output: Option<wl_output::WlOutput>,
    output_id: Option<ObjectId>,
    layer: Layer,
    anchor: Anchor,
    size: (u32, u32),
    keyboard: KeyboardInteractivity,
    exclusive_zone: i32,
    margins: (i32, i32, i32, i32),
    namespace: &'static str,
    input_none: bool,
}

struct App {
    // Wayland objects (surfaces first, so wgpu surfaces drop before wl_surface)
    surfaces: HashMap<ObjectId, SurfaceEntry>,
    outputs: HashMap<ObjectId, OutputEntry>,
    session_lock: Option<SessionLock>,
    locked: bool,
    unlocking: bool,
    /// `ext-idle-notify-v1`: the compositor tells us when the user went idle.
    idle_notifier: Option<ExtIdleNotifierV1>,
    idle_notification: Option<ExtIdleNotificationV1>,
    seat: Option<wl_seat::WlSeat>,

    // GPU
    renderer: Renderer,
    gpu: Gpu,
    scene: Scene,

    // Protocol state objects
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    background_effect: BackgroundEffectState,
    session_lock_state: SessionLockState,
    qh: QueueHandle<App>,
    // The Wayland display must outlive wgpu (the instance holds a borrowed
    // display handle) and every surface; keep it near the end of the struct.
    conn: Connection,

    // UI
    config: Config,
    mode: Mode,
    shell: Option<Shell>,
    greeter: Option<GreeterView>,
    auth: Option<AuthController>,
    services: Option<Services>,

    // Input
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pointer: Option<wl_pointer::WlPointer>,
    modifiers: Modifiers,
    keyboard_focus: Option<ObjectId>,

    // Loop bookkeeping
    loop_handle: LoopHandle<'static, App>,
    clock_armed: bool,
    deadline_armed: Option<Instant>,
    frame_clock: lard_core::anim::FrameClock,
    force_update: bool,
    callback_fired: bool,
    animating: bool,
    exit: bool,
    blur_supported: bool,
}

/// Connect to the compositor, discovering the socket when the environment does
/// not name one.
///
/// The shell is usually started by systemd, whose environment does not always
/// carry `WAYLAND_DISPLAY` (a session manager may not have imported it). The
/// socket is then picked from the runtime directory, so the bar comes up after
/// a reboot instead of failing in a restart loop.
fn connect_wayland() -> Result<Connection> {
    match Connection::connect_to_env() {
        Ok(connection) => Ok(connection),
        Err(error) => {
            let fallback = discover_wayland_display().and_then(connect_to_socket);
            match fallback {
                Some(connection) => {
                    tracing::info!(%error, "WAYLAND_DISPLAY is not set, using the socket found in XDG_RUNTIME_DIR");
                    // Children (wallpaper daemon, launchers) inherit the
                    // environment: hand them the display we just found. This
                    // runs once at startup, before any thread is spawned.
                    if let Some(name) = discover_wayland_display() {
                        unsafe {
                            std::env::set_var("WAYLAND_DISPLAY", name);
                        }
                    }
                    Ok(connection)
                }
                None => Err(error.into()),
            }
        }
    }
}

/// Connect to a compositor socket found in the runtime directory.
fn connect_to_socket(name: String) -> Option<Connection> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let path = std::path::Path::new(&runtime).join(name);
    let stream = std::os::unix::net::UnixStream::connect(path).ok()?;
    Connection::from_socket(stream).ok()
}

/// Newest `wayland-*` socket in the runtime directory (the compositor's own
/// display, never a helper socket such as the wallpaper daemon's).
fn discover_wayland_display() -> Option<String> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let mut newest: Option<(std::time::SystemTime, String)> = None;
    for entry in std::fs::read_dir(runtime).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("wayland-") else {
            continue;
        };
        // Only the compositor socket: `wayland-1`, not `wayland-1-awww.sock`.
        if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
            newest = Some((modified, name));
        }
    }
    newest.map(|(_, name)| name)
}

pub fn run(mode: Mode) -> Result<()> {
    let config = Config::load_default()?;
    let conn = match connect_wayland() {
        Ok(conn) => conn,
        Err(error) => {
            if !matches!(mode, Mode::Shell) {
                return Err(error).context("cannot connect to a Wayland compositor");
            }
            // Without a compositor there is nothing to draw. Exit with
            // EX_CONFIG so the systemd unit can use
            // `RestartPreventExitStatus=78` instead of looping every 2 s until
            // the next login.
            eprintln!("lard: cannot connect to a Wayland compositor: {error:#}");
            std::process::exit(crate::EXIT_NO_COMPOSITOR);
        }
    };
    lard_services::niri::export_socket_env();
    let (globals, event_queue) = registry_queue_init(&conn)?;
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<App> = EventLoop::try_new().context("calloop init")?;
    let loop_handle = event_loop.handle();

    let display_ptr = std::ptr::NonNull::new(conn.backend().display_ptr() as *mut std::ffi::c_void)
        .context("null wl_display")?;
    let gpu = Gpu::new(Some(display_ptr))?;
    let renderer = Renderer::new(&gpu, &config.font)?;
    let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor is required")?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .context("wlr-layer-shell is required (niri supports it)")?;
    let output_state = OutputState::new(&globals, &qh);
    let seat_state = SeatState::new(&globals, &qh);
    let registry_state = RegistryState::new(&globals);
    let background_effect = BackgroundEffectState::new(&globals, &qh);
    let session_lock_state = SessionLockState::new(&globals, &qh);
    // Idle detection for the built-in screensaver. Optional: compositors
    // without ext-idle-notify-v1 simply keep the classic idle behaviour.
    let idle_notifier = globals
        .bind::<ExtIdleNotifierV1, _, IdleData>(&qh, 2..=2, IdleData)
        .map_err(|error| tracing::debug!(%error, "ext-idle-notify-v1 unavailable"))
        .ok();

    let shell = (mode == Mode::Shell).then(|| Shell::new(config.clone()));

    let auth_mode = match mode {
        Mode::Lock => AuthMode::Lock,
        Mode::Greeter { preview: true } => AuthMode::Preview,
        Mode::Greeter { preview: false } => AuthMode::Greeter,
        Mode::Shell => AuthMode::Preview,
    };
    let greeter = (mode != Mode::Shell)
        .then(|| GreeterView::new(auth_mode, &config.greeter, config.animation.reduce_motion));

    let (service_tx, service_rx) = calloop::channel::channel();
    if shell.is_some() {
        event_loop
            .handle()
            .insert_source(service_rx, |event, _, app| {
                if let calloop::channel::Event::Msg(event) = event {
                    app.on_service(event);
                }
            })
            .map_err(|e| anyhow::anyhow!("service channel: {e}"))?;
    }
    let services = shell.as_ref().map(|_| {
        Services::start(
            service_tx,
            config.notifications.default_timeout_ms,
            config.notifications.enabled,
        )
    });

    let (auth_tx, auth_rx) = calloop::channel::channel();
    if greeter.is_some() {
        event_loop
            .handle()
            .insert_source(auth_rx, |event, _, app| {
                if let calloop::channel::Event::Msg(event) = event {
                    app.on_auth_event(event);
                }
            })
            .map_err(|e| anyhow::anyhow!("auth channel: {e}"))?;
    }
    let auth = match &greeter {
        Some(_) => Some(
            AuthController::new(auth_mode, &config.greeter, auth_tx)
                .map_err(|error| anyhow::anyhow!(error))?,
        ),
        None => None,
    };

    if shell.is_some() {
        let (ipc_tx, ipc_rx) = calloop::channel::channel();
        event_loop
            .handle()
            .insert_source(ipc_rx, |event, _, app| {
                if let calloop::channel::Event::Msg(command) = event {
                    app.on_ipc(command);
                }
            })
            .map_err(|e| anyhow::anyhow!("ipc channel: {e}"))?;
        config_watch::spawn(ipc_tx.clone());
        ipc::spawn(ipc_tx)?;
    }

    let mut app = App {
        surfaces: HashMap::new(),
        outputs: HashMap::new(),
        session_lock: None,
        locked: false,
        unlocking: false,
        idle_notifier,
        idle_notification: None,
        seat: None,
        conn: conn.clone(),
        qh,
        registry_state,
        output_state,
        seat_state,
        compositor,
        layer_shell,
        background_effect,
        session_lock_state,
        renderer,
        gpu,
        scene: Scene::new(),
        config,
        mode,
        shell,
        greeter,
        auth,
        services,
        keyboard: None,
        pointer: None,
        modifiers: Modifiers::default(),
        keyboard_focus: None,
        loop_handle: loop_handle.clone(),
        clock_armed: false,
        deadline_armed: None,
        frame_clock: lard_core::anim::FrameClock::new(),
        force_update: true,
        callback_fired: false,
        animating: true,
        exit: false,
        blur_supported: false,
    };

    if mode == Mode::Lock {
        app.start_session_lock()?;
        // Integration-test hook: request a real ext-session-lock surface,
        // then release it after N seconds. Used by the headless/SSH smoke test
        // without risking an orphan lock.
        if let Some(secs) = std::env::var("NIRI_SUITE_LOCK_TEST_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
        {
            let _ = app.loop_handle.insert_source(
                Timer::from_deadline(Instant::now() + Duration::from_secs(secs.max(1))),
                |_, _, app: &mut App| {
                    tracing::info!("lock test elapsed; unlocking");
                    app.unlock_session();
                    TimeoutAction::Drop
                },
            );
        }
    }

    app.update_clock();
    app.arm_clock();

    WaylandSource::new(conn, event_queue)
        .insert(event_loop.handle())
        .expect("cannot insert the Wayland source");

    while !app.exit {
        event_loop.dispatch(None::<Duration>, &mut app)?;
        app.after_dispatch();
    }

    Ok(())
}

impl App {
    fn on_service(&mut self, event: lard_services::ServiceEvent) {
        if let Some(shell) = &mut self.shell {
            shell.apply(event);
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    fn on_auth_event(&mut self, event: AuthEvent) {
        if let Some(greeter) = &mut self.greeter {
            greeter.on_auth_event(event);
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    fn on_ipc(&mut self, command: IpcCommand) {
        if self.mode != Mode::Shell {
            return;
        }
        match command {
            IpcCommand::Launcher => {
                if let Some(shell) = &mut self.shell {
                    shell.open_launcher();
                }
            }
            IpcCommand::ControlCenter => {
                if let Some(shell) = &mut self.shell {
                    shell.toggle_control();
                }
            }
            IpcCommand::Controls => {
                if let Some(shell) = &mut self.shell {
                    shell.open_controls_popover();
                }
            }
            IpcCommand::Notifications => {
                if let Some(shell) = &mut self.shell {
                    shell.open_notifications_panel();
                }
            }
            IpcCommand::Wifi => {
                if let Some(shell) = &mut self.shell {
                    shell.open_wifi_panel();
                }
            }
            IpcCommand::Bluetooth => {
                if let Some(shell) = &mut self.shell {
                    shell.open_bluetooth_panel();
                }
            }
            IpcCommand::PowerMenu => {
                if let Some(shell) = &mut self.shell {
                    shell.open_power_menu();
                }
            }
            IpcCommand::Settings(tab) => {
                if let Some(shell) = &mut self.shell {
                    shell.open_settings();
                    if let Some(tab) = tab {
                        shell.select_settings_tab(tab);
                    }
                }
                if let Some(services) = &self.services {
                    services.request_outputs();
                }
            }
            IpcCommand::Wallpapers => {
                if let Some(shell) = &mut self.shell {
                    shell.open_wallpaper();
                }
            }
            IpcCommand::VolumeUp => self.perform(ShellAction::VolumeStep(5)),
            IpcCommand::VolumeDown => self.perform(ShellAction::VolumeStep(-5)),
            IpcCommand::ToggleMute => self.perform(ShellAction::ToggleMute),
            IpcCommand::BrightnessUp => self.perform(ShellAction::BrightnessStep(5)),
            IpcCommand::BrightnessDown => self.perform(ShellAction::BrightnessStep(-5)),
            IpcCommand::Reload => self.reload_config(),
            IpcCommand::Lock => self.spawn_lockscreen(),
            IpcCommand::Screensaver => {
                let open = self.shell.as_ref().is_some_and(Shell::screensaver_open);
                if let Some(shell) = &mut self.shell {
                    if open {
                        shell.close_screensaver();
                    } else {
                        shell.open_screensaver();
                    }
                }
            }
            IpcCommand::Quit => self.exit = true,
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    fn reload_config(&mut self) {
        if self.shell.is_none() {
            return;
        }
        let config = match Config::load_default() {
            Ok(config) => config,
            Err(error) => {
                tracing::error!(%error, "config reload failed; keeping the old configuration");
                return;
            }
        };
        let renderer = match Renderer::new(&self.gpu, &config.font) {
            Ok(renderer) => renderer,
            Err(error) => {
                tracing::error!(%error, "cannot rebuild renderer for config reload");
                return;
            }
        };
        self.renderer = renderer;
        self.config = config.clone();
        if let Some(shell) = &mut self.shell {
            shell.set_config(config);
        }
        self.rebuild_shell_surfaces();
        self.arm_screensaver_idle();
        self.force_update = true;
        tracing::info!("configuration reloaded");
    }

    fn rebuild_shell_surfaces(&mut self) {
        if self.mode != Mode::Shell {
            return;
        }
        let outputs: Vec<(wl_output::WlOutput, String)> = self
            .outputs
            .values()
            .map(|entry| (entry.output.clone(), entry.name.clone()))
            .collect();
        // Drop every shell surface (and its swapchain) before recreating it
        // with the new geometry/theme.
        self.surfaces.clear();
        for (output, name) in outputs {
            if let Err(error) = self.create_bar(&output, &name) {
                tracing::error!(%error, "cannot recreate bar after reload");
            }
        }
        self.force_update = true;
    }

    fn spawn_lockscreen(&mut self) {
        // The lock screen covers everything: stop the screensaver so it does
        // not keep rendering underneath it.
        if let Some(shell) = &mut self.shell {
            shell.close_screensaver();
        }
        match std::process::Command::new("lard").arg("lock").spawn() {
            Ok(_) => tracing::info!("lockscreen launched"),
            Err(error) => tracing::error!(%error, "cannot launch lard lock"),
        }
    }

    fn perform(&mut self, action: ShellAction) {
        match action {
            ShellAction::OpenSettings => {
                if let Some(shell) = &mut self.shell {
                    shell.open_settings();
                }
                if let Some(services) = &self.services {
                    services.request_outputs();
                }
            }
            ShellAction::ReloadConfig => self.reload_config(),
            ShellAction::ReloadNiriConfig(path) => {
                // Ask niri to load the file we just wrote.
                if let Some(services) = &self.services {
                    services.spawn_command(vec![
                        "niri".into(),
                        "msg".into(),
                        "action".into(),
                        "load-config-file".into(),
                        "--path".into(),
                        path.to_string_lossy().into_owned(),
                    ]);
                }
            }
            ShellAction::OpenLauncher => {
                if let Some(shell) = &mut self.shell {
                    shell.open_launcher();
                }
            }
            ShellAction::ToggleControlCenter => {
                if let Some(shell) = &mut self.shell {
                    shell.toggle_control();
                }
            }
            ShellAction::ToggleBluetooth => {
                let enabled = self
                    .shell
                    .as_ref()
                    .is_some_and(|shell| !shell.bluetooth.powered);
                if let Some(services) = &self.services {
                    services.toggle_bluetooth(enabled);
                }
            }
            ShellAction::ToggleDnd => {
                if let Some(shell) = &mut self.shell {
                    shell.dnd = !shell.dnd;
                }
            }
            ShellAction::FocusWorkspace(id) => {
                if let Some(services) = &self.services {
                    services.focus_workspace(id);
                }
            }
            ShellAction::Spawn(argv) => {
                if let Some(services) = &self.services {
                    services.spawn_command(argv);
                }
            }
            ShellAction::Copy(text) => {
                // `wl-copy` reads the clipboard text from stdin and keeps
                // serving it, so it runs in its own thread.
                std::thread::spawn(move || {
                    use std::io::Write;
                    let child = std::process::Command::new("wl-copy")
                        .stdin(std::process::Stdio::piped())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn();
                    if let Ok(mut child) = child {
                        if let Some(stdin) = child.stdin.as_mut() {
                            let _ = stdin.write_all(text.as_bytes());
                        }
                        let _ = child.wait();
                    }
                });
            }
            ShellAction::VolumeStep(step) => {
                if let Some(shell) = &mut self.shell {
                    shell.nudge_volume(step);
                }
                if let Some(services) = &self.services {
                    services.volume_step(step);
                }
            }
            ShellAction::SetVolume(percent) => {
                if let Some(services) = &self.services {
                    services.set_volume(percent);
                }
            }
            ShellAction::ToggleMute => {
                if let Some(services) = &self.services {
                    services.toggle_mute();
                }
            }
            ShellAction::BrightnessStep(step) => {
                if let Some(shell) = &mut self.shell {
                    shell.nudge_brightness(step);
                }
                if let Some(services) = &self.services {
                    services.brightness_step(step);
                }
            }
            ShellAction::SetBrightness(percent) => {
                if let Some(services) = &self.services {
                    services.set_brightness(percent);
                }
            }
            ShellAction::SetWifi(enabled) => {
                if let Some(services) = &self.services {
                    services.toggle_wifi(enabled);
                }
            }
            ShellAction::SetBluetooth(enabled) => {
                if let Some(services) = &self.services {
                    services.toggle_bluetooth(enabled);
                }
            }
            ShellAction::OpenPowerMenu => {
                if let Some(shell) = &mut self.shell {
                    shell.open_power_menu();
                }
            }
            ShellAction::OpenWifiPanel => {
                if let Some(shell) = &mut self.shell {
                    shell.open_wifi_panel();
                }
            }
            ShellAction::OpenBluetoothPanel => {
                if let Some(shell) = &mut self.shell {
                    shell.open_bluetooth_panel();
                }
            }
            ShellAction::ScanWifi => {
                if let Some(services) = &self.services {
                    services.scan_wifi();
                }
            }
            ShellAction::ConnectWifi {
                ssid,
                password,
                identity,
            } => {
                if let Some(services) = &self.services {
                    services.connect_wifi(ssid, identity, password);
                }
            }
            ShellAction::DisconnectWifi => {
                if let Some(services) = &self.services {
                    services.disconnect_wifi();
                }
            }
            ShellAction::RefreshBluetooth => {
                if let Some(services) = &self.services {
                    services.refresh_bluetooth();
                }
            }
            ShellAction::ScanBluetooth => {
                if let Some(services) = &self.services {
                    services.scan_bluetooth();
                }
            }
            ShellAction::ConnectBluetooth { address } => {
                if let Some(services) = &self.services {
                    services.connect_bluetooth(address);
                }
            }
            ShellAction::DisconnectBluetooth { address } => {
                if let Some(services) = &self.services {
                    services.disconnect_bluetooth(address);
                }
            }
            ShellAction::CloseNotification { id, reason } => {
                if let Some(services) = &self.services {
                    services.close_notification(id, reason);
                }
            }
            ShellAction::NotificationAction { id, action } => {
                if let Some(services) = &self.services {
                    services.invoke_action(id, action);
                }
            }
            ShellAction::Power(action) => {
                if action == PowerAction::Lock {
                    self.spawn_lockscreen();
                } else {
                    power::perform_async(action);
                }
            }
            ShellAction::Lock => self.spawn_lockscreen(),
            ShellAction::Quit => self.exit = true,
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    fn drain_actions(&mut self) {
        let actions = self
            .shell
            .as_mut()
            .map(Shell::take_actions)
            .unwrap_or_default();
        for action in actions {
            self.perform(action);
        }
    }

    fn after_dispatch(&mut self) {
        if self.exit {
            return;
        }

        let forced = self.force_update;
        let tick = self.callback_fired || self.force_update;
        self.callback_fired = false;
        self.force_update = false;
        let dt = if tick {
            self.frame_clock.tick().max(0.0001)
        } else {
            0.0
        };

        if tick {
            let mut animating = false;
            if let Some(shell) = &mut self.shell {
                animating |= shell.update(dt);
            }
            if let Some(greeter) = &mut self.greeter {
                animating |= greeter.update(dt);
            }
            self.animating = animating;
        }

        self.drain_actions();
        self.check_greeter_finished();
        if self.exit {
            return;
        }
        self.sync_surfaces();

        let dirty = self.shell.as_mut().is_some_and(Shell::take_dirty);
        if dirty || self.animating || (forced && self.greeter.is_some()) {
            self.mark_all_dirty();
        }
        self.arm_next_deadline();

        let ids: Vec<ObjectId> = self
            .surfaces
            .iter()
            .filter(|(_, entry)| entry.needs_redraw && entry.configured && !entry.frame_pending)
            .map(|(id, _)| id.clone())
            .collect();

        for id in ids {
            // Register the callback before presenting: the present commits the
            // surface and the callback fires at the next vsync. This paces
            // hover/state redraws even when no spring is running.
            self.request_frame(&id);
            self.draw_surface(&id);
        }

        if self.animating {
            self.mark_all_dirty();
        }
    }

    fn mark_all_dirty(&mut self) {
        for entry in self.surfaces.values_mut() {
            entry.needs_redraw = true;
        }
    }

    fn request_frame(&mut self, id: &ObjectId) {
        let qh = self.qh.clone();
        if let Some(entry) = self.surfaces.get_mut(id)
            && entry.configured
            && !entry.frame_pending
        {
            entry.frame_pending = true;
            entry
                .wl_surface
                .frame(&qh, FrameCallbackData(entry.wl_surface.clone()));
        }
    }

    fn draw_surface(&mut self, id: &ObjectId) {
        let Some(entry) = self.surfaces.get(id) else {
            return;
        };
        if entry.logical.0 == 0 || entry.logical.1 == 0 {
            return;
        }
        let role = entry.role.clone();
        let size = Size::new(entry.logical.0 as f32, entry.logical.1 as f32);
        self.scene.clear();
        let mut greeter_hits = Vec::new();
        let overlay = match &role {
            Role::Launcher => Some(OverlayKind::Launcher),
            Role::ControlCenter => Some(OverlayKind::ControlCenter),
            Role::Settings => Some(OverlayKind::Settings),
            Role::Popover => Some(OverlayKind::Popover),
            Role::Wallpapers => Some(OverlayKind::Wallpapers),
            Role::Notification => Some(OverlayKind::Notification),
            _ => None,
        };
        // The bar and the dock are floating islands: their backdrop blur is
        // clipped to the panels themselves instead of the whole surface.
        let bar_output = match &role {
            Role::Bar { name } => Some(name.clone()),
            _ => None,
        };
        let is_dock = matches!(&role, Role::Dock);

        match role {
            Role::Bar { name } => {
                if let Some(shell) = &mut self.shell {
                    shell.build_bar(&mut self.scene, &name, size);
                }
            }
            Role::Dock => {
                if let Some(shell) = &mut self.shell {
                    shell.build_dock(&mut self.scene, size);
                }
            }
            Role::Launcher => {
                if let Some(shell) = &mut self.shell {
                    shell.build_launcher(&mut self.scene, size);
                }
            }
            Role::ControlCenter => {
                if let Some(shell) = &mut self.shell {
                    shell.build_control(&mut self.scene, size);
                }
            }
            Role::Settings => {
                if let Some(shell) = &mut self.shell {
                    shell.build_settings(&mut self.scene, size);
                }
            }
            Role::Popover => {
                let (anchor, card_w) = match &self.shell {
                    Some(shell) => (
                        shell.popover_anchor_x(),
                        shell.popover_card_size().map_or(0.0, |s| s.w),
                    ),
                    None => (0.0, 0.0),
                };
                let left = self.popover_left_margin(anchor, card_w) as f32;
                if let Some(shell) = &mut self.shell {
                    shell.build_popover(&mut self.scene, size, left);
                }
            }
            Role::Wallpapers => {
                if let Some(shell) = &mut self.shell {
                    shell.build_wallpaper(&mut self.scene, size);
                }
            }
            Role::Osd => {
                if let Some(shell) = &mut self.shell {
                    shell.build_osd(&mut self.scene, size);
                }
            }
            Role::Toasts => {
                if let Some(shell) = &mut self.shell {
                    shell.build_toasts(&mut self.scene, size);
                }
            }
            Role::Notification => {
                if let Some(shell) = &mut self.shell {
                    shell.build_notification_island(&mut self.scene, size);
                }
            }
            Role::Screensaver => {
                if let Some(shell) = &mut self.shell {
                    shell.build_screensaver(&mut self.scene, size);
                }
            }
            Role::Greeter | Role::Lock => {
                if let Some(greeter) = &mut self.greeter {
                    greeter.build_scene(
                        &mut self.scene,
                        &mut greeter_hits,
                        size,
                        &self.config.theme,
                        &self.config.font,
                    );
                }
            }
        }

        let island_regions: Vec<(Rect, f32)> = match (&bar_output, is_dock) {
            (Some(name), _) => self
                .shell
                .as_ref()
                .map(|shell| shell.bar_blur_regions(name))
                .unwrap_or_default(),
            (None, true) => self
                .shell
                .as_ref()
                .map(|shell| shell.dock_blur_regions())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let is_island = bar_output.is_some() || is_dock;
        if is_island {
            if self.config.theme.blur && self.config.theme.island_blur && !island_regions.is_empty()
            {
                self.set_blur_regions(id, &island_regions);
            } else {
                self.clear_blur_rect(id);
            }
        } else if self.config.theme.blur
            && let Some(kind) = overlay
            && let Some((rect, radius)) = self
                .shell
                .as_ref()
                .and_then(|shell| shell.overlay_shape(kind))
        {
            self.set_blur_regions(id, &[(rect, radius)]);
        } else {
            // Remove a previously-set region when blur is toggled off, so the
            // compositor does not keep blurring behind an opaque panel.
            self.clear_blur_rect(id);
        }

        let Some(entry) = self.surfaces.get_mut(id) else {
            return;
        };
        entry.greeter_hits = greeter_hits;
        let Some(render) = entry.render.as_mut() else {
            return;
        };
        let stats = self.renderer.render(&self.gpu, render, &self.scene);
        entry.needs_redraw = false;
        if !stats.presented {
            entry.needs_redraw = true;
            entry.frame_pending = false;
        } else {
            tracing::trace!(
                rects = stats.rect_instances,
                glyphs = stats.glyph_instances,
                draws = stats.draw_calls,
                "frame presented"
            );
        }
    }

    fn update_clock(&mut self) {
        let now = Local::now();
        if let Some(shell) = &mut self.shell {
            shell.refresh_clock(now);
        }
        if let Some(greeter) = &mut self.greeter {
            greeter.set_clock(
                now.format(&self.config.greeter.clock_format).to_string(),
                lard_shell::italian_date_long(now),
            );
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    fn arm_clock(&mut self) {
        if self.clock_armed {
            return;
        }
        let format = if self.shell.is_some() {
            &self.config.bar.clock_format
        } else {
            &self.config.greeter.clock_format
        };
        let seconds = if format.contains("%S") {
            1
        } else {
            60u64
                .saturating_sub(u64::from(Local::now().second()))
                .max(1)
        };
        let deadline = Instant::now() + Duration::from_secs(seconds);
        self.clock_armed = true;
        let inserted = self.loop_handle.insert_source(
            Timer::from_deadline(deadline),
            |_, _, app: &mut App| {
                app.clock_armed = false;
                app.update_clock();
                app.arm_clock();
                TimeoutAction::Drop
            },
        );
        if inserted.is_err() {
            self.clock_armed = false;
        }
    }

    fn arm_next_deadline(&mut self) {
        let deadline = self
            .shell
            .as_ref()
            .and_then(Shell::next_deadline)
            .into_iter()
            .chain(self.greeter.as_ref().and_then(GreeterView::next_deadline))
            .min();
        let Some(deadline) = deadline else {
            return;
        };
        if self.deadline_armed.is_some_and(|armed| armed <= deadline) {
            return;
        }
        self.deadline_armed = Some(deadline);
        let inserted = self.loop_handle.insert_source(
            Timer::from_deadline(deadline),
            |_, _, app: &mut App| {
                app.deadline_armed = None;
                app.force_update = true;
                TimeoutAction::Drop
            },
        );
        if inserted.is_err() {
            self.deadline_armed = None;
        }
    }

    // ---- surface management ----

    fn sync_surfaces(&mut self) {
        if self.mode != Mode::Shell {
            return;
        }
        let Some(shell) = &self.shell else {
            return;
        };
        let launcher = shell.launcher_wants_surface();
        let control = shell.control_wants_surface();
        let dock_size = shell.dock_size();
        let settings = shell.settings_wants_surface();
        let popover_open = shell.popover_wants_surface();
        let wallpapers = shell.wallpaper_wants_surface();
        let osd = shell.osd_wants_surface();
        let toast_size = shell.toast_size();
        let osd_size = shell.osd_size();
        let notification = shell.notification_surface_size();

        if launcher
            && self.has_role(|r| matches!(r, Role::ControlCenter))
            && !self.shell.as_ref().is_some_and(Shell::control_open)
        {
            self.remove_role(|r| matches!(r, Role::ControlCenter));
        }
        if control
            && self.has_role(|r| matches!(r, Role::Launcher))
            && !self.shell.as_ref().is_some_and(Shell::launcher_open)
        {
            self.remove_role(|r| matches!(r, Role::Launcher));
        }

        match dock_size {
            Some(size) => {
                let requested = (
                    size.w.ceil().max(24.0) as u32,
                    size.h.ceil().max(24.0) as u32,
                );
                if self.has_role(|r| matches!(r, Role::Dock)) {
                    self.resize_dock(requested);
                } else if let Err(error) = self.create_dock(requested) {
                    tracing::error!(%error, "cannot create dock surface");
                }
            }
            None => self.remove_role(|r| matches!(r, Role::Dock)),
        }

        if popover_open {
            if !self.has_role(|r| matches!(r, Role::Popover)) {
                if let Err(error) = self.create_popover() {
                    tracing::error!(%error, "cannot create popover surface");
                }
            }
        } else {
            self.remove_role(|r| matches!(r, Role::Popover));
        }

        if wallpapers && !self.has_role(|r| matches!(r, Role::Wallpapers)) {
            if let Err(error) = self.create_wallpapers() {
                tracing::error!(%error, "cannot create wallpapers surface");
            }
        } else if !wallpapers {
            self.remove_role(|r| matches!(r, Role::Wallpapers));
        }

        if settings && !self.has_role(|r| matches!(r, Role::Settings)) {
            if let Err(error) = self.create_settings() {
                tracing::error!(%error, "cannot create settings surface");
            }
        } else if !settings {
            self.remove_role(|r| matches!(r, Role::Settings));
        }

        if launcher && !self.has_role(|r| matches!(r, Role::Launcher)) {
            if let Err(error) = self.create_launcher() {
                tracing::error!(%error, "cannot create launcher surface");
            }
        } else if !launcher {
            self.remove_role(|r| matches!(r, Role::Launcher));
        }

        if control && !self.has_role(|r| matches!(r, Role::ControlCenter)) {
            if let Err(error) = self.create_control_center() {
                tracing::error!(%error, "cannot create control center surface");
            }
        } else if !control {
            self.remove_role(|r| matches!(r, Role::ControlCenter));
        }

        if osd && !self.has_role(|r| matches!(r, Role::Osd)) {
            if let Err(error) = self.create_osd(osd_size) {
                tracing::error!(%error, "cannot create OSD surface");
            }
        } else if !osd {
            self.remove_role(|r| matches!(r, Role::Osd));
        }

        match toast_size {
            Some(size) => {
                let requested = (size.w.ceil() as u32, size.h.ceil() as u32);
                if self.has_role(|r| matches!(r, Role::Toasts)) {
                    self.resize_toasts(requested);
                } else if let Err(error) = self.create_toasts(requested) {
                    tracing::error!(%error, "cannot create notification surface");
                }
            }
            None => self.remove_role(|r| matches!(r, Role::Toasts)),
        }

        match notification {
            Some(size) => {
                let requested = (0, size.h.ceil().max(20.0) as u32);
                if self.has_role(|r| matches!(r, Role::Notification)) {
                    self.resize_notification(requested);
                } else if let Err(error) = self.create_notification(requested) {
                    tracing::error!(%error, "cannot create the notification island");
                }
            }
            None => self.remove_role(|r| matches!(r, Role::Notification)),
        }

        // The screensaver covers every output. The surface is kept alive until
        // the exit fade is over, so leaving is the exact reverse of entering.
        let saver = self
            .shell
            .as_ref()
            .is_some_and(Shell::screensaver_wants_surface);
        if saver {
            if !self.has_role(|role| matches!(role, Role::Screensaver)) {
                let outputs: Vec<wl_output::WlOutput> = self.output_state.outputs().collect();
                for output in outputs {
                    if let Err(error) = self.create_screensaver(output) {
                        tracing::error!(%error, "cannot create screensaver surface");
                    }
                }
            }
        } else if self.has_role(|role| matches!(role, Role::Screensaver)) {
            self.remove_role(|role| matches!(role, Role::Screensaver));
        }
    }

    /// Full-screen fluid overlay on the given output.
    fn create_screensaver(&mut self, output: wl_output::WlOutput) -> Result<()> {
        let output_id = output.id();
        self.create_layer(LayerSpec {
            role: Role::Screensaver,
            output: Some(output),
            output_id: Some(output_id),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            // Zero means "fill the anchored area": the compositor configures
            // the surface with the full output size.
            size: (0, 0),
            keyboard: KeyboardInteractivity::Exclusive,
            exclusive_zone: -1,
            margins: (0, 0, 0, 0),
            namespace: "lard-screensaver",
            input_none: false,
        })
    }

    /// The notification island is taller when the notification has a body: the
    /// two lines need room, like the other panels.
    fn resize_notification(&mut self, size: (u32, u32)) {
        for entry in self.surfaces.values_mut() {
            if matches!(entry.role, Role::Notification) && entry.requested_size != size {
                entry.requested_size = size;
                if let Some(layer) = &entry.layer {
                    layer.set_size(size.0, size.1);
                    layer.commit();
                }
            }
        }
    }

    fn has_role(&self, predicate: impl Fn(&Role) -> bool) -> bool {
        self.surfaces.values().any(|entry| predicate(&entry.role))
    }

    fn remove_role(&mut self, predicate: impl Fn(&Role) -> bool) {
        let ids: Vec<ObjectId> = self
            .surfaces
            .iter()
            .filter(|(_, entry)| predicate(&entry.role))
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.surfaces.remove(&id);
        }
    }

    /// (Re)create the idle notification that opens the screensaver. Called
    /// when the seat appears and after a configuration reload: the timeout and
    /// the enable flag live in the shell configuration.
    fn arm_screensaver_idle(&mut self) {
        self.idle_notification = None;
        if !self.config.screensaver.enabled {
            tracing::debug!("screensaver disabled; idle timer not armed");
            return;
        }
        let (Some(notifier), Some(seat)) = (self.idle_notifier.as_ref(), self.seat.as_ref())
        else {
            tracing::debug!("no ext-idle-notify seat yet; idle timer not armed");
            return;
        };
        let timeout = self.config.screensaver.idle_secs().saturating_mul(1000);
        // `get_idle_notification` (not the input-only variant) so an idle
        // inhibitor — a video, a call — also holds the screensaver back.
        self.idle_notification =
            Some(notifier.get_idle_notification(timeout, seat, &self.qh, IdleData));
        tracing::info!(secs = timeout / 1000, "screensaver idle timer armed");
    }

    fn create_layer(&mut self, spec: LayerSpec) -> Result<()> {
        let qh = self.qh.clone();
        let wl = self.compositor.create_surface(&qh);
        let layer = self.layer_shell.create_layer_surface(
            &qh,
            wl.clone(),
            spec.layer,
            Some(spec.namespace),
            spec.output.as_ref(),
        );
        layer.set_anchor(spec.anchor);
        layer.set_size(spec.size.0, spec.size.1);
        layer.set_keyboard_interactivity(spec.keyboard);
        layer.set_exclusive_zone(spec.exclusive_zone);
        layer.set_margin(
            spec.margins.0,
            spec.margins.1,
            spec.margins.2,
            spec.margins.3,
        );

        let scale = spec
            .output_id
            .as_ref()
            .and_then(|id| self.outputs.get(id))
            .map_or(1.0, |output| output.scale.max(1) as f32);
        wl.set_buffer_scale(scale as i32);

        if spec.input_none {
            let region = self.compositor.wl_compositor().create_region(&qh, ());
            wl.set_input_region(Some(&region));
            region.destroy();
        }

        layer.commit();

        let display = self.conn.backend().display_ptr();
        // SAFETY: `wl` and `display` outlive the wgpu surface (see SurfaceEntry
        // field order and Drop order).
        let render = unsafe {
            RenderSurface::new(
                &self.gpu,
                display as *mut c_void,
                wl.id().as_ptr() as *mut c_void,
                (0, 0),
                scale,
            )?
        };

        let effect = if matches!(
            spec.role,
            Role::Launcher
                | Role::ControlCenter
                | Role::Settings
                | Role::Popover
                | Role::Wallpapers
                | Role::Bar { .. }
                | Role::Dock
        ) && self.config.theme.blur
            && (self.config.theme.island_blur
                || !matches!(spec.role, Role::Bar { .. } | Role::Dock))
        {
            self.background_effect.get_background_effect(&wl, &qh).ok()
        } else {
            None
        };

        let id = wl.id();
        self.surfaces.insert(
            id,
            SurfaceEntry {
                render: Some(render),
                effect,
                layer: Some(layer),
                lock: None,
                wl_surface: wl,
                role: spec.role,
                output_id: spec.output_id,
                logical: (0, 0),
                requested_size: spec.size,
                scale,
                configured: false,
                frame_pending: false,
                needs_redraw: true,
                greeter_hits: Vec::new(),
                blur_rect: None,
            },
        );
        self.force_update = true;
        Ok(())
    }

    fn create_bar(&mut self, output: &wl_output::WlOutput, name: &str) -> Result<()> {
        if !self.config.bar.enabled {
            return Ok(());
        }
        let id = output.id();
        if self.surfaces.values().any(|entry| {
            matches!(entry.role, Role::Bar { .. }) && entry.output_id.as_ref() == Some(&id)
        }) {
            return Ok(());
        }
        let bar = &self.config.bar;
        let height = bar.height.ceil().max(20.0) as u32;
        let margin = bar.margin.round().max(0.0) as i32;
        let (anchor, margins) = match bar.position {
            lard_core::theme::BarPosition::Top => (
                Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
                (margin, margin, 0, margin),
            ),
            lard_core::theme::BarPosition::Bottom => (
                Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
                (0, margin, margin, margin),
            ),
        };
        self.create_layer(LayerSpec {
            role: Role::Bar {
                name: name.to_string(),
            },
            output: Some(output.clone()),
            output_id: Some(id),
            layer: Layer::Top,
            anchor,
            size: (0, height),
            keyboard: KeyboardInteractivity::None,
            exclusive_zone: (bar.height + bar.margin).round().max(1.0) as i32,
            margins,
            namespace: "lard-bar",
            input_none: false,
        })
    }

    fn focused_output(&self) -> Option<wl_output::WlOutput> {
        let name = self
            .shell
            .as_ref()
            .and_then(|shell| shell.niri.focused_output.clone());
        if let Some(name) = name
            && let Some(entry) = self.outputs.values().find(|output| output.name == name)
        {
            return Some(entry.output.clone());
        }
        self.outputs
            .values()
            .next()
            .map(|entry| entry.output.clone())
    }

    fn create_dock(&mut self, size: (u32, u32)) -> Result<()> {
        let output = self.focused_output();
        self.create_layer(LayerSpec {
            role: Role::Dock,
            output_id: output.as_ref().map(Proxy::id),
            output,
            layer: Layer::Top,
            anchor: Anchor::BOTTOM,
            size,
            keyboard: KeyboardInteractivity::None,
            exclusive_zone: -1,
            margins: (0, 0, self.config.dock.margin.round().max(0.0) as i32, 0),
            namespace: "lard-dock",
            input_none: false,
        })
    }

    fn resize_dock(&mut self, size: (u32, u32)) {
        for entry in self.surfaces.values_mut() {
            if matches!(entry.role, Role::Dock) && entry.requested_size != size {
                entry.requested_size = size;
                if let Some(layer) = &entry.layer {
                    layer.set_size(size.0, size.1);
                    layer.commit();
                }
            }
        }
    }

    fn create_launcher(&mut self) -> Result<()> {
        let output = self.focused_output();
        self.create_layer(LayerSpec {
            role: Role::Launcher,
            output_id: output.as_ref().map(Proxy::id),
            output,
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size: (0, 0),
            keyboard: KeyboardInteractivity::Exclusive,
            exclusive_zone: -1,
            margins: (0, 0, 0, 0),
            namespace: "lard-launcher",
            input_none: false,
        })
    }

    fn create_control_center(&mut self) -> Result<()> {
        let output = self.focused_output();
        self.create_layer(LayerSpec {
            role: Role::ControlCenter,
            output_id: output.as_ref().map(Proxy::id),
            output,
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size: (0, 0),
            keyboard: KeyboardInteractivity::Exclusive,
            exclusive_zone: -1,
            margins: (0, 0, 0, 0),
            namespace: "lard-control",
            input_none: false,
        })
    }

    fn popover_left_margin(&self, anchor_x: f32, width: f32) -> i32 {
        let margin = self.config.bar.margin.max(0.0);
        let screen_w = self
            .outputs
            .values()
            .find_map(|o| o.logical)
            .map_or(1920.0, |(w, _)| w as f32);
        let left = (anchor_x - width * 0.5).clamp(margin, (screen_w - margin - width).max(margin));
        left.round() as i32
    }

    fn create_popover(&mut self) -> Result<()> {
        let output = self.focused_output();
        self.create_layer(LayerSpec {
            role: Role::Popover,
            output_id: output.as_ref().map(Proxy::id),
            output,
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size: (0, 0),
            // The expanded Wi-Fi page has a password field, and the compact
            // cards still need Escape/arrow keys. The card is modal anyway.
            keyboard: KeyboardInteractivity::Exclusive,
            exclusive_zone: -1,
            margins: (0, 0, 0, 0),
            namespace: "lard-popover",
            input_none: false,
        })
    }

    fn create_wallpapers(&mut self) -> Result<()> {
        let output = self.focused_output();
        self.create_layer(LayerSpec {
            role: Role::Wallpapers,
            output_id: output.as_ref().map(Proxy::id),
            output,
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size: (0, 0),
            keyboard: KeyboardInteractivity::Exclusive,
            exclusive_zone: -1,
            margins: (0, 0, 0, 0),
            namespace: "lard-wallpapers",
            input_none: false,
        })
    }

    fn create_settings(&mut self) -> Result<()> {
        let output = self.focused_output();
        self.create_layer(LayerSpec {
            role: Role::Settings,
            output_id: output.as_ref().map(Proxy::id),
            output,
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size: (0, 0),
            keyboard: KeyboardInteractivity::Exclusive,
            exclusive_zone: -1,
            margins: (0, 0, 0, 0),
            namespace: "lard-settings",
            input_none: false,
        })
    }

    fn create_osd(&mut self, size: Size) -> Result<()> {
        let output = self.focused_output();
        let width = size.w.ceil().max(80.0) as u32;
        let height = size.h.ceil().max(24.0) as u32;
        self.create_layer(LayerSpec {
            role: Role::Osd,
            output_id: output.as_ref().map(Proxy::id),
            output,
            layer: Layer::Overlay,
            anchor: Anchor::BOTTOM,
            size: (width, height),
            keyboard: KeyboardInteractivity::None,
            exclusive_zone: -1,
            margins: (0, 0, self.config.osd.margin_bottom.round() as i32, 0),
            namespace: "lard-osd",
            input_none: true,
        })
    }

    fn create_toasts(&mut self, size: (u32, u32)) -> Result<()> {
        let output = self.focused_output();
        let margin = self.config.notifications.margin.round() as i32;
        let bar_offset = if self.config.bar.enabled {
            (self.config.bar.height + self.config.bar.margin)
                .round()
                .max(0.0) as i32
        } else {
            0
        };
        let top_margin = margin + bar_offset;
        let (anchor, margins) = match self.config.notifications.position {
            lard_core::theme::ToastPosition::TopRight => {
                (Anchor::TOP | Anchor::RIGHT, (top_margin, margin, 0, 0))
            }
            lard_core::theme::ToastPosition::TopLeft => {
                (Anchor::TOP | Anchor::LEFT, (top_margin, 0, 0, margin))
            }
            lard_core::theme::ToastPosition::BottomRight => {
                (Anchor::BOTTOM | Anchor::RIGHT, (0, margin, margin, 0))
            }
            lard_core::theme::ToastPosition::BottomLeft => {
                (Anchor::BOTTOM | Anchor::LEFT, (0, 0, margin, margin))
            }
        };
        self.create_layer(LayerSpec {
            role: Role::Toasts,
            output_id: output.as_ref().map(Proxy::id),
            output,
            layer: Layer::Overlay,
            anchor,
            size,
            keyboard: KeyboardInteractivity::None,
            exclusive_zone: -1,
            margins,
            namespace: "lard-notifications",
            input_none: false,
        })
    }

    /// The expanded notification island: an overlay layer surface anchored
    /// like the bar, so it floats above the windows without resizing them.
    fn create_notification(&mut self, size: (u32, u32)) -> Result<()> {
        let output = self.focused_output();
        let margin = self.config.bar.margin.round().max(0.0) as i32;
        self.create_layer(LayerSpec {
            role: Role::Notification,
            output_id: output.as_ref().map(Proxy::id),
            output,
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::LEFT | Anchor::RIGHT,
            size,
            keyboard: KeyboardInteractivity::None,
            exclusive_zone: -1,
            margins: (margin, margin, 0, margin),
            namespace: "lard-notification",
            input_none: true,
        })
    }

    fn resize_toasts(&mut self, size: (u32, u32)) {
        for entry in self.surfaces.values_mut() {
            if matches!(entry.role, Role::Toasts) && entry.requested_size != size {
                entry.requested_size = size;
                if let Some(layer) = &entry.layer {
                    layer.set_size(size.0, size.1);
                    layer.commit();
                }
            }
        }
    }

    fn create_greeter_surface_for(&mut self, output: &wl_output::WlOutput) -> Result<()> {
        let output_id = output.id();
        if self.surfaces.values().any(|entry| {
            matches!(entry.role, Role::Greeter) && entry.output_id.as_ref() == Some(&output_id)
        }) {
            return Ok(());
        }
        // One surface per output. Only the first requests exclusive keyboard
        // focus; the others are click-to-focus, avoiding competing Exclusive
        // surfaces on a single seat.
        let keyboard = if self.has_role(|role| matches!(role, Role::Greeter)) {
            KeyboardInteractivity::OnDemand
        } else {
            KeyboardInteractivity::Exclusive
        };
        self.create_layer(LayerSpec {
            role: Role::Greeter,
            output_id: Some(output_id),
            output: Some(output.clone()),
            layer: Layer::Overlay,
            anchor: Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT,
            size: (0, 0),
            keyboard,
            exclusive_zone: -1,
            margins: (0, 0, 0, 0),
            namespace: "lard-greeter",
            input_none: false,
        })
    }

    fn create_lock_surface_for(&mut self, output: &wl_output::WlOutput) {
        let Some(lock) = self.session_lock.as_ref().cloned() else {
            return;
        };
        let output_id = output.id();
        if self.surfaces.values().any(|entry| {
            matches!(entry.role, Role::Lock) && entry.output_id.as_ref() == Some(&output_id)
        }) {
            return;
        }
        let qh = self.qh.clone();
        let wl = self.compositor.create_surface(&qh);
        let lock_surface = lock.create_lock_surface(wl.clone(), output, &qh);
        let scale = self
            .outputs
            .get(&output_id)
            .map_or(1, |entry| entry.scale.max(1)) as f32;
        // ext-session-lock forbids committing before the first ack_configure.
        // SCTK acknowledges the initial configure for us and then calls
        // `resize_after_configure`; the first buffer is committed from
        // `draw_surface`.
        wl.set_buffer_scale(scale as i32);

        let display = self.conn.backend().display_ptr();
        // SAFETY: the lock surface and wl_surface outlive the wgpu surface.
        let render = match unsafe {
            RenderSurface::new(
                &self.gpu,
                display as *mut c_void,
                wl.id().as_ptr() as *mut c_void,
                (0, 0),
                scale,
            )
        } {
            Ok(render) => render,
            Err(error) => {
                tracing::error!(%error, "cannot create lock wgpu surface; unlocking to avoid an orphan lock");
                self.unlock_session();
                return;
            }
        };
        let id = wl.id();
        self.surfaces.insert(
            id,
            SurfaceEntry {
                render: Some(render),
                effect: None,
                layer: None,
                lock: Some(lock_surface),
                wl_surface: wl,
                role: Role::Lock,
                output_id: Some(output_id),
                logical: (0, 0),
                requested_size: (0, 0),
                scale,
                configured: false,
                frame_pending: false,
                needs_redraw: true,
                greeter_hits: Vec::new(),
                blur_rect: None,
            },
        );
        self.force_update = true;
    }

    fn has_configured_lock(&self) -> bool {
        self.surfaces
            .values()
            .any(|entry| matches!(entry.role, Role::Lock) && entry.configured)
    }

    fn start_session_lock(&mut self) -> Result<()> {
        let lock = self
            .session_lock_state
            .lock(&self.qh)
            .map_err(|error| anyhow::anyhow!("ext-session-lock unavailable: {error}"))?;
        self.session_lock = Some(lock);
        let _ = self.conn.flush();
        tracing::info!("session lock requested");
        Ok(())
    }

    fn unlock_session(&mut self) {
        self.unlocking = true;
        if let Some(lock) = self.session_lock.take() {
            lock.unlock();
        }
        let _ = self.conn.flush();
        self.exit = true;
    }

    fn check_greeter_finished(&mut self) {
        let finished = self.greeter.as_ref().is_some_and(GreeterView::is_finished);
        if !finished {
            return;
        }
        match self.mode {
            Mode::Lock => self.unlock_session(),
            Mode::Greeter { .. } => {
                tracing::info!("greeter authentication complete; exiting for greetd");
                self.exit = true;
            }
            Mode::Shell => {}
        }
    }

    fn handle_greeter_action(&mut self, action: GreeterAction) {
        match action {
            GreeterAction::Submit(request) => {
                if let Some(auth) = &self.auth {
                    auth.submit(request.username, request.password, request.session);
                }
            }
            GreeterAction::Power(action) => power::perform_async(action),
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    fn resize_after_configure(&mut self, id: &ObjectId, width: u32, height: u32) {
        let fallback = self
            .surfaces
            .get(id)
            .and_then(|entry| entry.output_id.as_ref())
            .and_then(|output_id| self.outputs.get(output_id))
            .and_then(|output| output.logical)
            .unwrap_or((width.max(1), height.max(1)));

        let Some(entry) = self.surfaces.get_mut(id) else {
            return;
        };
        let w = if width == 0 { fallback.0 } else { width };
        let h = if height == 0 { fallback.1 } else { height };
        if w == 0 || h == 0 {
            return;
        }
        entry.logical = (w, h);
        entry.configured = true;
        entry.needs_redraw = true;
        entry
            .wl_surface
            .set_buffer_scale(entry.scale.max(1.0) as i32);
        if let Some(render) = &mut entry.render {
            render.resize(&self.gpu, (w, h), entry.scale);
        }
        self.force_update = true;
        entry.blur_rect = None;
        entry.needs_redraw = true;
    }

    /// Restrict the compositor blur to the overlay panel shape. The region is
    /// rasterised as horizontal strips following the panel's corner radius:
    /// a plain rectangle would leak blurred pixels past rounded corners and
    /// look like a sharp square halo.
    fn clear_blur_rect(&mut self, id: &ObjectId) {
        if !self.blur_supported {
            return;
        }
        let Some(entry) = self.surfaces.get_mut(id) else {
            return;
        };
        if entry.blur_rect.is_none() {
            return;
        }
        if let Some(effect) = &entry.effect {
            effect.set_blur_region(None);
        }
        entry.blur_rect = None;
    }

    /// Restrict the compositor blur to one or more rounded regions. The bar
    /// uses one region per island, so the blur follows the floating panels
    /// instead of the whole fullscreen bar surface.
    fn set_blur_regions(&mut self, id: &ObjectId, regions: &[(Rect, f32)]) {
        if !self.blur_supported {
            return;
        }
        let mut next: Vec<[i32; 5]> = Vec::with_capacity(regions.len());
        for (rect, radius) in regions {
            let x = rect.x.floor().max(0.0) as i32;
            let y = rect.y.floor().max(0.0) as i32;
            let width = rect.w.round().max(0.0) as i32;
            let height = rect.h.round().max(0.0) as i32;
            if width < 2 || height < 2 {
                continue;
            }
            let radius = radius
                .max(0.0)
                .min(width as f32 * 0.5)
                .min(height as f32 * 0.5);
            next.push([x, y, width, height, (radius * 10.0).round() as i32]);
        }
        if next.is_empty() {
            self.clear_blur_rect(id);
            return;
        }
        let Some(entry) = self.surfaces.get_mut(id) else {
            return;
        };
        if entry.blur_rect.as_ref() == Some(&next) {
            return;
        }
        let Some(effect) = &entry.effect else {
            return;
        };
        let qh = self.qh.clone();
        let region = self.compositor.wl_compositor().create_region(&qh, ());
        for [x, y, width, height, radius] in &next {
            add_rounded_region(&region, *x, *y, *width, *height, *radius as f32 / 10.0);
        }
        effect.set_blur_region(Some(&region));
        region.destroy();
        entry.blur_rect = Some(next);
    }

    // ---- keyboard ----

    fn key_text(event: &KeyEvent) -> Option<String> {
        let text = event.utf8.as_deref()?;
        if text.is_empty() || text.chars().any(char::is_control) {
            return None;
        }
        Some(text.to_string())
    }

    fn map_greeter_key(&self, event: &KeyEvent) -> Option<GreeterKey> {
        match event.keysym {
            Keysym::Escape => Some(GreeterKey::Escape),
            Keysym::Return | Keysym::KP_Enter => Some(GreeterKey::Enter),
            Keysym::BackSpace => Some(GreeterKey::Backspace),
            Keysym::Tab => Some(GreeterKey::Tab {
                shift: self.modifiers.shift,
            }),
            Keysym::Up => Some(GreeterKey::Up),
            Keysym::Down => Some(GreeterKey::Down),
            Keysym::Left => Some(GreeterKey::Left),
            Keysym::Right => Some(GreeterKey::Right),
            _ if !self.modifiers.ctrl && !self.modifiers.alt && !self.modifiers.logo => {
                Self::key_text(event).map(GreeterKey::Char)
            }
            _ => None,
        }
    }

    fn map_shell_key(&self, event: &KeyEvent) -> Option<ShellKey> {
        match event.keysym {
            Keysym::Escape => Some(ShellKey::Escape),
            Keysym::Return | Keysym::KP_Enter => Some(ShellKey::Enter),
            Keysym::BackSpace => Some(ShellKey::Backspace),
            Keysym::Delete => Some(ShellKey::Delete),
            Keysym::Home => Some(ShellKey::Home),
            Keysym::End => Some(ShellKey::End),
            Keysym::Page_Up => Some(ShellKey::PageUp),
            Keysym::Page_Down => Some(ShellKey::PageDown),
            Keysym::Tab => Some(ShellKey::Tab),
            Keysym::Up => Some(ShellKey::Up),
            Keysym::Down => Some(ShellKey::Down),
            Keysym::Left => Some(ShellKey::Left),
            Keysym::Right => Some(ShellKey::Right),
            _ if !self.modifiers.ctrl && !self.modifiers.alt && !self.modifiers.logo => {
                Self::key_text(event).map(ShellKey::Char)
            }
            _ => None,
        }
    }

    fn handle_key(&mut self, event: &KeyEvent) {
        if self.greeter.is_some() {
            if let Some(key) = self.map_greeter_key(event)
                && let Some(greeter) = &mut self.greeter
                && let Some(action) = greeter.key(key)
            {
                self.handle_greeter_action(action);
            }
            self.force_update = true;
            return;
        }

        let role = self
            .keyboard_focus
            .as_ref()
            .and_then(|id| self.surfaces.get(id))
            .map(|entry| entry.role.clone());

        // On the screensaver the arrows move between scenarios; any other key
        // dismisses it. The keypress is swallowed either way, like a real
        // screensaver: it must not reach the application underneath.
        if matches!(role, Some(Role::Screensaver)) {
            match event.keysym {
                Keysym::Left | Keysym::Right => {
                    let delta = if event.keysym == Keysym::Left { -1 } else { 1 };
                    if let Some(shell) = &mut self.shell {
                        shell.screensaver_cycle(delta);
                    }
                }
                _ => {
                    if let Some(shell) = &mut self.shell {
                        shell.close_screensaver();
                    }
                }
            }
            self.force_update = true;
            self.mark_all_dirty();
            return;
        }

        let target = match role {
            Some(Role::Launcher) => Some(Role::Launcher),
            Some(Role::ControlCenter) => Some(Role::ControlCenter),
            Some(Role::Settings) => Some(Role::Settings),
            Some(Role::Popover) => Some(Role::Popover),
            Some(Role::Wallpapers) => Some(Role::Wallpapers),
            _ if self.shell.as_ref().is_some_and(Shell::launcher_open) => Some(Role::Launcher),
            _ if self.shell.as_ref().is_some_and(Shell::control_open) => Some(Role::ControlCenter),
            _ if self.shell.as_ref().is_some_and(Shell::popover_open) => Some(Role::Popover),
            _ => None,
        };

        match target {
            Some(Role::Launcher) => {
                if let Some(key) = self.map_shell_key(event)
                    && let Some(shell) = &mut self.shell
                    && let Some(action) = shell.launcher_key(key)
                {
                    self.perform(action);
                }
            }
            Some(Role::ControlCenter) => {
                if let Some(key) = self.map_shell_key(event)
                    && let Some(shell) = &mut self.shell
                    && let Some(action) = shell.control_key(key)
                {
                    self.perform(action);
                }
            }
            Some(Role::Popover) => {
                if let Some(key) = self.map_shell_key(event)
                    && let Some(shell) = &mut self.shell
                {
                    shell.popover_key(key);
                }
            }
            Some(Role::Settings) => {
                if self.modifiers.ctrl && event.keysym == Keysym::s {
                    if let Some(shell) = &mut self.shell {
                        shell.settings_save_niri();
                    }
                } else if let Some(key) = self.map_shell_key(event)
                    && let Some(shell) = &mut self.shell
                {
                    shell.settings_key(key);
                }
            }
            Some(Role::Wallpapers) => {
                if let Some(shell) = &mut self.shell {
                    shell.wallpaper_key(
                        event.keysym == Keysym::Escape,
                        event.keysym == Keysym::Left,
                        event.keysym == Keysym::Right,
                        matches!(event.keysym, Keysym::Return | Keysym::KP_Enter),
                    );
                }
            }
            _ => {}
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    // ---- pointer ----

    fn pointer_move(&mut self, role: Role, pos: Point) {
        match role {
            Role::Bar { name } => {
                if let Some(shell) = &mut self.shell {
                    shell.bar_move(&name, pos);
                }
            }
            // The notification island is not interactive: the clicks fall
            // through to the bar underneath.
            Role::Notification => {}
            Role::Screensaver => {}
            Role::Dock => {
                if let Some(shell) = &mut self.shell {
                    shell.dock_move(pos);
                }
            }
            Role::Launcher => {
                if let Some(shell) = &mut self.shell {
                    shell.launcher_move(pos);
                }
            }
            Role::ControlCenter => {
                if let Some(shell) = &mut self.shell {
                    shell.control_move(pos);
                }
            }
            Role::Settings => {
                if let Some(shell) = &mut self.shell {
                    shell.settings_move(pos);
                }
            }
            Role::Popover => {
                let action = self
                    .shell
                    .as_mut()
                    .and_then(|shell| shell.popover_move(pos));
                if let Some(action) = action {
                    self.perform(action);
                }
            }
            Role::Wallpapers => {
                if let Some(shell) = &mut self.shell {
                    shell.wallpaper_move(pos);
                }
            }
            Role::Toasts => {
                if let Some(shell) = &mut self.shell {
                    shell.toast_move(pos);
                }
            }
            Role::Greeter | Role::Lock => {
                if let Some(greeter) = &mut self.greeter {
                    greeter.pointer_move(pos);
                }
            }
            Role::Osd => {}
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    /// Button released: end a slider drag in the popover and apply its final
    /// value (the drag throttle may have dropped the last motion event).
    fn pointer_release(&mut self, role: Role) {
        if !matches!(role, Role::Popover) {
            return;
        }
        let action = self
            .shell
            .as_mut()
            .and_then(|shell| shell.popover_release());
        if let Some(action) = action {
            self.perform(action);
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    fn pointer_leave(&mut self, role: Role) {
        match role {
            Role::Bar { name } => {
                if let Some(shell) = &mut self.shell {
                    shell.bar_leave(&name);
                }
            }
            Role::Notification => {}
            Role::Screensaver => {}
            Role::Dock => {
                if let Some(shell) = &mut self.shell {
                    shell.dock_leave();
                }
            }
            Role::Launcher => {
                if let Some(shell) = &mut self.shell {
                    shell.launcher_leave();
                }
            }
            Role::ControlCenter => {
                if let Some(shell) = &mut self.shell {
                    shell.control_leave();
                }
            }
            Role::Settings => {
                if let Some(shell) = &mut self.shell {
                    shell.settings_leave();
                }
            }
            Role::Popover => {
                if let Some(shell) = &mut self.shell {
                    shell.popover_leave();
                }
            }
            Role::Wallpapers => {
                if let Some(shell) = &mut self.shell {
                    shell.wallpaper_leave();
                }
            }
            Role::Toasts => {
                if let Some(shell) = &mut self.shell {
                    shell.toast_leave();
                }
            }
            Role::Greeter | Role::Lock => {
                if let Some(greeter) = &mut self.greeter {
                    greeter.pointer_leave();
                }
            }
            Role::Osd => {}
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    fn pointer_press(&mut self, role: Role, pos: Point, button: u32, surface_id: &ObjectId) {
        let is_bar = matches!(role, Role::Bar { .. });
        let action = if button == BTN_LEFT {
            match role {
                Role::Bar { name } => self
                    .shell
                    .as_mut()
                    .and_then(|shell| shell.bar_press(&name, pos)),
                Role::Dock => {
                    if let Some(shell) = &mut self.shell {
                        shell.dock_press(pos)
                    } else {
                        None
                    }
                }
                Role::Notification => None,
                Role::Screensaver => {
                    if let Some(shell) = &mut self.shell {
                        shell.close_screensaver();
                    }
                    None
                }
                Role::Launcher => {
                    if let Some(shell) = &mut self.shell {
                        shell.launcher_press(pos)
                    } else {
                        None
                    }
                }
                Role::ControlCenter => {
                    if let Some(shell) = &mut self.shell {
                        shell.control_press(pos)
                    } else {
                        None
                    }
                }
                Role::Settings => {
                    if let Some(shell) = &mut self.shell {
                        shell.settings_press(pos);
                    }
                    None
                }
                Role::Popover => {
                    if let Some(shell) = &mut self.shell {
                        shell.popover_press(pos);
                    }
                    None
                }
                Role::Wallpapers => {
                    if let Some(shell) = &mut self.shell {
                        shell.wallpaper_press(pos);
                    }
                    None
                }
                Role::Toasts => {
                    if let Some(shell) = &mut self.shell {
                        shell.toast_press(pos)
                    } else {
                        None
                    }
                }
                Role::Greeter | Role::Lock => {
                    let hits = self
                        .surfaces
                        .get(surface_id)
                        .map(|entry| entry.greeter_hits.clone())
                        .unwrap_or_default();
                    if let Some(greeter) = &mut self.greeter {
                        if let Some(action) = greeter.pointer_press(pos, &hits) {
                            self.handle_greeter_action(action);
                        }
                    }
                    None
                }
                Role::Osd => None,
            }
        } else {
            None
        };

        if let Some(action) = action {
            self.perform(action);
        }
        if button == BTN_RIGHT
            && is_bar
            && let Some(shell) = &mut self.shell
        {
            shell.toggle_control();
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    fn pointer_axis(&mut self, role: Role, pos: Point, vertical: &AxisScroll) {
        let delta = if vertical.absolute.abs() > f64::EPSILON {
            vertical.absolute
        } else {
            f64::from(vertical.value120) / 120.0
        };
        if delta == 0.0 {
            return;
        }
        match role {
            Role::Launcher => {
                if let Some(shell) = &mut self.shell {
                    shell.launcher_axis(delta);
                }
            }
            Role::Bar { name } => {
                let action = self
                    .shell
                    .as_mut()
                    .and_then(|shell| shell.bar_axis(&name, pos, -delta));
                if let Some(action) = action {
                    self.perform(action);
                }
            }
            Role::ControlCenter => {
                if let Some(shell) = &mut self.shell {
                    shell.control_axis(delta as f32);
                }
            }
            Role::Popover => {
                let action = self
                    .shell
                    .as_mut()
                    .and_then(|shell| shell.popover_axis(delta as f32));
                if let Some(action) = action {
                    self.perform(action);
                }
            }
            _ => {}
        }
        self.force_update = true;
        self.mark_all_dirty();
    }

    fn on_pointer_event(&mut self, event: &PointerEvent) {
        let id = event.surface.id();
        let Some(role) = self.surfaces.get(&id).map(|entry| entry.role.clone()) else {
            return;
        };
        let pos = Point::new(event.position.0 as f32, event.position.1 as f32);
        match &event.kind {
            PointerEventKind::Motion { .. } if matches!(&role, Role::Screensaver) => {
                // Moving the pointer wakes the session; the Enter that follows
                // the surface creation must not.
                if let Some(shell) = &mut self.shell {
                    shell.close_screensaver();
                }
                self.force_update = true;
                self.mark_all_dirty();
            }
            PointerEventKind::Enter { .. } => self.pointer_move(role, pos),
            PointerEventKind::Leave { .. } => self.pointer_leave(role),
            PointerEventKind::Motion { .. } => self.pointer_move(role, pos),
            PointerEventKind::Press { button, .. } => {
                if *button == BTN_LEFT || *button == BTN_RIGHT {
                    self.pointer_press(role, pos, *button, &id);
                }
            }
            PointerEventKind::Release { .. } => self.pointer_release(role),
            PointerEventKind::Axis { vertical, .. } => {
                self.pointer_axis(role, pos, vertical);
            }
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let id = surface.id();
        let scale = new_factor.max(1) as f32;
        let Some(entry) = self.surfaces.get_mut(&id) else {
            return;
        };
        if (entry.scale - scale).abs() < f32::EPSILON {
            return;
        }
        entry.scale = scale;
        entry.needs_redraw = true;
        entry.wl_surface.set_buffer_scale(new_factor.max(1));
        if entry.logical.0 > 0
            && entry.logical.1 > 0
            && let Some(render) = &mut entry.render
        {
            render.resize(&self.gpu, entry.logical, scale);
        }
        self.force_update = true;
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if let Some(entry) = self.surfaces.get_mut(&surface.id()) {
            entry.frame_pending = false;
        }
        self.callback_fired = true;
        self.force_update = true;
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let id = output.id();
        let info = self.output_state.info(&output);
        let name = info
            .as_ref()
            .and_then(|info| info.name.clone())
            .unwrap_or_else(|| format!("output-{}", id.protocol_id()));
        let scale = info.as_ref().map_or(1, |info| info.scale_factor.max(1));
        let logical = info
            .as_ref()
            .and_then(|info| info.logical_size)
            .map(|(w, h)| (w.max(1) as u32, h.max(1) as u32));
        self.outputs.insert(
            id.clone(),
            OutputEntry {
                output: output.clone(),
                name: name.clone(),
                scale,
                logical,
            },
        );

        match self.mode {
            Mode::Shell => {
                if let Err(error) = self.create_bar(&output, &name) {
                    tracing::error!(%error, "cannot create bar");
                }
            }
            Mode::Greeter { .. } => {
                if let Err(error) = self.create_greeter_surface_for(&output) {
                    tracing::error!(%error, "cannot create greeter surface");
                }
            }
            Mode::Lock => {
                if self.locked {
                    self.create_lock_surface_for(&output);
                }
            }
        }
        self.force_update = true;
    }

    fn update_output(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let id = output.id();
        let info = self.output_state.info(&output);
        if let Some(entry) = self.outputs.get_mut(&id) {
            if let Some(info) = &info {
                if let Some(name) = &info.name {
                    entry.name.clone_from(name);
                }
                entry.logical = info
                    .logical_size
                    .map(|(w, h)| (w.max(1) as u32, h.max(1) as u32));
                let scale = info.scale_factor.max(1);
                if entry.scale != scale {
                    entry.scale = scale;
                    let scale_f = scale as f32;
                    for surface in self.surfaces.values_mut() {
                        if surface.output_id.as_ref() == Some(&id) {
                            surface.scale = scale_f;
                            surface.wl_surface.set_buffer_scale(scale);
                            if surface.logical.0 > 0
                                && let Some(render) = &mut surface.render
                            {
                                render.resize(&self.gpu, surface.logical, scale_f);
                            }
                            surface.needs_redraw = true;
                        }
                    }
                }
            }
        }
        self.force_update = true;
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let id = output.id();
        self.outputs.remove(&id);
        let surfaces: Vec<ObjectId> = self
            .surfaces
            .iter()
            .filter(|(_, entry)| entry.output_id.as_ref() == Some(&id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in surfaces {
            self.surfaces.remove(&id);
        }
        self.force_update = true;
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        self.seat = Some(seat.clone());
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            match self.seat_state.get_keyboard(qh, &seat, None) {
                Ok(keyboard) => self.keyboard = Some(keyboard),
                Err(error) => tracing::error!(%error, "cannot create keyboard"),
            }
            // Idle notifications are tied to a seat: this is the first moment
            // the screensaver timer can be armed.
            self.arm_screensaver_idle();
        }
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                Err(error) => tracing::error!(%error, "cannot create pointer"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Keyboard
            && let Some(keyboard) = self.keyboard.take()
        {
            keyboard.release();
        }
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: wl_seat::WlSeat) {
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[Keysym],
    ) {
        self.keyboard_focus = Some(surface.id());
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        if self.keyboard_focus.as_ref() == Some(&surface.id()) {
            self.keyboard_focus = None;
        }
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key(&event);
    }

    fn repeat_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.handle_key(&event);
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _event: KeyEvent,
    ) {
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        modifiers: Modifiers,
        _raw_modifiers: RawModifiers,
        _layout: u32,
    ) {
        self.modifiers = modifiers;
    }
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            self.on_pointer_event(event);
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        let id = layer.wl_surface().id();
        self.surfaces.remove(&id);
        self.force_update = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let id = layer.wl_surface().id();
        let (width, height) = configure.new_size;
        self.resize_after_configure(&id, width, height);
    }
}

impl SessionLockHandler for App {
    fn locked(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _session_lock: SessionLock) {
        self.locked = true;
        tracing::info!("session locked");
        let outputs: Vec<wl_output::WlOutput> = self.output_state.outputs().collect();
        for output in outputs {
            self.create_lock_surface_for(&output);
        }
        // Fail open instead of leaving the compositor in an unlockable solid
        // color if the compositor never sends a usable lock-surface configure.
        let inserted = self.loop_handle.insert_source(
            Timer::from_deadline(Instant::now() + Duration::from_secs(3)),
            |_, _, app: &mut App| {
                if app.locked && !app.has_configured_lock() {
                    tracing::error!("lock surfaces were not configured; unlocking");
                    app.unlock_session();
                }
                TimeoutAction::Drop
            },
        );
        if inserted.is_err() {
            tracing::error!("cannot arm lock watchdog");
        }
        self.force_update = true;
    }

    fn finished(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _session_lock: SessionLock,
    ) {
        if self.unlocking {
            self.exit = true;
        } else {
            tracing::error!("session lock was rejected or taken over");
            self.exit = true;
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        session_lock_surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        let id = session_lock_surface.wl_surface().id();
        let (width, height) = configure.new_size;
        self.resize_after_configure(&id, width, height);
    }
}

/// Rasterise a rounded rectangle into a `wl_region` as horizontal strips.
/// Each corner is approximated with a few bands, enough to remove the sharp
/// blur halo at normal UI radii while keeping the region cheap.
fn add_rounded_region(
    region: &wayland_client::protocol::wl_region::WlRegion,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    radius: f32,
) {
    for r in rounded_region_rects(x, y, width, height, radius) {
        region.add(r[0], r[1], r[2], r[3]);
    }
}

/// Pure geometry for a rounded `wl_region`: a stack of horizontal strips that
/// approximate the four rounded corners. Kept separate from the protocol code
/// so it can be unit-tested.
fn rounded_region_rects(x: i32, y: i32, width: i32, height: i32, radius: f32) -> Vec<[i32; 4]> {
    let mut rects = Vec::with_capacity(32);
    let w = width.max(1);
    let h = height.max(1);
    let r = radius.max(0.0).min(w as f32 * 0.5).min(h as f32 * 0.5);
    if r < 1.0 {
        rects.push([x, y, w, h]);
        return rects;
    }
    let r_i = r.ceil() as i32;
    // One-logical-pixel bands keep the staircase at the rounded corners
    // below the visible threshold; the band count stays bounded for huge
    // radii so the compositor's damage filtering remains cheap.
    let bands = r_i.clamp(4, 48);
    let band_h = (r_i + bands - 1) / bands;
    let mid_top = r_i.min(h / 2);
    let mid_bottom = (h - r_i).max(mid_top);
    if mid_bottom > mid_top {
        rects.push([x, y + mid_top, w, mid_bottom - mid_top]);
    }

    // Top corners: circle centre at (r, r).
    let mut yy = 0;
    while yy < mid_top {
        let y1 = (yy + band_h).min(mid_top);
        let yc = (yy + y1) as f32 * 0.5;
        let dy = r - yc;
        let inset = (r - (r * r - dy * dy).max(0.0).sqrt()).round().max(0.0) as i32;
        let iw = w - inset * 2;
        if iw > 0 && y1 > yy {
            rects.push([x + inset, y + yy, iw, y1 - yy]);
        }
        yy = y1;
    }

    // Bottom corners: circle centre at (r, h - r).
    let mut yy = mid_bottom;
    while yy < h {
        let y1 = (yy + band_h).min(h);
        let yc = (yy + y1) as f32 * 0.5;
        let dy = yc - (h as f32 - r);
        let inset = (r - (r * r - dy * dy).max(0.0).sqrt()).round().max(0.0) as i32;
        let iw = w - inset * 2;
        if iw > 0 && y1 > yy {
            rects.push([x + inset, y + yy, iw, y1 - yy]);
        }
        yy = y1;
    }
    rects
}

#[cfg(test)]
mod blur_region_tests {
    use super::rounded_region_rects;

    #[test]
    fn square_when_radius_is_zero() {
        let rects = rounded_region_rects(10, 20, 300, 200, 0.0);
        assert_eq!(rects, vec![[10, 20, 300, 200]]);
    }

    #[test]
    fn rounded_region_is_inset_at_corners() {
        let rects = rounded_region_rects(0, 0, 300, 200, 18.0);
        assert!(rects.len() > 3);
        assert_eq!(rects[0], [0, 18, 300, 164]);
        for &[x, y, w, h] in &rects {
            assert!(x >= 0 && y >= 0 && w > 0 && h > 0);
            assert!(x + w <= 300 && y + h <= 200);
        }
        // The topmost strip must be narrower than the middle, i.e. corners
        // really are rounded instead of the whole rect being blurred.
        let top = rects.iter().min_by_key(|r| r[1]).unwrap();
        assert!(top[2] < 300);
    }

    #[test]
    fn radius_is_clamped_to_half_size() {
        let rects = rounded_region_rects(0, 0, 40, 20, 999.0);
        for &[x, y, w, h] in &rects {
            assert!(w > 0 && h > 0);
            assert!(x >= 0 && y >= 0 && x + w <= 40 && y + h <= 20);
        }
    }
}

impl BackgroundEffectHandler for App {
    fn background_effect_state(&mut self) -> &mut BackgroundEffectState {
        &mut self.background_effect
    }

    fn update_capabilities(&mut self) {
        self.blur_supported = self
            .background_effect
            .capabilities()
            .is_some_and(|capabilities| {
                capabilities.contains(
                    smithay_client_toolkit::reexports::protocols::ext::background_effect::v1::client::ext_background_effect_manager_v1::Capability::Blur,
                )
            });
        if self.blur_supported {
            tracing::info!("compositor-side backdrop blur available");
            for entry in self.surfaces.values_mut() {
                entry.blur_rect = None;
                entry.needs_redraw = true;
            }
            self.force_update = true;
        }
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

wayland_client::delegate_noop!(App: ignore wl_region::WlRegion);
smithay_client_toolkit::delegate_registry!(App);
impl Dispatch2<ExtIdleNotifierV1, App> for IdleData {
    fn event(
        &self,
        _app: &mut App,
        _proxy: &ExtIdleNotifierV1,
        _event: <ExtIdleNotifierV1 as Proxy>::Event,
        _conn: &Connection,
        _qh: &QueueHandle<App>,
    ) {
    }
}

impl Dispatch2<ExtIdleNotificationV1, App> for IdleData {
    fn event(
        &self,
        app: &mut App,
        _proxy: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        _conn: &Connection,
        _qh: &QueueHandle<App>,
    ) {
        tracing::debug!(?event, "idle notification event");
        match event {
            ext_idle_notification_v1::Event::Idled => {
                if let Some(shell) = &mut app.shell
                    && shell.open_screensaver()
                {
                    tracing::info!("session idle: showing the screensaver");
                }
                app.force_update = true;
                app.mark_all_dirty();
            }
            ext_idle_notification_v1::Event::Resumed => {
                // Input resumed, but the screensaver closes on its own input
                // handlers: the arrow keys are user input too, and closing here
                // would make the scenarios impossible to browse.
                tracing::debug!("session active");
            }
            _ => {}
        }
    }
}

smithay_client_toolkit::delegate_dispatch2!(App);
