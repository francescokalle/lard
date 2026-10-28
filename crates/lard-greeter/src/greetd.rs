//! Minimal greetd IPC client.
//!
//! greetd is the privilege boundary: it owns PAM, filesystem permissions and
//! session registration. The greeter only speaks this small protocol, which is
//! why it can run as an unprivileged user inside a compositor.
//!
//! Wire format (greetd >= 0.8, see `greetd_ipc`):
//!
//! ```text
//! +----------+-------------------+
//! | len: u32 | JSON payload: str |
//! +----------+-------------------+
//! ```
//!
//! The length is a native-endian `u32` and the payload is the JSON encoding of
//! a request/response variant. It is *not* newline-delimited: framing errors
//! make greetd close the connection with `early eof`.

use serde::{Deserialize, Serialize};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

/// Upper bound for a single protocol message. Responses are tiny; anything
/// bigger is a framing error (and must never be pre-allocated blindly).
const MAX_MESSAGE: usize = 1 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    Visible,
    Secret,
    Info,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Prompt { kind: PromptKind, message: String },
    Success,
    Error { description: String },
}

pub struct Client {
    stream: UnixStream,
}

impl Client {
    pub fn connect() -> Result<Self, String> {
        let socket = std::env::var_os("GREETD_SOCK")
            .ok_or_else(|| "GREETD_SOCK is not set".to_string())?;
        Self::connect_to(std::path::Path::new(&socket))
    }

