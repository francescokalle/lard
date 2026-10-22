//! Authentication worker. PAM for the lockscreen, greetd for login sessions.
//!
//! Authentication runs on a dedicated thread: a slow PAM module or a D-Bus
//! round-trip must never stall the 120 Hz render loop. Passwords are moved as
//! `Zeroizing<String>` and never logged or included in error messages.

use crate::greetd::{Client, PromptKind, Step};
use crate::pam;
use calloop::channel::Sender;
use lard_core::theme::GreeterConfig;
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    /// Unlock the current session in-process with PAM.
    Lock,
    /// Log in through greetd (PAM happens inside greetd).
    Greeter,
    /// Developer preview: PAM runs in-process but no session is started.
    Preview,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthBackend {
    Pam,
    Greetd,
    PamPreview,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthOutcome {
    Unlocked,
    SessionStarted,
}

#[derive(Clone, Debug)]
pub enum AuthEvent {
    Success(AuthOutcome),
    Failure { message: String },
}

pub struct AuthJob {
    pub username: String,
    pub password: Zeroizing<String>,
    pub session: Option<Vec<String>>,
}

pub struct AuthController {
    tx: std::sync::mpsc::Sender<AuthJob>,
    backend: AuthBackend,
}

impl AuthController {
    pub fn new(
        mode: AuthMode,
        config: &GreeterConfig,
        ui: Sender<AuthEvent>,
    ) -> Result<Self, String> {
        let service = config.pam_service.clone();
        let allow_empty = config.allow_empty_password;
        let backend = match mode {
            AuthMode::Lock => AuthBackend::Pam,
            AuthMode::Preview => AuthBackend::PamPreview,
            AuthMode::Greeter => {
                if std::env::var_os("GREETD_SOCK").is_some() {
                    AuthBackend::Greetd
                } else if std::env::var("NIRI_SUITE_PAM_PREVIEW").as_deref() == Ok("1") {
                    AuthBackend::PamPreview
                } else {
                    return Err("il greeter richiede greetd (GREETD_SOCK non impostata). \
                         Usa `--preview` solo per provare l'interfaccia."
                        .into());
                }
            }
        };

        let (tx, rx) = std::sync::mpsc::channel::<AuthJob>();
        std::thread::Builder::new()
            .name("auth-worker".into())
            .spawn(move || worker(rx, ui, backend, service, allow_empty))
            .map_err(|e| format!("impossibile avviare auth worker: {e}"))?;

        Ok(Self { tx, backend })
    }

    #[must_use]
    pub fn backend(&self) -> AuthBackend {
        self.backend
    }

    pub fn submit(
        &self,
        username: String,
        password: Zeroizing<String>,
        session: Option<Vec<String>>,
    ) {
        let _ = self.tx.send(AuthJob {
            username,
            password,
            session,
        });
    }
}

fn worker(
    rx: std::sync::mpsc::Receiver<AuthJob>,
    ui: Sender<AuthEvent>,
    backend: AuthBackend,
    service: String,
    allow_empty: bool,
) {
    while let Ok(job) = rx.recv() {
        let started = std::time::Instant::now();
        let result = match backend {
            AuthBackend::Pam | AuthBackend::PamPreview => {
                pam::authenticate(&service, &job.username, &job.password, allow_empty)
                    .map(|()| AuthOutcome::Unlocked)
                    .map_err(|e| e.user_message().to_string())
            }
            AuthBackend::Greetd => greetd_login(&job),
        };

        // A small floor avoids turning the greeter into a username oracle via
        // response timing. PAM/greetd still own the real rate limiting.
        let elapsed = started.elapsed();
        if elapsed < std::time::Duration::from_millis(280) {
            std::thread::sleep(std::time::Duration::from_millis(280) - elapsed);
        }

        let event = match result {
            Ok(outcome) => AuthEvent::Success(outcome),
            Err(message) => AuthEvent::Failure { message },
        };
        if ui.send(event).is_err() {
            return;
        }
    }
}

fn greetd_login(job: &AuthJob) -> Result<AuthOutcome, String> {
    let mut client = Client::connect()?;
    let mut step = client.create_session(&job.username)?;

    for _ in 0..8 {
        match step {
            Step::Prompt { kind, message } => {
                let response = match kind {
                    PromptKind::Secret => Some(job.password.as_str()),
                    PromptKind::Visible => Some(job.username.as_str()),
                    PromptKind::Info | PromptKind::Error => None,
                };
                if kind == PromptKind::Error && !message.is_empty() {
                    tracing::debug!(message, "greetd auth error prompt");
                }
                step = client.respond(response)?;
            }
            Step::Success => {
                let cmd = job
                    .session
                    .clone()
                    .unwrap_or_else(|| vec!["niri-session".into()]);
                let env = vec![
                    "XDG_SESSION_TYPE=wayland".to_string(),
                    "XDG_CURRENT_DESKTOP=niri".to_string(),
                    "MOZ_ENABLE_WAYLAND=1".to_string(),
                ];
                client.start_session(cmd, env)?;
                return Ok(AuthOutcome::SessionStarted);
            }
            Step::Error { description } => {
                client.cancel();
                return Err(if description.is_empty() {
                    "Credenziali non valide".into()
                } else {
                    description
                });
            }
        }
    }

    client.cancel();
    Err("Troppi passaggi di autenticazione".into())
}
