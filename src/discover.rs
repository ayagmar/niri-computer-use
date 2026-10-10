//! Finds the session variables a client didn't pass on. Clients such as Codex start MCP
//! servers with a short allow-list of variables that leaves out `XDG_RUNTIME_DIR`,
//! `NIRI_SOCKET` and `WAYLAND_DISPLAY`. A variable that is set always wins; discovery
//! fills in only what is missing, from the way logind and niri lay out a session:
//!
//! - the runtime directory is the one holding a given `NIRI_SOCKET`, because niri puts its
//!   socket in its own runtime directory (`socket_dir` in niri v26.04 `src/ipc/server.rs`);
//!   without one, it is `/run/user/<euid>`, which logind creates for the user with mode
//!   `0700`. Either must belong to the user and have mode `0700`;
//! - niri's socket is `niri.<display>.<pid>.sock` in niri's runtime directory
//!   (`IpcServer::start` in niri v26.04 `src/ipc/server.rs`), and it counts only while
//!   process `<pid>` is a running niri that accepts a connection on it;
//! - the Wayland display is that `<display>`, a socket in the runtime directory.
//!
//! Parsing a socket's name and choosing among the sockets are pure. The reads take their
//! roots as parameters, so tests use directories of their own; the connections go only to
//! the sockets found there.

use std::ffi::{OsStr, OsString};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::control::procs;
use crate::niri;

/// Where a session variable's value came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "source", content = "detail")]
pub(crate) enum Source {
    Environment,
    Discovered,
    /// Neither: why discovery found nothing.
    Missing(String),
}

/// Where each discoverable variable came from, for `status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Sources {
    pub(crate) runtime_dir: Source,
    pub(crate) niri_socket: Source,
    pub(crate) wayland_display: Source,
    /// A doubt about the values that doesn't stop the server from using them.
    pub(crate) warning: Option<String>,
}

impl Default for Sources {
    /// Nothing set and nothing looked for.
    fn default() -> Self {
        let unset = |name: &str| Source::Missing(format!("{name} is not set"));
        Self {
            runtime_dir: unset("XDG_RUNTIME_DIR"),
            niri_socket: unset("NIRI_SOCKET"),
            wayland_display: unset("WAYLAND_DISPLAY"),
            warning: None,
        }
    }
}

/// The variables as the environment gives them.
#[derive(Debug, Default)]
pub(crate) struct Given {
    pub(crate) runtime_dir: Option<PathBuf>,
    pub(crate) niri_socket: Option<PathBuf>,
    pub(crate) wayland_display: Option<OsString>,
}

/// Where discovery looks: `/run/user` and `/proc` on a host, for the effective user.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Roots<'a> {
    pub(crate) run_user: &'a Path,
    pub(crate) proc: &'a Path,
    pub(crate) euid: u32,
}

impl Roots<'static> {
    pub(crate) fn host() -> Self {
        Self {
            run_user: Path::new("/run/user"),
            proc: Path::new("/proc"),
            euid: rustix::process::geteuid().as_raw(),
        }
    }
}

/// The session: each variable given or discovered, or why it is neither.
#[derive(Debug)]
pub(crate) struct Session {
    pub(crate) runtime_dir: Option<PathBuf>,
    pub(crate) niri_socket: Result<PathBuf, String>,
    pub(crate) wayland_display: Option<OsString>,
    pub(crate) sources: Sources,
}

/// How long a socket found in the runtime directory may take to accept a connection.
const CONNECT_DEADLINE: Duration = Duration::from_millis(500);

