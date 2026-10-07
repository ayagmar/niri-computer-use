//! niri IPC with a deadline per request. `niri_ipc::socket::Socket` has no timeouts, so
//! this sends the same JSON lines over a `UnixStream` and bounds each request's write and
//! reply by one deadline. Connecting to a Unix socket only blocks if niri's accept backlog
//! is full, so the connect itself is not bounded.
//!
//! niri answers any number of requests on one connection, in order
//! (`src/ipc/server.rs`, `handle_client`). The supervisor keeps one connection from the
//! moment it identifies the nested niri until it sends `Quit`, so nothing can swap the
//! socket in between.

use std::io::{self, ErrorKind, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use niri_ipc::{Reply, Request, Response};

use crate::failure::{Context as _, Failure, Result};

const DEADLINE: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub(crate) struct Connection {
    stream: UnixStream,
    /// Bytes received after the last reply's newline.
    unread: Vec<u8>,
}

impl Connection {
    pub(crate) fn open(socket: &Path) -> Result<Self> {
        let stream =
            UnixStream::connect(socket).context(format!("connect to {}", socket.display()))?;
        Ok(Self {
            stream,
            unread: Vec::new(),
        })
    }

    /// Sends one request and returns niri's response. A niri error reply becomes a
    /// failure that keeps niri's message.
    pub(crate) fn request(&mut self, request: &Request) -> Result<Response> {
        self.request_within(request, DEADLINE)
    }

    fn request_within(&mut self, request: &Request, deadline: Duration) -> Result<Response> {
        let doing = format!("niri {request:?}");
        let end = Instant::now() + deadline;
        let mut line = serde_json::to_vec(request).context(&doing)?;
        line.push(b'\n');
        self.write_all(&line, end).context(&doing)?;
        let reply = self.read_line(end).context(&doing)?;
        let reply: Reply = serde_json::from_slice(&reply).context(&doing)?;
        reply.map_err(|message| Failure::new(format!("{doing}: niri replied: {message}")))
    }

    fn write_all(&mut self, mut bytes: &[u8], end: Instant) -> io::Result<()> {
        while !bytes.is_empty() {
            self.stream.set_write_timeout(Some(remaining(end)?))?;
            let written = self.stream.write(bytes)?;
            if written == 0 {
                return Err(io::Error::new(ErrorKind::WriteZero, "niri stopped reading"));
            }
            bytes = bytes.get(written..).unwrap_or_default();
        }
        Ok(())
    }

    /// Returns the next line without its newline, and keeps anything after it.
    fn read_line(&mut self, end: Instant) -> io::Result<Vec<u8>> {
        let mut chunk = [0; 4096];
        loop {
            if let Some(newline) = self.unread.iter().position(|&byte| byte == b'\n') {
                let mut line: Vec<u8> = self.unread.drain(..=newline).collect();
                line.pop();
                return Ok(line);
            }
            self.stream.set_read_timeout(Some(remaining(end)?))?;
            let read = self.stream.read(&mut chunk)?;
            if read == 0 {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "niri closed the connection",
                ));
            }
            self.unread
                .extend_from_slice(chunk.get(..read).unwrap_or_default());
        }
    }
}

fn remaining(end: Instant) -> io::Result<Duration> {
    let left = end.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(io::Error::new(ErrorKind::TimedOut, "deadline passed"))
    } else {
        Ok(left)
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader};
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::thread;

    use super::*;

    /// A one-connection fake niri. For each request line it reads, it sends the next
    /// reply in two writes. With no replies left it goes quiet.
    fn fake_niri(
        name: &str,
        replies: &'static [&'static str],
    ) -> (PathBuf, thread::JoinHandle<()>) {
        let path = std::env::temp_dir().join(format!("harness-niri-{name}-{}", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut connection = stream;
            for reply in replies {
                let mut request = String::new();
                reader.read_line(&mut request).unwrap();
                let (first, rest) = reply.split_at(3);
                connection.write_all(first.as_bytes()).unwrap();
                connection.write_all(rest.as_bytes()).unwrap();
            }
            thread::park_timeout(Duration::from_secs(2));
        });
        (path, server)
    }

    #[test]
    fn answers_several_requests_on_one_connection() {
        const REPLIES: &[&str] = &[
            "{\"Ok\":{\"Version\":\"26.04 (x)\"}}\n",
            "{\"Ok\":\"Handled\"}\n",
        ];
        let (path, server) = fake_niri("ok", REPLIES);
        let mut niri = Connection::open(&path).unwrap();
        let version = niri.request(&Request::Version).unwrap();
        assert!(matches!(version, Response::Version(text) if text == "26.04 (x)"));
        let handled = niri.request(&Request::Version).unwrap();
        assert!(matches!(handled, Response::Handled));
        drop(niri);
        server.thread().unpark();
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn keeps_niri_error_message() {
        let (path, server) = fake_niri("err", &["{\"Err\":\"boom\"}\n"]);
        let message = Connection::open(&path)
            .unwrap()
            .request(&Request::Version)
            .unwrap_err()
            .to_string();
        assert!(message.ends_with("niri replied: boom"), "{message}");
        server.thread().unpark();
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn silent_niri_hits_the_deadline() {
        let (path, server) = fake_niri("silent", &[]);
        let started = Instant::now();
        let mut niri = Connection::open(&path).unwrap();
        let failure = niri.request_within(&Request::Version, Duration::from_millis(200));
        assert!(failure.is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        server.thread().unpark();
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
