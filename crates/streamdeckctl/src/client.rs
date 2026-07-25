//! The socket client.
//!
//! Duplicated from the daemon deliberately: the CLI depends only on the protocol
//! types, so it can be installed and used without linking the daemon.

use std::path::Path;
use std::time::Duration;

use streamdeck_core::control::{self, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// How long to wait for the daemon to answer. A `hold` command deliberately takes
/// as long as the hold itself, so this has to be generous.
const TIMEOUT: Duration = Duration::from_secs(30);

pub async fn send(path: &Path, request: &Request) -> Result<Response, String> {
    let stream = UnixStream::connect(path).await.map_err(|error| {
        format!(
            "could not reach streamdeckd at {}: {error}\n\
             Is the daemon running? Try `launchctl list | grep streamdeckd`.",
            path.display()
        )
    })?;
    let (read, mut write) = stream.into_split();

    write
        .write_all(control::encode(request).as_bytes())
        .await
        .map_err(|error| error.to_string())?;
    write.flush().await.map_err(|error| error.to_string())?;

    let mut line = String::new();
    tokio::time::timeout(TIMEOUT, BufReader::new(read).read_line(&mut line))
        .await
        .map_err(|_| format!("streamdeckd did not answer within {}s", TIMEOUT.as_secs()))?
        .map_err(|error| error.to_string())?;

    if line.trim().is_empty() {
        return Err("streamdeckd closed the connection without answering".to_string());
    }

    let response = control::decode_response(line.trim()).map_err(|error| error.to_string())?;
    if response.version() != control::PROTOCOL_VERSION {
        return Err(format!(
            "streamdeckd speaks protocol version {} but this streamdeckctl speaks {}; \
             reinstall both from the same build",
            response.version(),
            control::PROTOCOL_VERSION
        ));
    }
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_missing_daemon_reports_the_socket_path_and_a_hint() {
        let directory = std::env::temp_dir().join(format!("streamdeckctl-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("absent.sock");

        let error = send(&path, &Request::Status).await.expect_err("no daemon");
        assert!(error.contains("absent.sock"), "{error}");
        assert!(error.contains("launchctl"), "{error}");

        std::fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[tokio::test]
    async fn a_response_round_trips_over_a_real_socket() {
        let directory =
            std::env::temp_dir().join(format!("streamdeckctl-rt-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("streamdeckd.sock");
        let listener = tokio::net::UnixListener::bind(&path).expect("bound");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accepted");
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            BufReader::new(read)
                .read_line(&mut line)
                .await
                .expect("read");
            let request = control::decode_request(line.trim()).expect("decoded");
            assert_eq!(request, Request::Status);

            write
                .write_all(
                    control::encode(&Response::data("ok", serde_json::json!({"page": "home"})))
                        .as_bytes(),
                )
                .await
                .expect("wrote");
            write.flush().await.expect("flushed");
        });

        let response = send(&path, &Request::Status).await.expect("answered");
        assert!(response.is_ok());
        server.await.expect("server");
        std::fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[tokio::test]
    async fn a_protocol_version_mismatch_is_refused_with_advice() {
        let directory =
            std::env::temp_dir().join(format!("streamdeckctl-ver-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("streamdeckd.sock");
        let listener = tokio::net::UnixListener::bind(&path).expect("bound");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accepted");
            let (read, mut write) = stream.into_split();
            let mut line = String::new();
            BufReader::new(read)
                .read_line(&mut line)
                .await
                .expect("read");
            write
                .write_all(b"{\"result\":\"ok\",\"version\":99,\"message\":\"ok\"}\n")
                .await
                .expect("wrote");
            write.flush().await.expect("flushed");
        });

        let error = send(&path, &Request::Status)
            .await
            .expect_err("version mismatch");
        assert!(error.contains("protocol version 99"), "{error}");
        assert!(error.contains("reinstall"), "{error}");

        server.await.expect("server");
        std::fs::remove_dir_all(&directory).expect("cleanup");
    }

    #[tokio::test]
    async fn a_daemon_that_answers_nothing_is_reported_rather_than_hanging_forever() {
        let directory =
            std::env::temp_dir().join(format!("streamdeckctl-silent-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("temp dir");
        let path = directory.join("streamdeckd.sock");
        let listener = tokio::net::UnixListener::bind(&path).expect("bound");

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accepted");
            // Close immediately without answering.
            drop(stream);
        });

        let error = send(&path, &Request::Status).await.expect_err("no answer");
        assert!(error.contains("without answering"), "{error}");

        server.await.expect("server");
        std::fs::remove_dir_all(&directory).expect("cleanup");
    }
}
