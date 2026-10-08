//! One short-lived connection per request. niri's replies carry no request id, so a
//! late reply on a reused connection could be read as the answer to the next request.
//! A fresh connection, dropped at the deadline, can't mix them up. `niri msg` works the
//! same way.

use std::path::Path;
use std::time::Duration;

use niri_ipc::{Reply, Request, Response};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;

use crate::error::{ErrorName, ToolError};

const DEADLINE: Duration = Duration::from_secs(2);

/// Sends one request on a new connection and returns niri's response, all within two
/// seconds.
pub(crate) async fn send(socket: &Path, request: &Request) -> Result<Response, ToolError> {
    within(DEADLINE, request, async {
        let stream = UnixStream::connect(socket).await.map_err(|error| {
            ToolError::new(
                ErrorName::NiriUnavailable,
                format!("connect to {}: {error}", socket.display()),
            )
        })?;
        exchange(stream, request).await
    })
    .await
}

async fn within(
    deadline: Duration,
    request: &Request,
    exchange: impl Future<Output = Result<Response, ToolError>>,
) -> Result<Response, ToolError> {
    tokio::time::timeout(deadline, exchange)
        .await
        .unwrap_or_else(|_| {
            Err(ToolError::new(
                ErrorName::DeadlineExceeded,
                format!("niri {request:?}: no reply within {deadline:?}"),
            ))
        })
}

/// Writes the request as one JSON line and reads one reply line, the way
/// `niri_ipc::socket::Socket::send` does.
async fn exchange(
    stream: impl AsyncRead + AsyncWrite + Unpin,
    request: &Request,
) -> Result<Response, ToolError> {
    let doing = format!("niri {request:?}");
    let broken = |error: std::io::Error| {
        ToolError::new(ErrorName::NiriUnavailable, format!("{doing}: {error}"))
    };
    let mut line = serde_json::to_vec(request)
        .map_err(|error| ToolError::new(ErrorName::UpstreamError, format!("{doing}: {error}")))?;
    line.push(b'\n');
    let mut stream = BufReader::new(stream);
    stream.get_mut().write_all(&line).await.map_err(broken)?;
    let mut reply = String::new();
    if stream.read_line(&mut reply).await.map_err(broken)? == 0 {
        return Err(ToolError::new(
            ErrorName::NiriUnavailable,
            format!("{doing}: niri closed the connection without a reply"),
        ));
    }
    let reply: Reply = serde_json::from_str(&reply).map_err(|error| {
        ToolError::new(
            ErrorName::UpstreamError,
            format!("{doing}: unreadable reply: {error}"),
        )
    })?;
    reply.map_err(|message| {
        ToolError::new(
            ErrorName::UpstreamError,
            format!("{doing}: niri replied: {message}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt as _, DuplexStream, duplex};

    use super::*;

    /// Runs `exchange` against an in-memory niri that answers with `reply`, and returns
    /// the result with the request line niri received.
    async fn against(reply: &'static str) -> (Result<Response, ToolError>, String) {
        let (client, mut niri) = duplex(4096);
        let fake = tokio::spawn(async move {
            let mut received = vec![0; 4096];
            let read = niri.read(&mut received).await.unwrap();
            niri.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8(received[..read].to_vec()).unwrap()
        });
        let result = exchange(client, &Request::Version).await;
        (result, fake.await.unwrap())
    }

    #[tokio::test]
    async fn sends_one_json_line_and_reads_the_reply() {
        let (result, received) = against("{\"Ok\":{\"Version\":\"26.04 (8ed0da4)\"}}\n").await;
        assert_eq!(received, "\"Version\"\n");
        let Response::Version(version) = result.unwrap() else {
            panic!("not a version response");
        };
        assert_eq!(version, "26.04 (8ed0da4)");
    }

    #[tokio::test]
    async fn keeps_niri_error_messages() {
        let (result, _) = against("{\"Err\":\"no such output\"}\n").await;
        assert_eq!(
            result.unwrap_err(),
            ToolError::new(
                ErrorName::UpstreamError,
                "niri Version: niri replied: no such output"
            )
        );
    }

    #[tokio::test]
    async fn an_unreadable_or_missing_reply_is_an_error() {
        let (unreadable, _) = against("not json\n").await;
        assert_eq!(unreadable.unwrap_err().name, ErrorName::UpstreamError);
        let (missing, _) = against("").await;
        let error = missing.unwrap_err();
        assert_eq!(error.name, ErrorName::NiriUnavailable);
        assert!(error.detail.contains("without a reply"), "{error:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_silent_niri_hits_the_deadline() {
        let (client, _niri): (DuplexStream, DuplexStream) = duplex(4096);
        let result = within(
            Duration::from_millis(50),
            &Request::Version,
            exchange(client, &Request::Version),
        )
        .await;
        assert_eq!(
            result.unwrap_err(),
            ToolError::new(
                ErrorName::DeadlineExceeded,
                "niri Version: no reply within 50ms"
            )
        );
    }

    #[tokio::test(start_paused = true)]
    async fn dropping_a_request_closes_its_connection() {
        let (client, mut niri) = duplex(4096);
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                exchange(client, &Request::Version)
            )
            .await
            .is_err()
        );
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(1), niri.read_to_end(&mut received))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(received, b"\"Version\"\n");
    }

    #[tokio::test]
    async fn a_missing_socket_is_unavailable() {
        let socket = std::env::temp_dir().join(format!(
            "niri-computer-use-missing-{}.sock",
            std::process::id()
        ));
        let error = send(&socket, &Request::Version).await.unwrap_err();
        assert_eq!(error.name, ErrorName::NiriUnavailable);
        assert!(error.detail.starts_with("connect to "), "{error:?}");
    }
}
