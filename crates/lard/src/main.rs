use anyhow::{bail, Context, Result};
use lard_wayland::Mode;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tracing_subscriber::EnvFilter;

/// Official bootstrap: it always fetches the current `main` snapshot and runs
/// the real installer. Keeping the URL here means `lard update` never depends
/// on a stale copy installed on disk.
const INSTALLER_URL: &str = "https://lard.francescocallegaro.com/install.sh";

fn main() {
    if let Err(error) = run() {
        eprintln!("lard: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("shell");

    match command {
        "shell" | "run" => {
            init_logging();
            lard_wayland::run(Mode::Shell)
        }
        "lock" | "lockscreen" => {
            init_logging();
            lard_wayland::run(Mode::Lock)
        }
        "greeter" => {
            init_logging();
            let preview = args.iter().any(|arg| arg == "--preview");
            lard_wayland::run(Mode::Greeter { preview })
        }
        "ipc" => {
            let Some(command) = args.get(1) else {
                eprintln!(
                    "usage: lard ipc <launcher|controls|control-center|notifications|wifi|bluetooth|battery|system|power-menu|settings|wallpapers|volume-up|volume-down|toggle-mute|lock|quit>"
                );
                std::process::exit(2);
            };
            lard_wayland::ipc::send(command)
        }
        "update" | "upgrade" => run_update(&args[1..]),
        "config-path" => {
            println!("{}", lard_core::Config::effective_path().display());
            Ok(())
        }
        "version" | "--version" | "-V" => {
            println!("LARD {} — light as rust dust", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => {
            eprintln!("unknown command: {other}\n");
            print_help();
            std::process::exit(2);
        }
    }
}

/// `lard update`: delegate to the system package manager when LARD was
/// installed from a package, otherwise run the official installer with
/// `--update` (keeps the configuration, rebuilds, reinstalls, restarts the bar).
fn run_update(args: &[String]) -> Result<()> {
    let force_source = args.iter().any(|a| a == "--source" || a == "--rebuild");
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let assume_yes = args.iter().any(|a| a == "-y" || a == "--yes");

    if !force_source && !dry_run {
        if let Some((program, command, label)) = package_manager_upgrade() {
            println!("lard: installed by {label}, updating through the package manager");
            if run_privileged(&program, &command)? {
                restart_shell();
                return Ok(());
            }
            eprintln!("lard: the package manager did not update {label}; falling back to the source build");
        }
    }

    let installer = download_installer()?;
    let mut argv = vec![installer.display().to_string(), "--update".to_string()];
    if dry_run {
        argv.push("--dry-run".into());
    }
    if assume_yes {
        argv.push("-y".into());
    }

    println!("lard: running the official updater ({}):", INSTALLER_URL);
    let status = run_installer(&argv).context("cannot run the updater")?;
    let _ = std::fs::remove_file(&installer);
    if !status.success() {
        bail!("the updater failed ({status})");
    }
    if !dry_run {
        println!("lard: update complete");
    }
    Ok(())
}

/// The installer writes to /usr/local and to ~/.config, so it must run as the
/// invoking user and use sudo internally; when `lard update` itself runs as
/// root, drop back to the desktop user.
fn run_installer(argv: &[String]) -> Result<std::process::ExitStatus> {
    if is_root() {
        if let Ok(user) = std::env::var("SUDO_USER") {
            if !user.is_empty() && user != "root" {
                let mut command = Command::new("runuser");
                command.arg("-u").arg(user).arg("--").arg("bash").args(argv);
                return Ok(command.status()?);
            }
        }
    }
    Ok(Command::new("bash").args(argv).status()?)
}

fn is_root() -> bool {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .is_some_and(|uid| uid.trim() == "0")
}

fn run_privileged(program: &str, args: &[String]) -> Result<bool> {
    let mut command;
    if is_root() {
        command = Command::new(program);
    } else {
        command = Command::new("sudo");
        command.arg("--").arg(program);
    }
    let status = command.args(args).status()?;
    Ok(status.success())
}

/// Detect a package-managed installation so `pacman -Syu`/`apt upgrade` stays
/// the single source of truth. A script install in /usr/local is not owned by
/// any package, so this returns None and the source updater runs instead.
fn package_manager_upgrade() -> Option<(String, Vec<String>, String)> {
    let exe = std::env::current_exe().ok()?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let path = exe.display().to_string();

    if let Ok(out) = Command::new("pacman").args(["-Qo", &path]).output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            // "/usr/bin/lard is owned by lard 0.1.0-1"
            if let Some(name) = text.split_whitespace().nth(4) {
                if name.starts_with("lard") {
                    return Some((
                        "pacman".into(),
                        vec![
                            "-S".into(),
                            "--needed".into(),
                            "--noconfirm".into(),
                            name.into(),
                        ],
                        format!("pacman ({name})"),
                    ));
                }
            }
        }
    }
    if let Ok(out) = Command::new("dpkg").args(["-S", &path]).output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Some(name) = text.split(':').next().map(str::trim) {
                if name.starts_with("lard") {
                    return Some((
                        "apt-get".into(),
                        vec![
                            "install".into(),
                            "--only-upgrade".into(),
                            "-y".into(),
                            name.into(),
                        ],
                        format!("apt ({name})"),
                    ));
                }
            }
        }
    }
    if let Ok(out) = Command::new("rpm").args(["-qf", &path]).output() {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.trim().starts_with("lard") {
                return Some((
                    "dnf".into(),
                    vec!["upgrade".into(), "-y".into(), "lard".into()],
                    "dnf (lard)".into(),
                ));
            }
        }
    }
    None
}

