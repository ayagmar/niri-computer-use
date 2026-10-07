//! M0 probe: sends one allowlisted command to a Noctalia IPC socket and prints the reply.
//!
//! Noctalia 5.2.1's own client writes `<cwd>\x1e<command>`, shuts down its write half and
//! reads the reply until EOF (`src/ipc/ipc_client.cpp`). The service erases everything up
//! to the first `\x1e` before it parses the command, and falls back to newline framing
//! when a read waits longer than 100 ms (`src/ipc/ipc_service.cpp`). So the probe writes the
//! whole payload before the shutdown, and the payload is always `/\x1e` followed by one of
//! `COMMANDS`. Nothing from the command line is copied into it.
//!
//! The probe sends to whatever socket it is given. Run it only through `make nested`, which
//! passes the nested Noctalia's socket after checking that it lies under `TEST_DIR/run`.

use std::env;
use std::error::Error;
use std::ffi::OsString;
use std::io::{self, ErrorKind, Read as _, Write as _};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

const USAGE: &str = "usage: noctalia-socket <socket> <command>
commands:
  status
  panel-open control-center
  panel-close control-center";

/// Every command the probe can send. The command line picks one by exact match.
const COMMANDS: [&str; 3] = [
    "status",
    "panel-open control-center",
    "panel-close control-center",
];
/// The caller's working directory and the separator, as Noctalia's client sends them. `/`
/// is always an absolute directory, so the service accepts it as the caller's cwd.
const CWD_PREFIX: &[u8] = b"/\x1e";
/// The Noctalia command deadline from plan §6, for connect, write and the whole reply.
const DEADLINE: Duration = Duration::from_secs(2);

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    match run(&args, DEADLINE) {
        Ok(reply) => {
            print!("{reply}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("noctalia-socket: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[OsString], deadline: Duration) -> Result<String> {
    let [socket, words @ ..] = args else {
        return Err(USAGE.into());
    };
    let words = words
        .iter()
        .map(|word| word.to_str().ok_or(USAGE))
        .collect::<std::result::Result<Vec<&str>, _>>()?;
    let command = allowlisted(&words.join(" ")).ok_or(USAGE)?;
    let socket = Path::new(socket);
    let reply = exchange(socket, &payload(command), deadline)
        .map_err(|error| format!("{command:?} on {}: {error}", socket.display()))?;
    let reply = String::from_utf8(reply).map_err(|error| format!("{command:?}: reply: {error}"))?;
    if reply.is_empty() {
        return Err(format!("{command:?}: Noctalia closed the connection without a reply").into());
    }
    if reply.starts_with("error:") {
        return Err(format!("{command:?}: Noctalia replied: {}", reply.trim_end()).into());
    }
    Ok(reply)
}

/// The allowlisted constant equal to `text`, never `text` itself.
fn allowlisted(text: &str) -> Option<&'static str> {
    COMMANDS.into_iter().find(|command| *command == text)
}

fn payload(command: &'static str) -> Vec<u8> {
    [CWD_PREFIX, command.as_bytes()].concat()
}

/// Connects, writes the whole payload, shuts down the write half, and reads until EOF, all
/// before one deadline.
fn exchange(socket: &Path, payload: &[u8], deadline: Duration) -> io::Result<Vec<u8>> {
    let end = Instant::now() + deadline;
    let mut stream = connect(socket.to_path_buf(), end)?;
    let mut unsent = payload;
    while !unsent.is_empty() {
        stream.set_write_timeout(Some(remaining(end)?))?;
        let written = stream
            .write(unsent)
            .map_err(|error| late(error, deadline, 0))?;
        if written == 0 {
            return Err(io::Error::new(
                ErrorKind::WriteZero,
                "Noctalia stopped reading",
            ));
        }
        unsent = unsent.get(written..).unwrap_or_default();
    }
    stream.shutdown(Shutdown::Write)?;
    let mut reply = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        stream.set_read_timeout(Some(remaining(end)?))?;
        let read = stream
            .read(&mut chunk)
            .map_err(|error| late(error, deadline, reply.len()))?;
        if read == 0 {
            return Ok(reply);
        }
        reply.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }
}

/// `UnixStream::connect` has no timeout, and blocks while the listener's backlog is full,
/// so it runs on its own thread. A thread still blocked at the deadline ends with the
/// process.
fn connect(socket: PathBuf, end: Instant) -> io::Result<UnixStream> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || sender.send(UnixStream::connect(socket)));
    match receiver.recv_timeout(remaining(end)?) {
        Ok(connected) => connected,
        Err(RecvTimeoutError::Timeout) => Err(io::Error::new(
            ErrorKind::TimedOut,
            "could not connect before the deadline",
        )),
        Err(RecvTimeoutError::Disconnected) => Err(io::Error::other("the connect thread stopped")),
    }
}

