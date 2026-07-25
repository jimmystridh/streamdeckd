//! The Unix control socket.
//!
//! Bound inside the user's application-support directory with `0600` permissions.
//! Requests are a closed enum, so no command string ever reaches a shell.

use std::path::{Path, PathBuf};

use streamdeck_core::control::{self, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};

use crate::runtime::RuntimeEvent;

/// Owns the listening socket and removes it on drop.
#[derive(Debug)]
pub struct ControlSocket {
    path: PathBuf,
    listener: UnixListener,
}

impl ControlSocket {
    /// Binds the socket, replacing a stale one left by an unclean exit.
    pub async fn bind(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        if path.exists() {
            // Only remove it once we know nobody is listening.
            if UnixStream::connect(&path).await.is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!(
                        "another streamdeckd is already listening on {}",
                        path.display()
                    ),
                ));
            }
            tokio::fs::remove_file(&path).await?;
        }

        let listener = UnixListener::bind(&path)?;
        restrict(&path)?;
        Ok(Self { path, listener })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Serves requests until the runtime stops. Each connection handles one
    /// request/response pair.
    pub async fn serve(self, events: mpsc::UnboundedSender<RuntimeEvent>) {
        loop {
            let (stream, _) = match self.listener.accept().await {
                Ok(accepted) => accepted,
                Err(error) => {
                    tracing::warn!(component = "control", error = %error, "accept failed");
                    continue;
                }
            };
            let events = events.clone();
            tokio::spawn(async move {
                if let Err(error) = handle(stream, events).await {
                    tracing::debug!(component = "control", error = %error, "connection ended");
                }
            });
        }
    }
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

async fn handle(
    stream: UnixStream,
    events: mpsc::UnboundedSender<RuntimeEvent>,
) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    // A bounded read so a stuck client cannot grow this process.
    let mut reader = BufReader::new(read.take(control::MAX_REQUEST_BYTES as u64));
    let mut line = String::new();
    let read_bytes = reader.read_line(&mut line).await?;
    if read_bytes == 0 {
        return Ok(());
    }

    let response = match control::decode_request(line.trim()) {
        Ok(request) => dispatch(request, events).await,
        Err(error) => Response::error(error.to_string()),
    };

    write
        .write_all(control::encode(&response).as_bytes())
        .await?;
    write.flush().await
}

async fn dispatch(request: Request, events: mpsc::UnboundedSender<RuntimeEvent>) -> Response {
    let (reply, receiver) = oneshot::channel();
    if events
        .send(RuntimeEvent::Control { request, reply })
        .is_err()
    {
        return Response::error("the daemon is shutting down");
    }
    match receiver.await {
        Ok(response) => response,
        Err(_) => Response::error("the daemon did not answer"),
    }
}

