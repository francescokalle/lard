use anyhow::Result;
use lard_wayland::Mode;
use tracing_subscriber::EnvFilter;

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
                    "uso: lard ipc <launcher|controls|control-center|notifications|wifi|bluetooth|power-menu|settings|wallpapers|volume-up|volume-down|toggle-mute|lock|quit>"
                );
                std::process::exit(2);
            };
            lard_wayland::ipc::send(command)
        }
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
            eprintln!("comando sconosciuto: {other}\n");
            print_help();
            std::process::exit(2);
        }
    }
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("info,lard_wayland=info,lard_render=warn,lard_services=info")
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn print_help() {
    println!(
        r#"LARD — light as rust dust{}

USO:
  lard                    avvia bar, launcher, OSD, notifiche, control center
  lard lock               lockscreen nativo (ext-session-lock + PAM)
  lard greeter            greeter per greetd (richiede GREETD_SOCK)
  lard greeter --preview  anteprima UI, autenticazione PAM senza avvio sessione
  lard ipc <cmd>          invia un comando alla shell in esecuzione
  lard config-path        stampa il percorso di config.toml
  lard version

COMANDI IPC:
  launcher, control-center, wifi, bluetooth, power-menu, settings, wallpapers, volume-up,
  volume-down, toggle-mute, brightness-up, brightness-down, reload, lock, quit
"#,
        env!("CARGO_PKG_VERSION")
    );
}
