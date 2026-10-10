//! One client of the engine, and what its own environment decides for it alone.

use std::ffi::OsString;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::{Semaphore, SemaphorePermit, oneshot, watch};

use crate::policy::{self, Loaded, Unrestricted};

/// What a client's own environment gives its session, and no other: its two variables,
/// its home, and the policy file as read from its config directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Given {
    /// `NIRI_COMPUTER_USE_UNRESTRICTED`, as given: `1` turns `unrestricted` on.
    pub(crate) unrestricted: Option<OsString>,
    /// `NIRI_COMPUTER_USE_KEYBOARD`: the experimental backend; absent means wtype.
    pub(crate) keyboard: Option<OsString>,
    /// `$HOME`, for a `capture_dir` under `~/`.
    pub(crate) home: Option<PathBuf>,
    pub(crate) policy: policy::Source,
}

impl Given {
    /// What `var` reads, with the policy file in `$XDG_CONFIG_HOME`, or `$HOME/.config`,
    /// read now. An empty variable counts as unset.
    pub(crate) fn read(var: impl Fn(&str) -> Option<OsString>) -> Self {
        let var = |name| var(name).filter(|value| !value.is_empty());
        let home = var("HOME").map(PathBuf::from);
        let config_dir = var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".config")));
        Self {
            unrestricted: var("NIRI_COMPUTER_USE_UNRESTRICTED"),
            keyboard: var("NIRI_COMPUTER_USE_KEYBOARD"),
            home,
            policy: policy::Source::read(config_dir.as_deref()),
        }
    }
}

/// A session's settings, fixed when it starts.
#[derive(Debug)]
pub(crate) struct Settings {
    /// The policy file, as the session read it when it started.
    pub(crate) policy: Loaded,
    /// Whether `unrestricted` is on, from the policy file and the session's variable.
    pub(crate) unrestricted: Unrestricted,
    /// The keyboard backend the session asked for.
    pub(crate) keyboard: Option<OsString>,
    /// The session's home, for a `capture_dir` under `~/`.
    pub(crate) home: Option<PathBuf>,
}

impl Settings {
    /// Checks what the session's environment gave.
    pub(crate) fn new(given: Given) -> Self {
        let env_on = Unrestricted::parse_env(given.unrestricted.as_deref());
        let policy = Loaded::from_source(&given.policy, env_on == Ok(true));
        Self {
            unrestricted: Unrestricted {
                policy: policy.unrestricted(),
                env: env_on,
            },
            policy,
            keyboard: given.keyboard,
            home: given.home,
        }
    }
}

/// Which session of the engine: what the lease, its refs and the window to give focus
/// back to belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SessionId(pub(crate) u64);

/// The most tool calls one session may have running at once.
pub(crate) const MAX_IN_FLIGHT: usize = 16;

/// A client: who it is, for the audit log and the lease record, its settings, and whether
/// it has ended. Clones share the ended flag.
#[derive(Debug, Clone)]
pub(crate) struct Session {
    id: SessionId,
    /// The process the client started: this server, or the bridge.
    pid: u32,
    settings: Arc<Settings>,
    ended: Arc<watch::Sender<bool>>,
    in_flight: Arc<Semaphore>,
}

impl Session {
    pub(crate) fn new(id: SessionId, pid: u32, settings: Settings) -> Self {
        Self {
            id,
            pid,
            settings: Arc::new(settings),
            ended: Arc::new(watch::Sender::new(false)),
            in_flight: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
        }
    }

    /// Lets one more tool call run, unless `MAX_IN_FLIGHT` are running already. The call
    /// holds what this returns while it runs.
    pub(crate) fn admit(&self) -> Option<SemaphorePermit<'_>> {
        self.in_flight.try_acquire().ok()
    }

    /// Marks the session ended, for good.
    pub(crate) fn end(&self) {
        self.ended.send_replace(true);
    }

    pub(crate) fn has_ended(&self) -> bool {
        *self.ended.borrow()
    }

    /// Returns once the session has ended.
    pub(crate) async fn ended(&self) {
        let mut ended = self.ended.subscribe();
        // `self` holds the sender, so the wait can't fail.
        ended.wait_for(|ended| *ended).await.map(drop).ok();
    }

    pub(crate) const fn id(&self) -> SessionId {
        self.id
    }

    pub(crate) fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The label for the audit log and the lease: the MCP client's name and the process it
    /// started, such as `claude-code/4711`.
    pub(crate) fn label(&self, client: &str) -> String {
        format!("{client}/{}", self.pid)
    }
}

