//! One client of the engine, and what its own environment decides for it alone.

use std::ffi::OsString;
use std::os::fd::{AsRawFd as _, OwnedFd};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use rustix::fs::{Mode, OFlags};
use tokio::io::{AsyncRead, ReadBuf};
use tokio::sync::{Semaphore, SemaphorePermit, oneshot, watch};

use crate::policy::{self, Loaded, Switch, Tools};
use crate::runner::Place;

/// What a client's own environment gives its session, and no other: its three variables,
/// its home, and the policy file as read from its config directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Given {
    /// `NIRI_COMPUTER_USE_UNRESTRICTED`, as given: `1` turns `unrestricted` on.
    pub(crate) unrestricted: Option<OsString>,
    /// `NIRI_COMPUTER_USE_CONFIRM_LEASE`, as given: `1` turns `confirm_lease` on.
    pub(crate) confirm_lease: Option<OsString>,
    /// `NIRI_COMPUTER_USE_TOOLS`, as given: the profile of tools to list.
    pub(crate) tools: Option<OsString>,
    /// `$HOME`, for a `capture_dir` under `~/`.
    pub(crate) home: Option<PathBuf>,
    /// `$PATH`, where the session's own programs, such as `grim`, are found. Empty
    /// isn't unset: it means the working directory.
    pub(crate) path: Option<OsString>,
    pub(crate) policy: policy::Source,
}

impl Given {
    /// What `var` reads, with the policy file in `$XDG_CONFIG_HOME`, or `$HOME/.config`,
    /// read now. An empty variable counts as unset, except `PATH`.
    pub(crate) fn read(var: impl Fn(&str) -> Option<OsString>) -> Self {
        let path = var("PATH");
        let var = |name| var(name).filter(|value| !value.is_empty());
        let home = var("HOME").map(PathBuf::from);
        let config_dir = var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(".config")));
        Self {
            unrestricted: var("NIRI_COMPUTER_USE_UNRESTRICTED"),
            confirm_lease: var("NIRI_COMPUTER_USE_CONFIRM_LEASE"),
            tools: var("NIRI_COMPUTER_USE_TOOLS"),
            home,
            path,
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
    pub(crate) unrestricted: Switch,
    /// Whether the user must allow each `acquire_desktop`, from the policy file and the
    /// session's variable.
    pub(crate) confirm_lease: Switch,
    /// Which tools the session lists, from the policy file and the session's variable.
    pub(crate) tools: Tools,
    /// The session's home, for a `capture_dir` under `~/`.
    pub(crate) home: Option<PathBuf>,
    /// The session's `PATH`, where `grim` is found for its calls.
    pub(crate) path: Option<OsString>,
    /// A shared engine's client's working directory, where those calls start and
    /// `PATH`'s relative entries are. `None` in a standalone server, whose calls start in
    /// its own.
    pub(crate) dir: Option<Dir>,
}

impl Settings {
    /// Checks what the session's environment gave.
    pub(crate) fn new(given: Given) -> Self {
        let env_on = Switch::parse_env(
            "NIRI_COMPUTER_USE_UNRESTRICTED",
            "unrestricted",
            false,
            given.unrestricted.as_deref(),
        );
        let policy = Loaded::from_source(&given.policy, env_on == Ok(true));
        Self {
            unrestricted: Switch {
                policy: policy.unrestricted(),
                env: env_on,
                when_wrong: false,
            },
            confirm_lease: Switch {
                policy: policy.confirm_lease(),
                env: Switch::parse_env(
                    "NIRI_COMPUTER_USE_CONFIRM_LEASE",
                    "confirm_lease",
                    true,
                    given.confirm_lease.as_deref(),
                ),
                when_wrong: true,
            },
            tools: Tools {
                policy: policy.tools(),
                env: Tools::parse_env(given.tools.as_deref()),
            },
            policy,
            home: given.home,
            path: given.path,
            dir: None,
        }
    }

    /// Where the session's own programs are found.
    pub(crate) fn place(&self) -> Place<'_> {
        Place {
            path: self.path.as_deref(),
            cwd: self.dir.as_ref().map(|dir| dir.path.as_path()),
        }
    }
}

