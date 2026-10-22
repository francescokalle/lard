pub mod app;
pub mod config_watch;
pub mod ipc;

/// Exit status used when no Wayland compositor is available.
///
/// `EX_CONFIG` (sysexits.h): the unit ships `RestartPreventExitStatus=78` so a
/// missing compositor stops the shell instead of restarting it forever.
pub const EXIT_NO_COMPOSITOR: i32 = 78;

use anyhow::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Shell,
    Lock,
    Greeter { preview: bool },
}

pub fn run(mode: Mode) -> Result<()> {
    app::run(mode)
}
