//! Session power actions. `systemctl` is used with fixed argv (no shell).
//! In a greeter this is the least-privilege path: logind applies polkit policy
//! and the greeter process never needs raw root.

use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAction {
    PowerOff,
    Reboot,
    Suspend,
    Lock,
    Logout,
}

impl PowerAction {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PowerOff => "Power off",
            Self::Reboot => "Restart",
            Self::Suspend => "Suspend",
            Self::Lock => "Lock",
            Self::Logout => "Log out",
        }
    }
}

pub fn perform(action: PowerAction) -> std::io::Result<()> {
    match action {
        PowerAction::PowerOff => systemctl("poweroff"),
        PowerAction::Reboot => systemctl("reboot"),
        PowerAction::Suspend => systemctl("suspend"),
        PowerAction::Lock => {
            // lard exposes `ipc lock`; prefer the running shell.
            Command::new("lard").args(["ipc", "lock"]).status()?;
            Ok(())
        }
        PowerAction::Logout => logout(),
    }
}

fn logout() -> std::io::Result<()> {
    // Ask niri to leave the session cleanly (greetd starts the greeter again).
    let niri = Command::new("niri")
        .args(["msg", "action", "quit", "--skip-confirmation"])
        .status();
    if matches!(niri, Ok(status) if status.success()) {
        return Ok(());
    }

    if let Ok(session) = std::env::var("XDG_SESSION_ID")
        && !session.is_empty()
    {
        let status = Command::new("loginctl")
            .args(["terminate-session", &session])
            .status()?;
        if status.success() {
            return Ok(());
        }
    }

    let user = std::env::var("USER").unwrap_or_default();
    let status = Command::new("loginctl")
        .args(["terminate-user", &user])
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "loginctl terminate-user exited with {status}"
        )))
    }
}

fn systemctl(action: &str) -> std::io::Result<()> {
    let status = Command::new("systemctl").arg(action).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "systemctl {action} exited with {status}"
        )))
    }
}

/// Run a power action without blocking the UI thread.
pub fn perform_async(action: PowerAction) {
    let _ = std::thread::Builder::new()
        .name("power-action".into())
        .spawn(move || {
            if let Err(error) = perform(action) {
                tracing::error!(%error, ?action, "power action failed");
            }
        });
}