/// The longest line a client may send, newline included: `paste`'s 1 MiB of text, at up
/// to six bytes a character in JSON, with room to spare.
pub(crate) const MAX_LINE: usize = 16 * 1024 * 1024;

/// A client's input, which says when the client has gone: at its end, on a read error, on
/// a line longer than `MAX_LINE`, which ends the session, or when the transport drops it.
#[derive(Debug)]
pub(crate) struct Incoming<R> {
    input: R,
    gone: Option<oneshot::Sender<()>>,
    /// Bytes read since the last newline.
    line: usize,
}

impl<R> Incoming<R> {
    /// `input`, and what completes once the client has gone.
    pub(crate) fn new(input: R) -> (Self, oneshot::Receiver<()>) {
        let (gone, went) = oneshot::channel();
        let incoming = Self {
            input,
            gone: Some(gone),
            line: 0,
        };
        (incoming, went)
    }

    /// Counts `read` into the current line. Returns whether the line is still short enough.
    fn count(&mut self, read: &[u8]) -> bool {
        self.line = match read.iter().rposition(|byte| *byte == b'\n') {
            Some(newline) => read.len() - newline - 1,
            None => self.line.saturating_add(read.len()),
        };
        self.line <= MAX_LINE
    }

    fn gone(&mut self) {
        if let Some(gone) = self.gone.take() {
            gone.send(()).ok();
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Incoming<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let room = buf.remaining();
        let read = Pin::new(&mut self.input).poll_read(context, buf);
        match &read {
            // Nothing read into room for something is the end.
            Poll::Ready(Ok(())) if room > 0 && buf.remaining() == room => self.gone(),
            Poll::Ready(Ok(())) => {
                if !self.count(buf.filled().get(before..).unwrap_or_default()) {
                    // A read that fails reads nothing.
                    buf.set_filled(before);
                    self.gone();
                    return Poll::Ready(Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("the client sent a line over {MAX_LINE} bytes"),
                    )));
                }
            }
            Poll::Ready(Err(_)) => self.gone(),
            Poll::Pending => {}
        }
        read
    }
}

impl<R> Drop for Incoming<R> {
    fn drop(&mut self) {
        self.gone();
    }
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncReadExt as _;

    use super::*;

    /// Reads all of `input` through `Incoming`. Returns the read's result and whether the
    /// client counts as gone.
    async fn read_all(input: Vec<u8>) -> (std::io::Result<usize>, bool) {
        let (mut incoming, mut gone) = Incoming::new(input.as_slice());
        let read = incoming.read_to_end(&mut Vec::new()).await;
        (read, gone.try_recv().is_ok())
    }

    #[tokio::test]
    async fn a_line_over_the_limit_ends_the_session_and_the_end_of_input_too() {
        let mut lines = vec![b'x'; MAX_LINE - 1];
        lines.push(b'\n');
        lines.extend(vec![b'y'; MAX_LINE]);
        let (whole, gone) = read_all(lines.clone()).await;
        assert_eq!(whole.unwrap(), 2 * MAX_LINE);
        assert!(gone);
        lines.push(b'z');
        let (over, ended) = read_all(lines).await;
        assert_eq!(over.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert!(ended);
    }

    #[test]
    fn the_policy_comes_from_the_sessions_own_config_directory() {
        let dir = crate::test_support::fresh_dir("session-given");
        for config in ["config", "home/.config"] {
            let file = dir.join(config).join("niri-computer-use");
            std::fs::create_dir_all(&file).unwrap();
            std::fs::write(file.join("policy.toml"), format!("# {config}")).unwrap();
        }
        let text = |given: Given| {
            let policy::Source::Text { text, .. } = given.policy else {
                panic!("{:?}", given.policy);
            };
            text
        };
        let home = dir.join("home").into_os_string();
        let config = dir.join("config").into_os_string();
        let both = Given::read(|name| match name {
            "HOME" => Some(home.clone()),
            "XDG_CONFIG_HOME" => Some(config.clone()),
            _ => None,
        });
        assert_eq!(both.home, Some(dir.join("home")));
        assert_eq!(text(both), "# config");
        // An empty variable counts as unset, so the file under `$HOME` is read.
        let home_only = Given::read(|name| match name {
            "HOME" => Some(home.clone()),
            "XDG_CONFIG_HOME" | "NIRI_COMPUTER_USE_UNRESTRICTED" => Some(OsString::new()),
            _ => None,
        });
        assert_eq!(home_only.unrestricted, None);
        assert_eq!(text(home_only), "# home/.config");
        assert_eq!(Given::read(|_| None).policy, policy::Source::NoConfigDir);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
