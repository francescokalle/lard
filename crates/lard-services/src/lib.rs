//! System integration, isolated from rendering.
//!
//! Every service pushes changes into a `calloop` channel; the UI thread never
//! blocks on a syscall, D-Bus call or subprocess. Services are event-driven
//! where the system exposes an event source, with conservative polling as a
//! fallback (battery: 30 s, network: 15 s, bluetooth: 30 s).

pub mod audio;
pub mod battery;
pub mod bluetooth;
pub mod brightness;
pub mod network;
pub mod niri;
pub mod notifications;
pub mod power;
pub mod system;

use calloop::channel::Sender as CalloopSender;
use niri_ipc::Request;
use std::sync::mpsc::Sender as StdSender;
use tokio::sync::mpsc::UnboundedSender;

pub use bluetooth::BluetoothDevice;
pub use network::WifiNetwork;
pub use niri::{NiriSnapshot, WindowView, WorkspaceView};
pub use notifications::{CloseReason, DaemonCommand, Notification, NotificationAction};

#[derive(Clone, Debug)]
pub enum ServiceEvent {
    Niri(NiriSnapshot),
    /// The connected outputs and their modes.
    Outputs(Vec<niri::OutputView>),
    Volume {
        percent: u8,
        muted: bool,
    },
    Battery(battery::BatteryState),
    Brightness(brightness::BrightnessState),
    Network(network::NetworkState),
    WifiEnabled(bool),
    WifiNetworks(Vec<WifiNetwork>),
    WifiScanError(String),
    WifiConnectResult {
        ssid: String,
        ok: bool,
        message: Option<String>,
    },
    Bluetooth(bluetooth::BluetoothState),
    BluetoothDevices(Vec<BluetoothDevice>),
    BluetoothScanError(String),
    BluetoothConnectResult {
        address: String,
        ok: bool,
        message: Option<String>,
    },
    SystemStats(system::SystemStats),
    Notification(Notification),
    NotificationClosed {
        id: u32,
        reason: CloseReason,
    },
}

/// Handles to every running service.
pub struct Services {
    niri_requests: StdSender<Request>,
    notifications: Option<UnboundedSender<DaemonCommand>>,
    ui: CalloopSender<ServiceEvent>,
}

impl Services {
    pub fn start(
        ui: CalloopSender<ServiceEvent>,
        notification_timeout_ms: i32,
        notifications_enabled: bool,
    ) -> Self {
        let niri_requests = niri::spawn(ui.clone());
        audio::spawn(ui.clone());
        battery::spawn(ui.clone());
        brightness::spawn(ui.clone());
        system::spawn(ui.clone());
        network::spawn(ui.clone());
        bluetooth::spawn(ui.clone());
        let notifications = if notifications_enabled {
            notifications::spawn(ui.clone(), notification_timeout_ms)
        } else {
            None
        };
        Self {
            niri_requests,
            notifications,
            ui,
        }
    }

    /// Fire-and-forget niri IPC request (workspace click, launch, …).
    pub fn send_niri(&self, request: Request) {
        if self.niri_requests.send(request).is_err() {
            tracing::warn!("niri request worker is gone");
        }
    }

    /// Ask niri for the connected outputs and their modes.
    pub fn request_outputs(&self) {
        self.send_niri(Request::Outputs);
    }

    pub fn focus_workspace(&self, id: u64) {
        self.send_niri(Request::Action(niri_ipc::Action::FocusWorkspace {
            reference: niri_ipc::WorkspaceReferenceArg::Id(id),
        }));
    }

    pub fn spawn_command(&self, command: Vec<String>) {
        self.send_niri(Request::Action(niri_ipc::Action::Spawn { command }));
    }

    pub fn volume_step(&self, delta_percent: i32) {
        spawn_action(move || audio::step_volume(delta_percent));
    }

    pub fn set_volume(&self, percent: u8) {
        spawn_action(move || audio::set_volume(percent));
    }

    pub fn toggle_mute(&self) {
        spawn_action(audio::toggle_mute);
    }

    pub fn close_notification(&self, id: u32, reason: CloseReason) {
        if let Some(tx) = &self.notifications {
            let _ = tx.send(DaemonCommand::Close {
                id,
                reason: reason.as_u32(),
            });
        }
    }

    pub fn invoke_action(&self, id: u32, action: String) {
        if let Some(tx) = &self.notifications {
            let _ = tx.send(DaemonCommand::Action { id, action });
        }
    }

    pub fn toggle_wifi(&self, enabled: bool) {
        spawn_action(move || network::set_wifi(enabled));
    }

    pub fn scan_wifi(&self) {
        let ui = self.ui.clone();
        spawn_action(move || network::scan(&ui));
    }

    pub fn connect_wifi(&self, ssid: String, password: Option<String>) {
        let ui = self.ui.clone();
        spawn_action(move || network::connect(&ui, ssid, password));
    }

    pub fn disconnect_wifi(&self) {
        let ui = self.ui.clone();
        spawn_action(move || network::disconnect(&ui));
    }

    pub fn toggle_bluetooth(&self, enabled: bool) {
        spawn_action(move || bluetooth::set_powered(enabled));
    }

    pub fn refresh_bluetooth(&self) {
        let ui = self.ui.clone();
        spawn_action(move || bluetooth::refresh(&ui));
    }

    pub fn scan_bluetooth(&self) {
        let ui = self.ui.clone();
        spawn_action(move || bluetooth::scan(&ui));
    }

    pub fn connect_bluetooth(&self, address: String) {
        let ui = self.ui.clone();
        spawn_action(move || bluetooth::connect(&ui, address));
    }

    pub fn disconnect_bluetooth(&self, address: String) {
        let ui = self.ui.clone();
        spawn_action(move || bluetooth::disconnect(&ui, address));
    }

    pub fn brightness_step(&self, delta: i32) {
        let ui = self.ui.clone();
        spawn_action(move || brightness::step(delta, ui));
    }

    pub fn set_brightness(&self, percent: u8) {
        let ui = self.ui.clone();
        spawn_action(move || brightness::set(percent, ui));
    }
}

/// Commands such as `nmcli`, `wpctl` or `systemctl` can take tens of
/// milliseconds. They must never run on the Wayland/render thread.
fn spawn_action(action: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("lard-action".into())
        .spawn(action);
}