/// A client's working directory, held open, so that renaming or removing its path, or
/// making another directory there, can't move the session's calls elsewhere. The bridge
/// never changes its directory.
#[derive(Debug)]
pub(crate) struct Dir {
    _open: OwnedFd,
    /// The directory through the open descriptor: `/proc/self/fd/<fd>`, which is this
    /// process's and, until they start their program, its children's.
    path: PathBuf,
}

impl Dir {
    /// The working directory of the process `pid`, as it is now.
    pub(crate) fn of_process(pid: u32) -> Result<Self, String> {
        let flags = OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC;
        let open = rustix::fs::open(format!("/proc/{pid}/cwd"), flags, Mode::empty())
            .map_err(|error| format!("open the working directory of process {pid}: {error}"))?;
        let path = PathBuf::from(format!("/proc/self/fd/{}", open.as_raw_fd()));
        Ok(Self { _open: open, path })
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

    /// The label for the audit log, the lease and the lease prompt, from the name the MCP
    /// client gives itself.
    pub(crate) fn label(&self, client: &str) -> Label {
        Label {
            client: shown_name(client),
            pid: self.pid,
        }
    }
}

/// The longest client name a label shows, in characters.
const CLIENT_NAME: usize = 64;

/// Who a session's calls come from, shown as `claude-code/4711`: the name the MCP client
/// gives itself, which nothing checks, made safe to show (`shown_name`), and the process
/// it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Label {
    client: String,
    pid: u32,
}

impl Label {
    /// The name the client gives itself, as shown.
    pub(crate) fn client(&self) -> &str {
        &self.client
    }

    pub(crate) const fn pid(&self) -> u32 {
        self.pid
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.client, self.pid)
    }
}