fn download_installer() -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("lard-updater-{}.sh", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let available = |program: &str| {
        Command::new(program)
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    };

    let ok = if available("curl") {
        Command::new("curl")
            .args(["-fsSL", "--proto", "=https", "--tlsv1.2", INSTALLER_URL, "-o"])
            .arg(&path)
            .status()?
            .success()
    } else if available("wget") {
        Command::new("wget")
            .args(["-qO"])
            .arg(&path)
            .arg(INSTALLER_URL)
            .status()?
            .success()
    } else {
        bail!("curl or wget is required to update");
    };
    if !ok {
        bail!("cannot download {INSTALLER_URL}");
    }
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    if !body.contains("LARD") {
        let _ = std::fs::remove_file(&path);
        bail!("the downloaded file is not the LARD installer");
    }
    Ok(path)
}

/// Restart the running bar so it picks up the new binary. The shelved unit is
/// only touched when it is actually running: a manually started `lard` is left
/// alone, and a terminal running `lard update` is never affected.
fn restart_shell() {
    let active = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "lard.service"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !active {
        println!("lard: start the new version with `lard` (the service is not running)");
        return;
    }
    match Command::new("systemctl")
        .args(["--user", "restart", "lard.service"])
        .status()
    {
        Ok(status) if status.success() => println!("lard: shell restarted"),
        _ => eprintln!("lard: restart the shell with `systemctl --user restart lard.service`"),
    }
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "info,lard_wayland=info,lard_render=warn,lard_services=info,usvg=error,resvg=error",
        )
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn print_help() {
    println!(
        r#"LARD — light as rust dust{}

USAGE:
  lard                    start bar, launcher, OSD, notifications, control center
  lard lock               native lockscreen (ext-session-lock + PAM)
  lard greeter            greetd greeter (needs GREETD_SOCK)
  lard greeter --preview  UI preview, PAM authentication without starting a session
  lard ipc <cmd>          send a command to the running shell
  lard update             fetch and install the latest version
  lard update --source    always rebuild from source (skip the package manager)
  lard update --dry-run   show what the updater would do
  lard config-path        print the config.toml path
  lard version

IPC COMMANDS:
  launcher, control-center, controls, notifications, wifi, bluetooth, battery, system,
  power-menu, settings, wallpapers, volume-up, volume-down, toggle-mute,
  brightness-up, brightness-down, theme-system, theme-light, theme-dark,\n  reload, lock, quit
"#,
        env!("CARGO_PKG_VERSION")
    );
}