/// How long to wait for an answer. `hold` deliberately takes as long as the hold.
const CLIENT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Sends one request to a running daemon and returns its response.
pub async fn send(path: &Path, request: &Request) -> Result<Response, String> {
    let stream = UnixStream::connect(path)
        .await
        .map_err(|error| format!("could not reach streamdeckd at {}: {error}", path.display()))?;
    let (read, mut write) = stream.into_split();

    write
        .write_all(control::encode(request).as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    write.flush().await.map_err(|error| error.to_string())?;

    let mut line = String::new();
    tokio::time::timeout(CLIENT_TIMEOUT, BufReader::new(read).read_line(&mut line))
        .await
        .map_err(|_| {
            format!(
                "streamdeckd did not answer within {}s",
                CLIENT_TIMEOUT.as_secs()
            )
        })?
        .map_err(|error| error.to_string())?;
    if line.trim().is_empty() {
        return Err("the daemon closed the connection without answering".to_string());
    }

    let response = control::decode_response(line.trim()).map_err(|error| error.to_string())?;
    if response.version() != control::PROTOCOL_VERSION {
        return Err(format!(
            "the daemon speaks protocol version {} but this CLI speaks {}",
            response.version(),
            control::PROTOCOL_VERSION
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use streamdeck_core::model::PageId;

    /// Answers every request with a canned response, standing in for the runtime.
    fn spawn_fake_runtime(
        mut events: mpsc::UnboundedReceiver<RuntimeEvent>,
    ) -> tokio::task::JoinHandle<Vec<Request>> {
        tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(event) = events.recv().await {
                if let RuntimeEvent::Control { request, reply } = event {
                    seen.push(request.clone());
                    let response = match request {
                        Request::Status => {
                            Response::data("ok", serde_json::json!({"page": "home"}))
                        }
                        other => Response::ok(format!("{other:?}")),
                    };
                    let _ = reply.send(response);
                }
            }
            seen
        })
    }

    #[tokio::test]
    async fn a_request_round_trips_through_the_socket() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("streamdeckd.sock");
        let socket = ControlSocket::bind(&path).await.expect("bound");

        let (sender, receiver) = mpsc::unbounded_channel();
        let runtime = spawn_fake_runtime(receiver);
        let server = tokio::spawn(socket.serve(sender.clone()));

        let response = send(&path, &Request::Status).await.expect("answered");
        assert!(response.is_ok());
        assert_eq!(
            response,
            Response::data("ok", serde_json::json!({"page": "home"}))
        );

        server.abort();
        drop(sender);
        let seen = runtime.await.expect("runtime");
        assert_eq!(seen, vec![Request::Status]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_socket_is_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("streamdeckd.sock");
        let _socket = ControlSocket::bind(&path).await.expect("bound");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the socket must not be world writable");
    }

    #[tokio::test]
    async fn a_stale_socket_from_an_unclean_exit_is_replaced() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("streamdeckd.sock");
        std::fs::write(&path, b"").expect("stale file");

        let socket = ControlSocket::bind(&path).await.expect("bound over stale");
        assert_eq!(socket.path(), path);
    }

    #[tokio::test]
    async fn a_second_daemon_refuses_to_bind_over_a_live_socket() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("streamdeckd.sock");
        let first = ControlSocket::bind(&path).await.expect("bound");
        let (sender, receiver) = mpsc::unbounded_channel();
        let _runtime = spawn_fake_runtime(receiver);
        let server = tokio::spawn(first.serve(sender));

        let error = ControlSocket::bind(&path).await.expect_err("refused");
        assert_eq!(error.kind(), std::io::ErrorKind::AddrInUse);

        server.abort();
    }

    #[tokio::test]
    async fn dropping_the_socket_removes_the_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("streamdeckd.sock");
        {
            let _socket = ControlSocket::bind(&path).await.expect("bound");
            assert!(path.exists());
        }
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_malformed_request_gets_an_error_response_rather_than_a_dropped_connection() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("streamdeckd.sock");
        let socket = ControlSocket::bind(&path).await.expect("bound");
        let (sender, receiver) = mpsc::unbounded_channel();
        let _runtime = spawn_fake_runtime(receiver);
        let server = tokio::spawn(socket.serve(sender));

        let mut stream = UnixStream::connect(&path).await.expect("connected");
        stream
            .write_all(b"{\"command\":\"exec\",\"script\":\"rm -rf ~\"}\n")
            .await
            .expect("wrote");
        stream.flush().await.expect("flushed");

        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .await
            .expect("read");
        let response = control::decode_response(line.trim()).expect("decoded");
        assert!(!response.is_ok());
        assert!(response.message().contains("malformed"), "{response:?}");

        server.abort();
    }

    #[tokio::test]
    async fn talking_to_a_missing_daemon_reports_the_path() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("absent.sock");
        let error = send(&path, &Request::Status).await.expect_err("no daemon");
        assert!(error.contains("absent.sock"), "{error}");
    }

    #[tokio::test]
    async fn every_command_reaches_the_runtime_unchanged() {
        let directory = tempfile::tempdir().expect("temp dir");
        let path = directory.path().join("streamdeckd.sock");
        let socket = ControlSocket::bind(&path).await.expect("bound");
        let (sender, receiver) = mpsc::unbounded_channel();
        let runtime = spawn_fake_runtime(receiver);
        let server = tokio::spawn(socket.serve(sender.clone()));

        let requests = [
            Request::Page {
                page: PageId::Mixer,
            },
            Request::Reload,
            Request::Doctor,
            Request::Stop,
        ];
        for request in &requests {
            assert!(send(&path, request).await.expect("answered").is_ok());
        }

        server.abort();
        drop(sender);
        let seen = runtime.await.expect("runtime");
        assert_eq!(seen, requests.to_vec());
    }
}