/// Fills in what `given` lacks.
pub(crate) async fn session(given: Given, roots: &Roots<'_>) -> Session {
    let warning = misplaced(given.runtime_dir.as_deref(), given.niri_socket.as_deref());
    let (runtime_dir, runtime_source) = pick(given.runtime_dir, || {
        given.niri_socket.as_deref().map_or_else(
            || runtime_dir(roots.run_user, roots.euid),
            |socket| socket_runtime_dir(socket, roots.euid),
        )
    });
    let runtime_dir = runtime_dir.ok();
    let (niri_socket, niri_source) = if let Some(socket) = given.niri_socket {
        (Ok(socket), Source::Environment)
    } else {
        let display = given.wayland_display.as_deref();
        found(niri_socket(runtime_dir.as_deref(), display, roots).await)
    };
    let (wayland_display, wayland_source) = pick(given.wayland_display, || {
        wayland_display(niri_socket.as_deref(), runtime_dir.as_deref())
    });
    Session {
        runtime_dir,
        niri_socket,
        wayland_display: wayland_display.ok(),
        sources: Sources {
            runtime_dir: runtime_source,
            niri_socket: niri_source,
            wayland_display: wayland_source,
            warning,
        },
    }
}

/// The given value, or else what `discover` finds.
fn pick<T>(
    given: Option<T>,
    discover: impl FnOnce() -> Result<T, String>,
) -> (Result<T, String>, Source) {
    if let Some(value) = given {
        return (Ok(value), Source::Environment);
    }
    found(discover())
}

/// What discovery found, or why it found nothing.
fn found<T>(discovered: Result<T, String>) -> (Result<T, String>, Source) {
    match discovered {
        Ok(value) => (Ok(value), Source::Discovered),
        Err(detail) => (Err(detail.clone()), Source::Missing(detail)),
    }
}

/// `<run_user>/<euid>`, if it is a private directory of `euid`'s, as logind creates it.
fn runtime_dir(run_user: &Path, euid: u32) -> Result<PathBuf, String> {
    let dir = run_user.join(euid.to_string());
    private(&dir, euid)
        .map(|()| dir.clone())
        .map_err(|why| format!("XDG_RUNTIME_DIR is not set and {} {why}", dir.display()))
}

/// The directory holding niri's socket, which is niri's runtime directory, if it is a
/// private directory of `euid`'s. Anything else is refused, never replaced by another
/// directory: the lease and the stop flag must be the ones niri's own session uses.
fn socket_runtime_dir(socket: &Path, euid: u32) -> Result<PathBuf, String> {
    let dir = socket.parent().unwrap_or_else(|| Path::new(""));
    private(dir, euid)
        .map(|()| dir.to_path_buf())
        .map_err(|why| {
            format!(
                "XDG_RUNTIME_DIR is not set and {}, the directory of NIRI_SOCKET, {why}; set \
             XDG_RUNTIME_DIR to niri's runtime directory",
                dir.display()
            )
        })
}

/// Whether `dir` is a directory, not a symlink, owned by `euid` with mode `0700`; if not,
/// why not.
fn private(dir: &Path, euid: u32) -> Result<(), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|error| format!("can't be read: {error}"))?;
    if !meta.is_dir() {
        return Err("is not a directory".to_owned());
    }
    if meta.uid() != euid {
        return Err(format!("belongs to user {}, not {euid}", meta.uid()));
    }
    let mode = meta.mode() & 0o7777;
    if mode != 0o700 {
        return Err(format!("has mode {mode:04o}, not 0700"));
    }
    Ok(())
}

/// A warning when both variables are given and niri's socket isn't in the runtime
/// directory. niri puts its socket in its own runtime directory, but the same directory
/// can be spelled another way, through a symlink for one, so this doesn't refuse.
fn misplaced(runtime_dir: Option<&Path>, niri_socket: Option<&Path>) -> Option<String> {
    let (runtime_dir, socket) = (runtime_dir?, niri_socket?);
    (socket.parent() != Some(runtime_dir)).then(|| {
        format!(
            "NIRI_SOCKET {} is not in XDG_RUNTIME_DIR {}; unless that is another name for \
             niri's runtime directory, this server's lease and stop flag aren't the ones \
             servers started from niri's session use",
            socket.display(),
            runtime_dir.display()
        )
    })
}