/// A socket timeout shows up as `WouldBlock`; say which deadline passed instead.
fn late(error: io::Error, deadline: Duration, received: usize) -> io::Error {
    match error.kind() {
        ErrorKind::WouldBlock | ErrorKind::TimedOut => io::Error::new(
            ErrorKind::TimedOut,
            format!("no complete reply within {deadline:?} ({received} bytes received)"),
        ),
        _ => error,
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
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::thread;

    use super::*;

    fn args(socket: &Path, command: &[&str]) -> Vec<OsString> {
        let mut args = vec![socket.as_os_str().to_owned()];
        args.extend(command.iter().map(OsString::from));
        args
    }

    /// A one-connection fake Noctalia. It reads the request until the client's EOF, as the
    /// real service does, then sends `reply` in two writes and closes. With `None` it
    /// holds the connection open without replying.
    fn fake_noctalia(
        name: &str,
        reply: Option<&'static str>,
    ) -> (PathBuf, thread::JoinHandle<Vec<u8>>) {
        let path = env::temp_dir().join(format!("noctalia-socket-{name}-{}", std::process::id()));
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).unwrap();
            match reply {
                Some(reply) => {
                    let (first, rest) = reply.split_at(reply.len() / 2);
                    stream.write_all(first.as_bytes()).unwrap();
                    stream.write_all(rest.as_bytes()).unwrap();
                }
                None => thread::park_timeout(Duration::from_secs(2)),
            }
            request
        });
        (path, server)
    }

    #[test]
    fn payloads_are_the_cwd_prefix_and_an_allowlisted_constant() {
        for command in COMMANDS {
            assert!(!command.contains(['\x1e', '\n', '\r']), "{command:?}");
            let payload = payload(command);
            assert!(payload.starts_with(b"/\x1e"));
            assert_eq!(payload.split(|&byte| byte == 0x1e).count(), 2);
        }
        assert_eq!(payload("status"), b"/\x1estatus");
    }

    #[test]
    fn only_exact_allowlisted_commands_are_accepted() {
        assert_eq!(allowlisted("status"), Some("status"));
        assert_eq!(
            allowlisted("panel-open control-center"),
            Some("panel-open control-center")
        );
        for text in [
            "",
            "session lock",
            "session shutdown",
            "panel-open launcher",
            "panel-open session",
            "panel-open control-center audio",
            "panel-open  control-center",
            "status\x1esession lock",
            "status\nsession lock",
            "status ",
            "STATUS",
        ] {
            assert_eq!(allowlisted(text), None, "{text:?}");
        }
    }

    #[test]
    fn rejects_other_commands_before_connecting() {
        let missing = Path::new("/nonexistent/noctalia.sock");
        for command in [&["session", "lock"][..], &["panel-open", "launcher"], &[]] {
            let error = run(&args(missing, command), DEADLINE).unwrap_err();
            assert!(error.to_string().starts_with("usage:"), "{error}");
        }
        assert!(run(&[], DEADLINE).is_err());
    }

    #[test]
    fn writes_the_whole_payload_then_reads_the_reply_until_eof() {
        let (path, server) = fake_noctalia("ok", Some("{\"activePanelId\": null}\n"));
        let reply = run(&args(&path, &["panel-open", "control-center"]), DEADLINE).unwrap();
        assert_eq!(reply, "{\"activePanelId\": null}\n");
        assert_eq!(server.join().unwrap(), b"/\x1epanel-open control-center");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn keeps_noctalia_error_replies() {
        let (path, server) = fake_noctalia("error", Some("error: unknown panel \"x\"\n"));
        let error = run(&args(&path, &["status"]), DEADLINE).unwrap_err();
        assert_eq!(
            error.to_string(),
            "\"status\": Noctalia replied: error: unknown panel \"x\""
        );
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn an_empty_reply_is_an_error() {
        let (path, server) = fake_noctalia("empty", Some(""));
        let error = run(&args(&path, &["status"]), DEADLINE).unwrap_err();
        assert!(error.to_string().contains("without a reply"), "{error}");
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_silent_noctalia_hits_the_deadline() {
        let (path, server) = fake_noctalia("silent", None);
        let started = Instant::now();
        let error = run(&args(&path, &["status"]), Duration::from_millis(200)).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(
            error
                .to_string()
                .ends_with("no complete reply within 200ms (0 bytes received)"),
            "{error}"
        );
        server.thread().unpark();
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn a_connect_blocked_by_a_full_backlog_hits_the_deadline() {
        // The listener never accepts. A closed client's connection stays in the
        // listener's queue until it is accepted, so dropping each stream fills the backlog
        // (SOMAXCONN) without holding a descriptor per connection. Then `connect` blocks.
        let path = env::temp_dir().join(format!("noctalia-socket-backlog-{}", std::process::id()));
        let _listener = UnixListener::bind(&path).unwrap();
        let mut queued = 0;
        let blocked = loop {
            assert!(queued < 100_000, "the backlog never filled");
            let started = Instant::now();
            match connect(path.clone(), started + Duration::from_millis(100)) {
                Ok(stream) => {
                    drop(stream);
                    queued += 1;
                }
                Err(error) => break (error, started.elapsed()),
            }
        };
        assert_eq!(blocked.0.kind(), ErrorKind::TimedOut, "{}", blocked.0);
        assert!(blocked.1 < Duration::from_secs(1));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn socket_timeouts_name_the_deadline_and_the_bytes_received() {
        assert_eq!(
            late(io::Error::from(ErrorKind::WouldBlock), DEADLINE, 7).to_string(),
            "no complete reply within 2s (7 bytes received)"
        );
        let other = late(io::Error::from(ErrorKind::BrokenPipe), DEADLINE, 0);
        assert_eq!(other.kind(), ErrorKind::BrokenPipe);
    }

    #[test]
    fn a_non_utf8_command_is_a_usage_error() {
        use std::os::unix::ffi::OsStrExt as _;
        let bad = std::ffi::OsStr::from_bytes(b"stat\xffus").to_owned();
        let error = run(&[OsString::from("/nonexistent"), bad], DEADLINE).unwrap_err();
        assert!(error.to_string().starts_with("usage:"), "{error}");
    }

    #[test]
    fn keeps_the_io_error_of_a_missing_socket() {
        let error = run(
            &args(Path::new("/nonexistent/noctalia.sock"), &["status"]),
            DEADLINE,
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.starts_with("\"status\" on /nonexistent/noctalia.sock: No such file"),
            "{error}"
        );
    }
}
