//! One short-lived connection per request. niri's replies carry no request id, so a
//! late reply on a reused connection could be read as the answer to the next request.
//! A fresh connection, dropped at the deadline, can't mix them up. `niri msg` works the
//! same way.

use std::path::Path;
use std::time::Duration;

use niri_ipc::{Reply, Request, Response};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, AsyncWrite, AsyncWriteExt as _, BufReader};
use tokio::net::UnixStream;

use crate::error::{ErrorName, ToolError, Unanswered};

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

/// The PID of the process listening on niri's socket, from the connection's peer
/// credentials. Nothing is sent; niri sees the connection close.
pub(crate) async fn peer_pid(socket: &Path) -> Result<u32, ToolError> {
    let connect = UnixStream::connect(socket);
    let stream = tokio::time::timeout(DEADLINE, connect)
        .await
        .map_err(|_| {
            ToolError::new(
                ErrorName::DeadlineExceeded,
                format!(
                    "connect to {}: no answer within {DEADLINE:?}",
                    socket.display()
                ),
            )
        })?
        .map_err(|error| {
            ToolError::new(
                ErrorName::NiriUnavailable,
                format!("connect to {}: {error}", socket.display()),
            )
        })?;
    let credentials = stream.peer_cred().map_err(|error| {
        ToolError::new(
            ErrorName::UpstreamError,
            format!("read niri's credentials: {error}"),
        )
    })?;
    credentials
        .pid()
        .and_then(|pid| u32::try_from(pid).ok())
        .ok_or_else(|| ToolError::new(ErrorName::UpstreamError, "niri's socket gave no PID"))
}

/// Sends one request that changes something, on a new connection, within two seconds,
/// and says whether niri can have carried it out when there is no answer.
pub(crate) async fn dispatch(socket: &Path, request: &Request) -> Result<Response, Unanswered> {
    dispatch_within(DEADLINE, socket, request).await
}

async fn dispatch_within(
    limit: Duration,
    socket: &Path,
    request: &Request,
) -> Result<Response, Unanswered> {
    let deadline = tokio::time::Instant::now() + limit;
    let doing = format!("niri {request:?}");
    let late = |what: &str| {
        ToolError::new(
            ErrorName::DeadlineExceeded,
            format!("{doing}: {what} within {limit:?}"),
        )
    };
    let stream = tokio::time::timeout_at(deadline, UnixStream::connect(socket))
        .await
        .map_err(|_| Unanswered::Refused(late("niri didn't accept the connection")))?
        .map_err(|error| {
            Unanswered::Refused(ToolError::new(
                ErrorName::NiriUnavailable,
                format!("connect to {}: {error}", socket.display()),
            ))
        })?;
    let mut stream = BufReader::new(stream);
    tokio::time::timeout_at(deadline, write(&mut stream, request))
        .await
        .map_err(|_| Unanswered::Refused(late("the request couldn't be sent")))?
        .map_err(Unanswered::Refused)?;
    match tokio::time::timeout_at(deadline, read(&mut stream, request)).await {
        Err(_) => Err(Unanswered::Lost(late("no reply"))),
        Ok(Err(error)) => Err(Unanswered::Lost(error)),
        Ok(Ok(Err(message))) => Err(Unanswered::Refused(ToolError::new(
            ErrorName::UpstreamError,
            format!("{doing}: niri replied: {message}"),
        ))),
        Ok(Ok(Ok(response))) => Ok(response),
    }
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
    let mut stream = BufReader::new(stream);
    write(&mut stream, request).await?;
    read(&mut stream, request).await?.map_err(|message| {
        ToolError::new(
            ErrorName::UpstreamError,
            format!("niri {request:?}: niri replied: {message}"),
        )
    })
}