/// The display and PID in a socket name niri gives, `niri.<display>.<pid>.sock`.
pub(crate) fn parse_socket_name(name: &str) -> Option<(&str, u32)> {
    let (display, pid) = name
        .strip_prefix("niri.")?
        .strip_suffix(".sock")?
        .rsplit_once('.')?;
    if display.is_empty() || display.contains('/') || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((display, pid.parse().ok()?))
}

/// The socket of the one running niri in the runtime directory `dir`, on `display` if one
/// is given.
async fn niri_socket(
    dir: Option<&Path>,
    display: Option<&OsStr>,
    roots: &Roots<'_>,
) -> Result<PathBuf, String> {
    let dir =
        dir.ok_or("NIRI_SOCKET is not set and there is no runtime directory to look for it in")?;
    let mut found = niri_sockets(dir, roots).await?;
    found.retain(|socket| display.is_none_or(|display| serves(socket, display, dir)));
    choose(dir, &found, display)
}

/// The sockets in `dir` named the way niri names its own, owned by `roots.euid`, whose
/// PID is a running process called `niri` that listens on them, in name order. A socket a
/// crashed niri left stays behind, and its PID may since belong to another niri; it accepts
/// no connection, so it doesn't count. Nothing is deleted.
async fn niri_sockets(dir: &Path, roots: &Roots<'_>) -> Result<Vec<PathBuf>, String> {
    let entries = std::fs::read_dir(dir).map_err(|error| {
        format!(
            "NIRI_SOCKET is not set and {} can't be listed: {error}",
            dir.display()
        )
    })?;
    let named: Vec<(PathBuf, u32)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let (_, pid) = parse_socket_name(entry.file_name().to_str()?)?;
            // Not followed: a symlink is not niri's socket.
            let meta = entry.metadata().ok()?;
            let ours = meta.file_type().is_socket() && meta.uid() == roots.euid;
            (ours && runs_niri(roots.proc, pid)).then(|| (entry.path(), pid))
        })
        .collect();
    let mut found = Vec::new();
    for (socket, pid) in named {
        if niri::listener_pid(&socket, CONNECT_DEADLINE).await == Ok(pid) {
            found.push(socket);
        }
    }
    found.sort();
    Ok(found)
}

fn runs_niri(proc_root: &Path, pid: u32) -> bool {
    procs::stat(proc_root, pid).is_some_and(|stat| !stat.exited())
        && procs::comm(proc_root, pid).as_deref() == Some("niri")
}

/// Whether niri's socket `socket` in `dir` names the given `WAYLAND_DISPLAY`: a name in
/// `dir`, or an absolute path.
fn serves(socket: &Path, display: &OsStr, dir: &Path) -> bool {
    let Some((named, _)) = socket
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(parse_socket_name)
    else {
        return false;
    };
    let display = Path::new(display);
    if display.is_absolute() {
        return display == dir.join(named);
    }
    display == Path::new(named)
}

/// The one niri socket found in `dir`, for the given `display` if there is one. None, or
/// several, is an error that says what to do; this never guesses between two sessions.
fn choose(dir: &Path, found: &[PathBuf], display: Option<&OsStr>) -> Result<PathBuf, String> {
    let on = display.map_or_else(String::new, |display| {
        format!(" on WAYLAND_DISPLAY {}", display.to_string_lossy())
    });
    match found {
        [] => Err(format!(
            "NIRI_SOCKET is not set and {} has no socket of a running niri{on}",
            dir.display()
        )),
        [one] => Ok(one.clone()),
        several => {
            let names: Vec<_> = several
                .iter()
                .filter_map(|socket| socket.file_name())
                .map(OsStr::to_string_lossy)
                .collect();
            Err(format!(
                "NIRI_SOCKET is not set and {} has the sockets of {} running niri instances{on}: \
                 {}; set NIRI_SOCKET to the one to use",
                dir.display(),
                several.len(),
                names.join(", ")
            ))
        }
    }
}

