//! niri IPC: event-stream state + a tiny request worker.
//!
//! niri's event socket is read-only after `EventStream`, so UI actions use a
//! second connection in a dedicated thread. Requests are rare (a click), never
//! per-frame.

use crate::ServiceEvent;
use calloop::channel::Sender as CalloopSender;
use niri_ipc::socket::Socket;
use niri_ipc::state::{EventStreamState, EventStreamStatePart};
use niri_ipc::{Action, Request, Response, WorkspaceReferenceArg};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkspaceView {
    pub id: u64,
    pub idx: u8,
    pub name: Option<String>,
    pub output: Option<String>,
    pub is_active: bool,
    pub is_focused: bool,
    pub is_urgent: bool,
    pub has_windows: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowView {
    pub id: u64,
    pub title: String,
    pub app_id: String,
}

/// A connected output with the modes it supports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutputView {
    pub name: String,
    /// The available modes, "1920x1080@120.030".
    pub modes: Vec<String>,
    pub current_mode: Option<String>,
    pub scale: f64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NiriSnapshot {
    pub connected: bool,
    pub workspaces: Vec<WorkspaceView>,
    pub focused_window: Option<WindowView>,
    /// All mapped windows, used by the dock for running indicators.
    pub windows: Vec<WindowView>,
    pub keyboard_layout: Option<String>,
    pub focused_output: Option<String>,
}

impl NiriSnapshot {
    #[must_use]
    pub fn focused_workspace(&self) -> Option<&WorkspaceView> {
        self.workspaces.iter().find(|w| w.is_focused)
    }
}

pub fn spawn(ui: CalloopSender<ServiceEvent>) -> mpsc::Sender<Request> {
    let (request_tx, request_rx) = mpsc::channel::<Request>();

    let events_ui = ui.clone();
    std::thread::Builder::new()
        .name("niri-events".into())
        .spawn(move || event_loop(events_ui))
        .expect("failed to spawn niri events thread");

    std::thread::Builder::new()
        .name("niri-requests".into())
        .spawn(move || request_loop(request_rx, ui))
        .expect("failed to spawn niri requests thread");

    request_tx
}

fn event_loop(ui: CalloopSender<ServiceEvent>) {
    let mut last = NiriSnapshot::default();
    let _ = ui.send(ServiceEvent::Niri(last.clone()));

    loop {
        let mut socket = match connect() {
            Ok(socket) => socket,
            Err(error) => {
                if last.connected || last.workspaces.is_empty() {
                    last = NiriSnapshot::default();
                    let _ = ui.send(ServiceEvent::Niri(last.clone()));
                }
                tracing::debug!(%error, "niri IPC unavailable, retrying");
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };

        let stream = match socket.send(Request::EventStream) {
            Ok(Ok(Response::Handled)) => socket.read_events(),
            Ok(Err(message)) => {
                tracing::warn!(%message, "niri refused the event stream");
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            Ok(Ok(other)) => {
                tracing::warn!(?other, "unexpected niri event stream reply");
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
            Err(error) => {
                tracing::warn!(%error, "cannot request niri event stream");
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        };

        let mut stream = stream;
        let mut state = EventStreamState::default();
        // The first events are full Workspaces/Windows snapshots.
        loop {
            match stream() {
                Ok(event) => {
                    let _ = state.apply(event);
                    let snapshot = snapshot(&state);
                    if snapshot != last {
                        last = snapshot.clone();
                        if ui.send(ServiceEvent::Niri(snapshot)).is_err() {
                            return;
                        }
                    }
                }
                Err(error) => {
                    tracing::debug!(%error, "niri event stream closed");
                    break;
                }
            }
        }

        last = NiriSnapshot::default();
        let _ = ui.send(ServiceEvent::Niri(last.clone()));
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn snapshot(state: &EventStreamState) -> NiriSnapshot {
    let mut workspaces: Vec<WorkspaceView> = state
        .workspaces
        .workspaces
        .values()
        .map(|w| WorkspaceView {
            id: w.id,
            idx: w.idx,
            name: w.name.clone(),
            output: w.output.clone(),
            is_active: w.is_active,
            is_focused: w.is_focused,
            is_urgent: w.is_urgent,
            has_windows: w.active_window_id.is_some(),
        })
        .collect();
    workspaces.sort_by(|a, b| {
        (a.output.as_deref().unwrap_or(""), a.idx).cmp(&(b.output.as_deref().unwrap_or(""), b.idx))
    });

    let focused_window = state
        .windows
        .windows
        .values()
        .find(|w| w.is_focused)
        .map(|w| WindowView {
            id: w.id,
            title: w.title.clone().unwrap_or_default(),
            app_id: w.app_id.clone().unwrap_or_default(),
        });

    let mut windows: Vec<WindowView> = state
        .windows
        .windows
        .values()
        .map(|w| WindowView {
            id: w.id,
            title: w.title.clone().unwrap_or_default(),
            app_id: w.app_id.clone().unwrap_or_default(),
        })
        .collect();
    windows.sort_by_key(|w| w.id);

    let focused_output = workspaces
        .iter()
        .find(|w| w.is_focused)
        .and_then(|w| w.output.clone());

    let keyboard_layout = state
        .keyboard_layouts
        .keyboard_layouts
        .as_ref()
        .and_then(|layouts| layouts.names.get(layouts.current_idx as usize).cloned());

    NiriSnapshot {
        connected: true,
        workspaces,
        focused_window,
        windows,
        keyboard_layout,
        focused_output,
    }
}

fn request_loop(rx: mpsc::Receiver<Request>, ui: CalloopSender<ServiceEvent>) {
    while let Ok(request) = rx.recv() {
        if matches!(request, Request::Outputs) {
            match fetch_outputs() {
                Ok(outputs) => {
                    let _ = ui.send(ServiceEvent::Outputs(outputs));
                }
                Err(error) => tracing::debug!(%error, "cannot read the niri outputs"),
            }
            continue;
        }
        if let Err(error) = send_request(request) {
            tracing::warn!(%error, "niri request failed");
        }
    }
}

/// Connect to niri, discovering its socket when the environment does not
/// point at one.
///
/// niri names the socket after the Wayland display and, when the plain name is
/// already taken, appends its own pid (`niri.wayland-1.1234.sock`). A shell
/// started from systemd keeps a stale `NIRI_SOCKET` across a niri restart, so
/// the path is checked and re-discovered on every connection.
fn connect() -> io::Result<Socket> {
    Socket::connect_to(socket_path()?)
}

/// Path of niri's IPC socket: the configured one while it exists, otherwise
/// the most recent `niri.*.sock` in the runtime directory.
fn socket_path() -> io::Result<PathBuf> {
    if let Some(path) = std::env::var_os("NIRI_SOCKET") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path);
        }
    }
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR is not set"))?;
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(runtime)?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("niri.") || !name.ends_with(".sock") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
            newest = Some((modified, entry.path()));
        }
    }
    newest.map(|(_, path)| path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "no niri IPC socket in XDG_RUNTIME_DIR",
        )
    })
}