async fn write(
    stream: &mut BufReader<impl AsyncRead + AsyncWrite + Unpin>,
    request: &Request,
) -> Result<(), ToolError> {
    let doing = format!("niri {request:?}");
    let mut line = serde_json::to_vec(request)
        .map_err(|error| ToolError::new(ErrorName::UpstreamError, format!("{doing}: {error}")))?;
    line.push(b'\n');
    stream
        .get_mut()
        .write_all(&line)
        .await
        .map_err(|error| ToolError::new(ErrorName::NiriUnavailable, format!("{doing}: {error}")))
}

/// niri's reply line: its response, or the message it refused the request with.
async fn read(
    stream: &mut BufReader<impl AsyncRead + AsyncWrite + Unpin>,
    request: &Request,
) -> Result<Reply, ToolError> {
    let doing = format!("niri {request:?}");
    let mut reply = String::new();
    let read = stream
        .read_line(&mut reply)
        .await
        .map_err(|error| ToolError::new(ErrorName::NiriUnavailable, format!("{doing}: {error}")))?;
    if read == 0 {
        return Err(ToolError::new(
            ErrorName::NiriUnavailable,
            format!("{doing}: niri closed the connection without a reply"),
        ));
    }
    serde_json::from_str(&reply).map_err(|error| {
        ToolError::new(
            ErrorName::UpstreamError,
            format!("{doing}: unreadable reply: {error}"),
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

    /// A niri on a socket of its own that reads one request per connection and then
    /// writes `reply`, or closes without a reply for `None`, or holds the connection for
    /// `Some("")`.
    fn niri(name: &str, reply: Option<&'static str>) -> std::path::PathBuf {
        let dir = Path::new("/tmp").join(format!(
            "ncu-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let socket = dir.join("niri.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        tokio::spawn(async move {
            while let Ok((connection, _)) = listener.accept().await {
                tokio::spawn(answer(connection, reply));
            }
        });
        socket
    }

    async fn answer(connection: UnixStream, reply: Option<&'static str>) {
        let mut connection = BufReader::new(connection);
        let mut line = String::new();
        connection.read_line(&mut line).await.unwrap();
        match reply {
            None => {}
            Some("") => std::future::pending::<()>().await,
            Some(reply) => connection.write_all(reply.as_bytes()).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn a_dispatch_says_whether_niri_can_have_carried_it_out() {
        let focus = Request::Action(niri_ipc::Action::FocusWindow { id: 3 });
        let limit = Duration::from_millis(200);
        let handled = niri("handled", Some("{\"Ok\":\"Handled\"}\n"));
        assert!(matches!(
            dispatch_within(limit, &handled, &focus).await,
            Ok(Response::Handled)
        ));
        let refused = niri("refused", Some("{\"Err\":\"no\"}\n"));
        let Err(Unanswered::Refused(error)) = dispatch_within(limit, &refused, &focus).await else {
            panic!("niri's refusal isn't Refused");
        };
        assert_eq!(error.name, ErrorName::UpstreamError);
        assert!(error.detail.ends_with("niri replied: no"), "{error:?}");
        let missing = handled.with_file_name("missing.sock");
        assert!(matches!(
            dispatch_within(limit, &missing, &focus).await,
            Err(Unanswered::Refused(ToolError {
                name: ErrorName::NiriUnavailable,
                ..
            }))
        ));
        let closed = niri("closed", None);
        assert!(matches!(
            dispatch_within(limit, &closed, &focus).await,
            Err(Unanswered::Lost(ToolError {
                name: ErrorName::NiriUnavailable,
                ..
            }))
        ));
        let silent = niri("silent", Some(""));
        assert!(matches!(
            dispatch_within(limit, &silent, &focus).await,
            Err(Unanswered::Lost(ToolError {
                name: ErrorName::DeadlineExceeded,
                ..
            }))
        ));
        let garbled = niri("garbled", Some("not json\n"));
        assert!(matches!(
            dispatch_within(limit, &garbled, &focus).await,
            Err(Unanswered::Lost(ToolError {
                name: ErrorName::UpstreamError,
                ..
            }))
        ));
        for socket in [handled, refused, closed, silent, garbled] {
            std::fs::remove_dir_all(socket.parent().unwrap()).unwrap();
        }
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