    fn connect_to(socket: &std::path::Path) -> Result<Self, String> {
        let stream = UnixStream::connect(socket)
            .map_err(|e| format!("cannot connect to greetd: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(45)))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        Ok(Self { stream })
    }

    pub fn create_session(&mut self, username: &str) -> Result<Step, String> {
        self.send(&Request::CreateSession { username })?;
        self.receive()
    }

    pub fn respond(&mut self, response: Option<&str>) -> Result<Step, String> {
        self.send(&Request::PostAuthMessageResponse { response })?;
        self.receive()
    }

    pub fn start_session(&mut self, cmd: Vec<String>, env: Vec<String>) -> Result<(), String> {
        self.send(&Request::StartSession { cmd, env })?;
        match self.receive()? {
            Step::Success => Ok(()),
            Step::Error { description } => Err(description),
            Step::Prompt { message, .. } => Err(format!("unexpected prompt: {message}")),
        }
    }

    pub fn cancel(&mut self) {
        let _ = self.send(&Request::CancelSession);
    }

    fn send(&mut self, request: &Request<'_>) -> Result<(), String> {
        let body =
            serde_json::to_vec(request).map_err(|e| format!("invalid greetd request: {e}"))?;
        let len =
            u32::try_from(body.len()).map_err(|_| "greetd request too large".to_string())?;
        self.stream
            .write_all(&len.to_ne_bytes())
            .and_then(|()| self.stream.write_all(&body))
            .and_then(|()| self.stream.flush())
            .map_err(|e| format!("cannot write to greetd: {e}"))
    }

    fn receive(&mut self) -> Result<Step, String> {
        let mut len_bytes = [0u8; 4];
        match self.stream.read_exact(&mut len_bytes) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                return Err("greetd closed the connection".into());
            }
            Err(e) => return Err(format!("cannot read from greetd: {e}")),
        }
        let len = u32::from_ne_bytes(len_bytes) as usize;
        if len == 0 || len > MAX_MESSAGE {
            return Err(format!("invalid greetd response (length {len})"));
        }
        let mut body = vec![0u8; len];
        self.stream
            .read_exact(&mut body)
            .map_err(|e| format!("cannot read from greetd: {e}"))?;
        let reply: Reply = serde_json::from_slice(&body)
            .map_err(|e| format!("invalid greetd response: {e}"))?;
        Ok(match reply {
            Reply::AuthMessage {
                auth_message_type,
                auth_message,
            } => {
                let kind = match auth_message_type.as_str() {
                    "secret" => PromptKind::Secret,
                    "visible" => PromptKind::Visible,
                    "error" => PromptKind::Error,
                    _ => PromptKind::Info,
                };
                Step::Prompt {
                    kind,
                    message: auth_message,
                }
            }
            Reply::Success => Step::Success,
            Reply::Error { description, .. } => Step::Error { description },
            Reply::Unknown => Step::Error {
                description: "unknown greetd response".into(),
            },
        })
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request<'a> {
    CreateSession { username: &'a str },
    PostAuthMessageResponse { response: Option<&'a str> },
    StartSession { cmd: Vec<String>, env: Vec<String> },
    CancelSession,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Reply {
    AuthMessage {
        auth_message_type: String,
        auth_message: String,
    },
    Success,
    Error {
        #[allow(dead_code)]
        error_type: String,
        description: String,
    },
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;

    fn write_message(stream: &mut UnixStream, body: &str) {
        let bytes = body.as_bytes();
        stream
            .write_all(&(bytes.len() as u32).to_ne_bytes())
            .unwrap();
        stream.write_all(bytes).unwrap();
        stream.flush().unwrap();
    }

    fn read_message(stream: &mut UnixStream) -> String {
        let mut len = [0u8; 4];
        stream.read_exact(&mut len).unwrap();
        let mut body = vec![0u8; u32::from_ne_bytes(len) as usize];
        stream.read_exact(&mut body).unwrap();
        String::from_utf8(body).unwrap()
    }

    #[test]
    fn speaks_length_prefixed_protocol() {
        let dir = std::env::temp_dir().join(format!("lard-greetd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sock");
        let listener = UnixListener::bind(&path).unwrap();

        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            // CreateSession -> secret prompt.
            let request = read_message(&mut socket);
            assert!(request.contains("\"create_session\""), "{request}");
            assert!(request.contains("\"testuser\""), "{request}");
            write_message(
                &mut socket,
                r#"{"type":"auth_message","auth_message_type":"secret","auth_message":"Password:"}"#,
            );

            // Password -> success.
            let request = read_message(&mut socket);
            assert!(
                request.contains("\"post_auth_message_response\""),
                "{request}"
            );
            assert!(request.contains("segreto-di-prova"), "{request}");
            write_message(&mut socket, r#"{"type":"success"}"#);

            // StartSession -> success.
            let request = read_message(&mut socket);
            assert!(request.contains("\"start_session\""), "{request}");
            assert!(request.contains("niri-session"), "{request}");
            write_message(&mut socket, r#"{"type":"success"}"#);

            // CancelSession is accepted silently.
            let _ = read_message(&mut socket);
        });

        // SAFETY: no shared env: the client connects to the explicit socket path.
        let mut client = Client::connect_to(&path).unwrap();
        assert_eq!(
            client.create_session("testuser").unwrap(),
            Step::Prompt {
                kind: PromptKind::Secret,
                message: "Password:".into()
            }
        );
        assert_eq!(
            client.respond(Some("segreto-di-prova")).unwrap(),
            Step::Success
        );
        client
            .start_session(vec!["niri-session".into()], vec![])
            .unwrap();
        client.cancel();

        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_oversized_frames() {
        let dir = std::env::temp_dir().join(format!("lard-greetd-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sock");
        let listener = UnixListener::bind(&path).unwrap();

        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let _ = read_message(&mut socket);
            socket
                .write_all(&(MAX_MESSAGE as u32 + 1).to_ne_bytes())
                .unwrap();
            socket.flush().unwrap();
            thread::sleep(Duration::from_millis(50));
        });

        // SAFETY: no shared env: the client connects to the explicit socket path.
        let mut client = Client::connect_to(&path).unwrap();
        let error = client.create_session("testuser").unwrap_err();
        assert!(error.contains("lunghezza"), "{error}");

        server.join().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