/// The display niri's socket names, if `runtime_dir` has a socket of that name.
fn wayland_display(
    niri_socket: Result<&Path, &String>,
    runtime_dir: Option<&Path>,
) -> Result<OsString, String> {
    let socket = niri_socket
        .map_err(|_| "WAYLAND_DISPLAY is not set and there is no niri socket to take it from")?;
    let display = socket
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(parse_socket_name)
        .map(|(display, _)| display)
        .ok_or_else(|| {
            format!(
                "WAYLAND_DISPLAY is not set and the niri socket {} names no display",
                socket.display()
            )
        })?;
    let dir = runtime_dir
        .ok_or("WAYLAND_DISPLAY is not set and there is no runtime directory to look for it in")?;
    let path = dir.join(display);
    if !std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.file_type().is_socket()) {
        return Err(format!(
            "WAYLAND_DISPLAY is not set and {} is not a socket",
            path.display()
        ));
    }
    Ok(display.into())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::os::unix::net::{UnixListener, UnixStream};

    use super::*;

    /// A fake host: `run/<euid>` with mode `0700` and an empty `proc`. The directories
    /// belong to whoever runs the test, so the roots use that user's ID.
    struct Host {
        root: PathBuf,
        euid: u32,
        sockets: Vec<UnixListener>,
    }

    impl Host {
        fn new(name: &str) -> Self {
            let root = crate::test_support::fresh_dir(name);
            let euid = rustix::process::geteuid().as_raw();
            let run = root.join("run").join(euid.to_string());
            std::fs::create_dir_all(&run).unwrap();
            std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::create_dir(root.join("proc")).unwrap();
            Self {
                root,
                euid,
                sockets: Vec::new(),
            }
        }

        fn run_user(&self) -> PathBuf {
            self.root.join("run")
        }

        fn proc(&self) -> PathBuf {
            self.root.join("proc")
        }

        fn runtime(&self) -> PathBuf {
            self.run_user().join(self.euid.to_string())
        }

        fn roots(&self) -> (PathBuf, PathBuf) {
            (self.run_user(), self.proc())
        }

        /// A process `pid` called `comm`, running or a zombie.
        fn process(&self, pid: u32, comm: &str, state: char) {
            let dir = self.proc().join(pid.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
            let stat = format!(
                "{pid} ({comm}) {state} 1 {pid} {pid} 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 98765 0 0"
            );
            std::fs::write(dir.join("stat"), stat).unwrap();
        }

        /// A listening socket called `name` in the runtime directory.
        fn socket(&mut self, name: &str) {
            let socket = UnixListener::bind(self.runtime().join(name)).unwrap();
            self.sockets.push(socket);
        }

        /// A socket file called `name` that nothing listens on, as a crashed niri leaves.
        async fn stale(&self, name: &str) -> PathBuf {
            let path = self.runtime().join(name);
            drop(UnixListener::bind(&path).unwrap());
            // A child another test forked meanwhile holds the listener until it execs.
            let mut polls = 0;
            while UnixStream::connect(&path).is_ok() {
                assert!(polls < 500, "{} still accepts connections", path.display());
                polls += 1;
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            path
        }

        /// A running niri on `display`, as niri names its socket. The test's process
        /// listens on the socket, so the niri has the test's PID. Returns the name.
        fn niri(&mut self, display: &str) -> String {
            let me = std::process::id();
            self.process(me, "niri", 'S');
            let name = format!("niri.{display}.{me}.sock");
            self.socket(&name);
            name
        }

        async fn session(&self, given: Given) -> Session {
            let (run_user, proc) = self.roots();
            let roots = Roots {
                run_user: &run_user,
                proc: &proc,
                euid: self.euid,
            };
            session(given, &roots).await
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    /// Discovery with one socket, named for the test's PID, and `process` as the process
    /// with that PID, if any.
    async fn alone_with(process: Option<(&str, char)>) -> Session {
        let me = std::process::id();
        let mut host = Host::new("discover-dead");
        if let Some((comm, state)) = process {
            host.process(me, comm, state);
        }
        host.socket(&format!("niri.wayland-1.{me}.sock"));
        host.session(Given::default()).await
    }

    fn missing(source: &Source) -> &str {
        match source {
            Source::Missing(detail) => detail,
            Source::Environment | Source::Discovered => panic!("{source:?} isn't missing"),
        }
    }

    #[test]
    fn reads_the_display_and_pid_from_niris_socket_names() {
        assert_eq!(
            parse_socket_name("niri.wayland-1.1605.sock"),
            Some(("wayland-1", 1605))
        );
        assert_eq!(
            parse_socket_name("niri.wayland.dotted-2.7.sock"),
            Some(("wayland.dotted-2", 7))
        );
        for other in [
            "niri.test.sock",
            "niri..42.sock",
            "niri.wayland-1.+42.sock",
            "niri.wayland-1.42",
            "noctalia-wayland-1.sock",
            "niri.wayland-1.99999999999.sock",
        ] {
            assert_eq!(parse_socket_name(other), None, "{other}");
        }
    }

    #[test]
    fn uses_one_socket_but_never_guesses_between_several() {
        let dir = Path::new("/run/user/1000");
        let none = choose(dir, &[], None).unwrap_err();
        assert!(none.contains("/run/user/1000 has no socket"), "{none}");
        let one = dir.join("niri.wayland-1.5.sock");
        assert_eq!(
            choose(dir, std::slice::from_ref(&one), None),
            Ok(one.clone())
        );
        let several = choose(dir, &[one, dir.join("niri.wayland-2.6.sock")], None).unwrap_err();
        assert_eq!(
            several,
            "NIRI_SOCKET is not set and /run/user/1000 has the sockets of 2 running niri \
             instances: niri.wayland-1.5.sock, niri.wayland-2.6.sock; set NIRI_SOCKET to the \
             one to use"
        );
    }

    #[tokio::test]
    async fn finds_the_session_of_the_one_running_niri() {
        let mut host = Host::new("discover-one");
        let name = host.niri("wayland-1");
        host.socket("wayland-1");
        let session = host.session(Given::default()).await;
        assert_eq!(session.runtime_dir, Some(host.runtime()));
        assert_eq!(session.niri_socket, Ok(host.runtime().join(name)));
        assert_eq!(session.wayland_display, Some("wayland-1".into()));
        let found = Source::Discovered;
        assert_eq!(
            session.sources,
            Sources {
                runtime_dir: found.clone(),
                niri_socket: found.clone(),
                wayland_display: found,
                warning: None,
            }
        );
    }

    #[tokio::test]
    async fn skips_sockets_of_dead_or_other_processes_and_files_that_arent_sockets() {
        for process in [None, Some(("niri", 'Z')), Some(("bash", 'S'))] {
            let session = alone_with(process).await;
            let detail = missing(&session.sources.niri_socket);
            assert!(
                detail.contains("has no socket of a running niri"),
                "{process:?}: {detail}"
            );
            assert_eq!(session.niri_socket, Err(detail.to_owned()));
            let display = missing(&session.sources.wayland_display);
            assert!(display.contains("no niri socket"), "{display}");
        }
        let me = std::process::id();
        let host = Host::new("discover-file");
        host.process(me, "niri", 'S');
        std::fs::write(host.runtime().join(format!("niri.wayland-1.{me}.sock")), "").unwrap();
        let file = host.session(Given::default()).await;
        assert!(missing(&file.sources.niri_socket).contains("has no socket of a running niri"));
    }

    /// A crashed niri leaves its socket behind, and its PID may later be another niri's.
    #[tokio::test]
    async fn only_a_socket_niri_listens_on_counts_and_none_is_deleted() {
        let mut host = Host::new("discover-stale");
        let me = std::process::id();
        host.process(me, "niri", 'S');
        let stale = host.stale(&format!("niri.wayland-0.{me}.sock")).await;
        let only_stale = host.session(Given::default()).await;
        let detail = missing(&only_stale.sources.niri_socket);
        assert!(
            detail.contains("has no socket of a running niri"),
            "{detail}"
        );
        // Named for a running niri, but another process listens on it.
        host.process(5, "niri", 'S');
        host.socket("niri.wayland-2.5.sock");
        let live = host.niri("wayland-1");
        let session = host.session(Given::default()).await;
        assert_eq!(session.niri_socket, Ok(host.runtime().join(live)));
        assert!(stale.exists());
    }

    #[tokio::test]
    async fn several_running_niris_leave_the_socket_unknown() {
        let mut host = Host::new("discover-two");
        let names = ["wayland-1", "wayland-2"].map(|display| {
            host.socket(display);
            host.niri(display)
        });
        let session = host.session(Given::default()).await;
        let detail = session.niri_socket.unwrap_err();
        assert!(detail.contains(&names.join(", ")), "{detail}");
        assert_eq!(session.wayland_display, None);
    }

    #[tokio::test]
    async fn a_given_display_keeps_only_the_niri_that_serves_it() {
        let mut host = Host::new("discover-on-display");
        host.niri("wayland-1");
        let second = host.runtime().join(host.niri("wayland-2"));
        let on = |display: &Path| {
            host.session(Given {
                wayland_display: Some(display.into()),
                ..Given::default()
            })
        };
        assert_eq!(
            on(Path::new("wayland-2")).await.niri_socket,
            Ok(second.clone())
        );
        assert_eq!(
            on(&host.runtime().join("wayland-2")).await.niri_socket,
            Ok(second)
        );
        for other in [Path::new("wayland-3"), Path::new("/elsewhere/wayland-2")] {
            let session = on(other).await;
            let detail = session.niri_socket.unwrap_err();
            assert!(
                detail.ends_with(&format!(
                    "has no socket of a running niri on WAYLAND_DISPLAY {}",
                    other.display()
                )),
                "{detail}"
            );
        }
    }

    #[tokio::test]
    async fn a_socket_of_another_user_is_not_niris() {
        let mut host = Host::new("discover-owner");
        host.niri("wayland-1");
        let (run_user, proc) = host.roots();
        let roots = Roots {
            run_user: &run_user,
            proc: &proc,
            euid: host.euid,
        };
        let other = Roots {
            euid: host.euid + 1,
            ..roots
        };
        assert_eq!(
            niri_sockets(&host.runtime(), &roots).await.unwrap().len(),
            1
        );
        assert_eq!(
            niri_sockets(&host.runtime(), &other).await.unwrap(),
            Vec::<PathBuf>::new()
        );
    }

    #[tokio::test]
    async fn set_variables_win_over_discovery() {
        let mut host = Host::new("discover-given");
        host.niri("wayland-1");
        host.socket("wayland-1");
        let session = host
            .session(Given {
                runtime_dir: Some("/elsewhere".into()),
                niri_socket: Some("/elsewhere/niri.wayland-9.9.sock".into()),
                wayland_display: Some("wayland-9".into()),
            })
            .await;
        assert_eq!(session.runtime_dir, Some("/elsewhere".into()));
        assert_eq!(
            session.niri_socket,
            Ok("/elsewhere/niri.wayland-9.9.sock".into())
        );
        assert_eq!(session.wayland_display, Some("wayland-9".into()));
        let given = Source::Environment;
        assert_eq!(
            session.sources,
            Sources {
                runtime_dir: given.clone(),
                niri_socket: given.clone(),
                wayland_display: given,
                warning: None,
            }
        );
    }

    #[tokio::test]
    async fn the_display_comes_from_a_given_niri_socket_too() {
        let mut host = Host::new("discover-display");
        host.socket("wayland-3");
        let given = |socket: &str| Given {
            niri_socket: Some(host.runtime().join(socket)),
            ..Given::default()
        };
        let session = host.session(given("niri.wayland-3.42.sock")).await;
        assert_eq!(session.wayland_display, Some("wayland-3".into()));
        assert_eq!(session.sources.wayland_display, Source::Discovered);
        let nameless = host.session(given("niri.test.sock")).await;
        let unnamed = missing(&nameless.sources.wayland_display);
        assert!(unnamed.contains("names no display"), "{unnamed}");
        let gone = host.session(given("niri.wayland-4.42.sock")).await;
        let absent = missing(&gone.sources.wayland_display);
        assert!(absent.ends_with("wayland-4 is not a socket"), "{absent}");
    }

    #[tokio::test]
    async fn a_given_niri_socket_names_the_runtime_directory() {
        let host = Host::new("discover-nested");
        let nested = host.root.join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _display = UnixListener::bind(nested.join("wayland-2")).unwrap();
        let socket = nested.join("niri.wayland-2.42.sock");
        let given = || Given {
            niri_socket: Some(socket.clone()),
            ..Given::default()
        };
        let session = host.session(given()).await;
        assert_eq!(session.runtime_dir, Some(nested.clone()));
        assert_eq!(session.sources.runtime_dir, Source::Discovered);
        assert_eq!(session.wayland_display, Some("wayland-2".into()));

        // Never the default directory instead: its lease and stop flag are another one's.
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
        let open = host.session(given()).await;
        assert_eq!(open.runtime_dir, None);
        let detail = missing(&open.sources.runtime_dir);
        assert!(
            detail.contains("the directory of NIRI_SOCKET, has mode 0755, not 0700"),
            "{detail}"
        );
        assert!(detail.ends_with("set XDG_RUNTIME_DIR to niri's runtime directory"));
    }

    #[tokio::test]
    async fn a_niri_socket_outside_the_given_runtime_directory_is_a_warning() {
        let host = Host::new("discover-misplaced");
        let session = host
            .session(Given {
                runtime_dir: Some("/run/user/1000".into()),
                niri_socket: Some("/tmp/nested/niri.wayland-2.42.sock".into()),
                wayland_display: None,
            })
            .await;
        assert_eq!(session.runtime_dir, Some("/run/user/1000".into()));
        let warning = session.sources.warning.unwrap();
        assert!(
            warning.starts_with(
                "NIRI_SOCKET /tmp/nested/niri.wayland-2.42.sock is not in XDG_RUNTIME_DIR \
                 /run/user/1000"
            ),
            "{warning}"
        );
        let inside = host
            .session(Given {
                runtime_dir: Some("/run/user/1000/".into()),
                niri_socket: Some("/run/user/1000/niri.wayland-1.5.sock".into()),
                wayland_display: None,
            })
            .await;
        assert_eq!(inside.sources.warning, None);
    }

    #[test]
    fn the_runtime_directory_must_be_the_users_own_with_mode_0700() {
        let host = Host::new("discover-runtime");
        let run_user = host.run_user();
        assert_eq!(runtime_dir(&run_user, host.euid), Ok(host.runtime()));
        let other = runtime_dir(&run_user, host.euid + 1).unwrap_err();
        assert!(other.contains("can't be read"), "{other}");
        std::fs::set_permissions(host.runtime(), std::fs::Permissions::from_mode(0o755)).unwrap();
        let open = runtime_dir(&run_user, host.euid).unwrap_err();
        assert!(open.ends_with("has mode 0755, not 0700"), "{open}");
        let file = run_user.join("4242");
        std::fs::write(&file, "").unwrap();
        let not_dir = runtime_dir(&run_user, 4242).unwrap_err();
        assert!(not_dir.ends_with("is not a directory"), "{not_dir}");
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        let foreign = runtime_dir(&run_user, 4242).unwrap_err();
        assert!(
            foreign.contains(&format!("belongs to user {}", host.euid)),
            "{foreign}"
        );
    }
}