/// `name` as a label shows it: its first `CLIENT_NAME` characters, with every one other
/// than an ASCII letter, digit, space, `.`, `_`, `-`, `+` or `@` as `_`, and `unknown` when
/// nothing is left. So a name can't hold a `/` or a character that looks like one and
/// pass for another `name/pid`, start a line, or fill the lease record or the prompt.
fn shown_name(name: &str) -> String {
    let shown: String = name
        .chars()
        .take(CLIENT_NAME)
        .map(|c| {
            if c.is_ascii_alphanumeric() || " ._-+@".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let shown = shown.trim();
    if shown.is_empty() {
        "unknown".to_owned()
    } else {
        shown.to_owned()
    }
}

/// The longest line a client may send, newline included: a paste's 1 MiB of text, at up
/// to six bytes a character in JSON, with room to spare.
pub(crate) const MAX_LINE: usize = 16 * 1024 * 1024;

/// Counts a stream into lines and says when one is over `max` bytes, newline included,
/// however the stream was split into reads.
#[derive(Debug)]
pub(crate) struct LineLimit {
    max: usize,
    /// Bytes counted since the last newline.
    line: usize,
}

impl LineLimit {
    pub(crate) const fn new(max: usize) -> Self {
        Self { max, line: 0 }
    }

    /// Counts `read`. Returns whether every line it ends, and the one it leaves unfinished,
    /// is within the limit so far.
    pub(crate) fn count(&mut self, read: &[u8]) -> bool {
        let mut rest = read;
        while let Some(newline) = rest.iter().position(|byte| *byte == b'\n') {
            if self.line.saturating_add(newline + 1) > self.max {
                return false;
            }
            self.line = 0;
            rest = rest.get(newline + 1..).unwrap_or_default();
        }
        self.line = self.line.saturating_add(rest.len());
        self.line <= self.max
    }
}

/// A client's input, which says when the client has gone: at its end, on a read error, on
/// a line longer than `MAX_LINE`, which ends the session, or when the transport drops it.
#[derive(Debug)]
pub(crate) struct Incoming<R> {
    input: R,
    gone: Option<oneshot::Sender<()>>,
    lines: LineLimit,
}

impl<R> Incoming<R> {
    /// `input`, and what completes once the client has gone.
    pub(crate) fn new(input: R) -> (Self, oneshot::Receiver<()>) {
        let (gone, went) = oneshot::channel();
        let incoming = Self {
            input,
            gone: Some(gone),
            lines: LineLimit::new(MAX_LINE),
        };
        (incoming, went)
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
                if !self
                    .lines
                    .count(buf.filled().get(before..).unwrap_or_default())
                {
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

    // The label's name is the client's own claim, shown in the audit log, to other agents
    // as the lease holder and to the user in the lease prompt. It can't pass for another
    // `name/pid` through a `/` or a character that looks like one, can't start a line, and
    // can't fill those with a long name.
    #[test]
    fn a_clients_name_cant_pass_for_another_label_or_fill_it() {
        let session = crate::test_support::session(1);
        let pid = std::process::id();
        let shown = |name: &str| session.label(name).to_string();
        assert_eq!(shown("claude-code"), format!("claude-code/{pid}"));
        assert_eq!(shown("Claude Code 2.1"), format!("Claude Code 2.1/{pid}"));
        for slash in ["evil/1", "evil\u{2215}1", "evil\u{2044}1", "evil\u{ff0f}1"] {
            assert_eq!(shown(slash), format!("evil_1/{pid}"), "{slash}");
        }
        assert_eq!(shown("a\nb\u{1b}[2J"), format!("a_b__2J/{pid}"));
        assert_eq!(session.label(&"x".repeat(1000)).client(), "x".repeat(64));
        assert_eq!(shown(""), format!("unknown/{pid}"));
        assert_eq!(shown(" \t "), format!("_/{pid}"));
    }

    /// The engine refuses a client whose directory it can't hold, rather than start its
    /// calls in its own: process 0 is what a peer without credentials counts as.
    #[test]
    fn a_process_without_a_directory_has_no_dir() {
        let error = Dir::of_process(0).unwrap_err();
        assert!(
            error.starts_with("open the working directory of process 0"),
            "{error}"
        );
    }

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

    #[tokio::test]
    async fn a_line_over_the_limit_in_the_middle_of_a_read_ends_the_session() {
        let mut lines = b"{}\n".to_vec();
        lines.extend(vec![b'x'; MAX_LINE]);
        lines.extend(b"\n{}\n");
        let (over, ended) = read_all(lines).await;
        assert_eq!(over.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
        assert!(ended);
    }

    #[test]
    fn a_line_counts_with_its_newline_however_the_input_is_split() {
        // At most four bytes a line, newline included.
        let within = |input: &[u8]| {
            let whole = LineLimit::new(4).count(input);
            for split in 0..=input.len() {
                let (first, second) = input.split_at(split);
                let mut limit = LineLimit::new(4);
                let split_within = limit.count(first) && limit.count(second);
                assert_eq!(split_within, whole, "{input:?} split at {split}");
            }
            let mut limit = LineLimit::new(4);
            let bytewise = input.iter().all(|byte| limit.count(&[*byte]));
            assert_eq!(bytewise, whole, "{input:?} a byte at a time");
            whole
        };
        assert!(within(b"abc\n"));
        assert!(!within(b"abcd\n"));
        // A line not yet finished may reach the limit, but not its newline after.
        assert!(within(b"abcd"));
        assert!(!within(b"ab\nabcd\nab\n"));
        assert!(within(b"ab\nabc\na\nabc"));
    }

    /// A typo in the variable that asks for the lease prompt must not drop the prompt, while
    /// one in the variable that grants `unrestricted` grants nothing.
    #[test]
    fn a_wrong_value_keeps_the_prompt_on_and_unrestricted_off() {
        let settings = Settings::new(Given {
            unrestricted: Some("true".into()),
            confirm_lease: Some("true".into()),
            tools: None,
            home: None,
            path: None,
            policy: policy::Source::Missing,
        });
        assert!(settings.confirm_lease.enabled());
        assert!(!settings.unrestricted.enabled());
        let prompt = serde_json::to_value(settings.confirm_lease.status()).unwrap();
        assert_eq!(prompt["source"], "env");
        let error = prompt["error"].as_str().unwrap();
        assert!(error.contains("NIRI_COMPUTER_USE_CONFIRM_LEASE"), "{error}");
        assert!(error.contains("on until the value is fixed"), "{error}");
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
