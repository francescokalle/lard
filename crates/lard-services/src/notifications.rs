//! Minimal but real `org.freedesktop.Notifications` server.
//!
//! It owns the well-known name on the session bus and forwards notifications
//! to the UI thread. The UI owns timeouts and sends `NotificationClosed` back
//! through [`DaemonCommand::Close`], so the D-Bus signal is emitted exactly
//! once with the correct reason.
//!
//! Capabilities intentionally advertised: `body` only. Actions, markup and
//! image hints are parsed later, not silently mangled.

use crate::ServiceEvent;
use calloop::channel::Sender;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    Call = 3,
}

impl CloseReason {
    #[must_use]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotificationAction {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Debug)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<NotificationAction>,
    /// Milliseconds; `0` means "never expire".
    pub timeout_ms: u32,
}

#[derive(Clone, Debug)]
pub enum DaemonCommand {
    Close { id: u32, reason: u32 },
    Action { id: u32, action: String },
}

struct NotificationsIface {
    ui: Sender<ServiceEvent>,
    next_id: AtomicU32,
    default_timeout_ms: i32,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl NotificationsIface {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        _hints: HashMap<String, zbus::OwnedVariant>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::Relaxed).max(1)
        };
        let timeout_ms = if expire_timeout < 0 {
            self.default_timeout_ms.max(0) as u32
        } else {
            expire_timeout as u32
        };
        let actions = actions
            .chunks_exact(2)
            .filter(|pair| !pair[0].is_empty())
            .map(|pair| NotificationAction {
                key: pair[0].clone(),
                label: pair[1].clone(),
            })
            .collect();
        let _ = self.ui.send(ServiceEvent::Notification(Notification {
            id,
            app_name,
            app_icon,
            summary,
            body,
            actions,
            timeout_ms,
        }));
        id
    }

    fn close_notification(&self, id: u32) {
        let _ = self.ui.send(ServiceEvent::NotificationClosed {
            id,
            reason: CloseReason::Call,
        });
    }

    fn get_capabilities(&self) -> Vec<String> {
        vec!["body".to_string(), "actions".to_string()]
    }

    #[zbus(out_args("name", "vendor", "version", "spec_version"))]
    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "lard".into(),
            "lard".into(),
            env!("CARGO_PKG_VERSION").into(),
            "1.2".into(),
        )
    }
}

/// Spawn the D-Bus daemon. The returned sender is safe to keep even when the
/// session bus is unavailable; commands are simply ignored.
pub fn spawn(
    ui: Sender<ServiceEvent>,
    default_timeout_ms: i32,
) -> Option<UnboundedSender<DaemonCommand>> {
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();

    std::thread::Builder::new()
        .name("notifications-dbus".into())
        .spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                tracing::error!("cannot create tokio runtime for notifications");
                return;
            };
            runtime.block_on(async move {
                let iface = NotificationsIface {
                    ui,
                    next_id: AtomicU32::new(1),
                    default_timeout_ms,
                };
                let connection = zbus::connection::Builder::session()
                    .name("org.freedesktop.Notifications")
                    .serve_at("/org/freedesktop/Notifications", iface)
                    .build()
                    .await;

                let connection = match connection {
                    Ok(connection) => connection,
                    Err(error) => {
                        tracing::warn!(%error, "cannot own org.freedesktop.Notifications");
                        return;
                    }
                };
                tracing::info!("notification daemon ready on the session bus");
                run_signals(connection, cmd_rx).await;
            });
        })
        .expect("failed to spawn notifications thread");

    Some(cmd_tx)
}

async fn run_signals(connection: zbus::Connection, mut rx: UnboundedReceiver<DaemonCommand>) {
    while let Some(command) = rx.recv().await {
        match command {
            DaemonCommand::Close { id, reason } => {
                let _ = connection
                    .emit_signal(
                        None::<&str>,
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                        "NotificationClosed",
                        &(id, reason),
                    )
                    .await;
            }
            DaemonCommand::Action { id, action } => {
                let _ = connection
                    .emit_signal(
                        None::<&str>,
                        "/org/freedesktop/Notifications",
                        "org.freedesktop.Notifications",
                        "ActionInvoked",
                        &(id, action),
                    )
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_reason_values() {
        assert_eq!(CloseReason::Expired.as_u32(), 1);
        assert_eq!(CloseReason::Dismissed.as_u32(), 2);
        assert_eq!(CloseReason::Call.as_u32(), 3);
    }
}