/// Export the discovered socket so child processes (`niri msg`) and the
/// service threads inherit a path that is still valid after a niri restart.
///
/// Called once during startup, before any thread is spawned.
pub fn export_socket_env() {
    match socket_path() {
        Ok(path) => unsafe {
            std::env::set_var("NIRI_SOCKET", path);
        },
        Err(error) => tracing::debug!(%error, "niri IPC socket not found at startup"),
    }
}

/// Ask niri for the connected outputs and their modes.
fn fetch_outputs() -> io::Result<Vec<OutputView>> {
    let mut socket = connect()?;
    let outputs = match socket.send(Request::Outputs)? {
        Ok(Response::Outputs(outputs)) => outputs,
        Ok(_) | Err(_) => return Ok(Vec::new()),
    };
    Ok(outputs
        .into_values()
        .map(|output| {
            let modes: Vec<String> = output.modes.iter().map(mode_label).collect();
            let current_mode = output
                .current_mode
                .and_then(|index| output.modes.get(index))
                .map(mode_label);
            let scale = output
                .logical
                .as_ref()
                .map(|logical| logical.scale)
                .unwrap_or(1.0);
            OutputView {
                name: output.name,
                modes,
                current_mode,
                scale,
            }
        })
        .collect())
}

fn mode_label(mode: &niri_ipc::Mode) -> String {
    if mode.refresh_rate == 0 {
        format!("{}x{}", mode.width, mode.height)
    } else {
        format!(
            "{}x{}@{:.3}",
            mode.width,
            mode.height,
            f64::from(mode.refresh_rate) / 1000.0
        )
    }
}

fn send_request(request: Request) -> io::Result<()> {
    let mut socket = connect()?;
    match socket.send(request)? {
        Ok(_) => Ok(()),
        Err(message) => Err(io::Error::other(format!("niri: {message}"))),
    }
}

/// Convenience for the launcher, used by tests and external tools.
pub fn spawn_command(command: Vec<String>) -> Request {
    Request::Action(Action::Spawn { command })
}

pub fn focus_workspace(id: u64) -> Request {
    Request::Action(Action::FocusWorkspace {
        reference: WorkspaceReferenceArg::Id(id),
    })
}
