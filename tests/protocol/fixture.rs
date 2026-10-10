//! A directory per test holding the server's whole environment: fake programs on `PATH`,
//! a runtime directory for the niri and Noctalia sockets, and a state directory for the
//! audit log. The server starts with only these variables set.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::client::WAIT;

/// Programs the fake programs may use. They run with their own `PATH`, so the server's
/// `PATH` holds nothing but the fakes.
const UTILITIES: [&str; 4] = ["cat", "head", "sleep", "tr"];

pub(crate) const DISPLAY: &str = "wayland-test";

/// The fake programs' bounded wait for a file the test creates: 500 polls of 20 ms.
const AWAIT_FILE: &str = r#"await_file() {
    polls=0
    while [ ! -e "$DIR/$1" ]; do
        if [ ! -d "$DIR" ] || [ "$polls" -ge 500 ]; then exit 1; fi
        polls=$((polls + 1))
        sleep 0.02
    done
}"#;

/// Whether the suite runs its servers in shared mode: `NCU_PROTOCOL_MODE=shared`.
pub(crate) fn shared_mode() -> bool {
    std::env::var_os("NCU_PROTOCOL_MODE").is_some_and(|mode| mode == "shared")
}

/// Held while writing a fake program and while starting a server. A child forked during a
/// write would inherit the open file until it execs, and running the program meanwhile
/// fails with "Text file busy".
pub(crate) static SPAWNING: Mutex<()> = Mutex::new(());
pub(crate) const SESSION: &str = "7";

#[derive(Debug)]
pub(crate) struct Fixture {
    pub(crate) dir: PathBuf,
    env: BTreeMap<&'static str, OsString>,
    /// The servers started here. Each must have exited before the directory is removed,
    /// or its audit log could create the directory again.
    servers: Mutex<Vec<u32>>,
    /// The Wayland display, served by the test's process like the fake niri, so the server
    /// takes it as niri's. Nothing speaks Wayland on it.
    _display: UnixListener,
}

impl Fixture {
    /// Creates the directory, which must not exist yet, and links the utilities.
    pub(crate) fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // In `/tmp` whatever `TMPDIR` says, because the sockets inside must fit the 108-byte
        // limit on Unix socket paths.
        let dir = Path::new("/tmp").join(format!("ncu-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        for sub in ["bin", "utils", "run", "state", "config"] {
            std::fs::create_dir(dir.join(sub)).unwrap();
        }
        for utility in UTILITIES {
            std::os::unix::fs::symlink(locate(utility), dir.join("utils").join(utility)).unwrap();
        }
        let env = BTreeMap::from([
            ("PATH", dir.join("bin").into_os_string()),
            (
                "NIRI_SOCKET",
                dir.join("run/niri.test.sock").into_os_string(),
            ),
            ("XDG_RUNTIME_DIR", dir.join("run").into_os_string()),
            ("WAYLAND_DISPLAY", DISPLAY.into()),
            ("XDG_SESSION_ID", SESSION.into()),
            ("XDG_STATE_HOME", dir.join("state").into_os_string()),
            ("XDG_CONFIG_HOME", dir.join("config").into_os_string()),
        ]);
        let display = UnixListener::bind(dir.join("run").join(DISPLAY)).unwrap();
        let mut fixture = Self {
            dir,
            env,
            servers: Mutex::new(Vec::new()),
            _display: display,
        };
        if shared_mode() {
            fixture.set("NIRI_COMPUTER_USE_SHARED", "1");
        }
        let longest = fixture.runtime_dir().join("engine.sock").as_os_str().len();
        assert!(
            longest < 108,
            "fixture name {name:?} makes socket paths too long"
        );
        fixture
    }

    pub(crate) fn env(&self) -> &BTreeMap<&'static str, OsString> {
        &self.env
    }

    /// Notes a server started on this fixture, which must exit before the fixture drops.
    pub(crate) fn started(&self, server: u32) {
        self.servers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(server);
    }

    pub(crate) fn set(&mut self, name: &'static str, value: impl Into<OsString>) {
        self.env.insert(name, value.into());
    }

    /// Removes a variable from the server's environment, except `XDG_RUNTIME_DIR`: without
    /// it the server would look for the host's session in `/run/user/<uid>`.
    pub(crate) fn unset(&mut self, name: &'static str) {
        assert_ne!(
            name, "XDG_RUNTIME_DIR",
            "the server must never discover the host session"
        );
        self.env.remove(name);
    }

    pub(crate) fn niri_socket(&self) -> PathBuf {
        self.dir.join("run/niri.test.sock")
    }

    pub(crate) fn noctalia_socket(&self) -> PathBuf {
        self.dir.join(format!("run/noctalia-{DISPLAY}.sock"))
    }

    /// The server's runtime directory for the fixture's niri, which holds the engine's
    /// socket, lock and log.
    pub(crate) fn runtime_dir(&self) -> PathBuf {
        self.dir.join("run/niri-computer-use/niri.test")
    }

    /// What the shared engine and its guardian wrote to stderr, or nothing before an engine
    /// ran.
    pub(crate) fn engine_log(&self) -> String {
        std::fs::read_to_string(self.runtime_dir().join("engine.log")).unwrap_or_default()
    }

    pub(crate) fn audit_log(&self) -> PathBuf {
        self.dir.join("state/niri-computer-use/audit.jsonl")
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// A fake program on the server's `PATH`: a shell script that sees the test directory
    /// as `$DIR`. Programs record their arguments in `<name>.args` when they need to.
    /// `await_file <name>` waits for `$DIR/<name>`, and exits the program after ten seconds
    /// or once the test directory is gone: a `SIGKILL`ed server leaves its children running.
    pub(crate) fn program(&self, name: &str, body: &str) {
        let script = format!(
            "#!/bin/sh\nPATH='{}'\nDIR='{}'\n{AWAIT_FILE}\n{body}\n",
            self.dir.join("utils").display(),
            self.dir.display()
        );
        let file = self.dir.join("bin").join(name);
        let _spawning = SPAWNING.lock().unwrap_or_else(PoisonError::into_inner);
        std::fs::write(&file, script).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The audit log's lines, or none if it doesn't exist.
    pub(crate) fn audit_lines(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.audit_log())
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The arguments a fake program recorded, one per line.
    pub(crate) fn args(&self, program: &str) -> Vec<String> {
        std::fs::read_to_string(self.path(&format!("{program}.args")))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Fixture {
    /// Removes the directory once nothing started for it runs any more. A shared engine
    /// outlives its bridges for its idle grace, and it or its guardian could otherwise
    /// write in the directory after the removal and create it again.
    fn drop(&mut self) {
        let servers = self
            .servers
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner);
        let running: Vec<u32> = servers
            .iter()
            .copied()
            .filter(|pid| !exited(i32::try_from(*pid).unwrap()))
            .collect();
        end_processes(&self.env["XDG_RUNTIME_DIR"]);
        std::fs::remove_dir_all(&self.dir).ok();
        if !std::thread::panicking() {
            assert!(
                running.is_empty(),
                "servers {running:?} outlived their fixture; drop them before it"
            );
        }
    }
}

/// The utility's path on the test's own `PATH`.
fn locate(utility: &str) -> PathBuf {
    let path = std::env::var_os("PATH").unwrap();
    std::env::split_paths(&path)
        .map(|dir| dir.join(utility))
        .find(|file| file.is_file())
        .unwrap()
}

/// Polls `done` every 20 ms until it holds, for at most `limit`.
pub(crate) async fn eventually(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    done()
}

/// The process ID a fake program wrote to `file`, once it has.
pub(crate) async fn pid_in(file: &Path) -> i32 {
    assert!(
        eventually(Duration::from_secs(10), || std::fs::read_to_string(file)
            .is_ok_and(|text| text.ends_with('\n')))
        .await,
        "{} never appeared",
        file.display()
    );
    std::fs::read_to_string(file)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

/// Kills every process whose environment has `XDG_RUNTIME_DIR` set to `runtime`, such as
/// a shared engine in its idle grace and its guardian, and waits until none is left, for
/// at most `WAIT`. Every fixture sets the variable, and the engine keeps it even when a
/// test leaves `NIRI_SOCKET` to discovery.
fn end_processes(runtime: &OsStr) {
    let end = Instant::now() + WAIT;
    loop {
        let left = processes_in(runtime);
        if left.is_empty() || Instant::now() > end {
            return;
        }
        for pid in left {
            if let Some(pid) = rustix::process::Pid::from_raw(pid) {
                // It may have exited since the listing.
                rustix::process::kill_process(pid, rustix::process::Signal::KILL).ok();
            }
        }
        std::thread::yield_now();
    }
}

/// The running processes of ours whose environment has the entry
/// `XDG_RUNTIME_DIR=<runtime>`, matched whole: a process that only mentions the fixture's
/// directory elsewhere, such as a shell whose `PWD` is inside it, isn't one.
fn processes_in(runtime: &OsStr) -> Vec<i32> {
    let wanted = [b"XDG_RUNTIME_DIR=".as_slice(), runtime.as_bytes()].concat();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .filter(|pid| !exited(*pid))
        .filter(|pid| {
            // Another user's processes can't be read, and a process can exit meanwhile.
            std::fs::read(format!("/proc/{pid}/environ"))
                .is_ok_and(|environ| environ.split(|byte| *byte == 0).any(|part| part == wanted))
        })
        .collect()
}

/// Sends `SIGKILL` to `pid`.
pub(crate) fn kill(pid: u32) {
    signal(pid, rustix::process::Signal::KILL);
}

/// Sends `signal` to `pid`.
pub(crate) fn signal(pid: u32, signal: rustix::process::Signal) {
    let pid = rustix::process::Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
    rustix::process::kill_process(pid, signal).unwrap();
}

/// Whether `pid` has exited: gone, or a zombie waiting to be reaped.
pub(crate) fn exited(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| rest.starts_with('Z'))
    })
}

impl Fixture {
    /// A grim that records its arguments and writes `image`.
    pub(crate) fn grim(&self, image: &[u8]) {
        std::fs::write(self.path("grim.out"), image).unwrap();
        self.program(
            "grim",
            r#"printf '%s\n' "$@" > "$DIR/grim.args"; cat "$DIR/grim.out""#,
        );
    }
}

/// A JPEG's first segments, as far as a baseline start-of-frame of `width` x `height`,
/// followed by `tail`.
pub(crate) fn jpeg(width: u16, height: u16, tail: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08];
    bytes.extend(height.to_be_bytes());
    bytes.extend(width.to_be_bytes());
    bytes.extend([3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1]);
    bytes.extend(tail);
    bytes
}

/// A PNG's signature and `IHDR` chunk for `width` x `height`.
pub(crate) fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
    bytes.extend(width.to_be_bytes());
    bytes.extend(height.to_be_bytes());
    bytes.extend([8, 2, 0, 0, 0, 0, 0, 0, 0]);
    bytes
}

/// Runs the fake program `name` itself, as the server's runner would. It dies with the
/// returned child.
#[expect(
    clippy::disallowed_methods,
    reason = "the test runs a fake program itself, as the server's runner would"
)]
pub(crate) fn fake(fixture: &Fixture, name: &str) -> tokio::process::Child {
    let _spawning = SPAWNING.lock().unwrap_or_else(PoisonError::into_inner);
    tokio::process::Command::new(fixture.path(&format!("bin/{name}")))
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

/// The exit code of a fake program waiting for `go` once the test creates it, or once the
/// test directory is removed instead.
async fn await_file_exit(remove_dir: bool) -> Option<i32> {
    let fixture = Fixture::new("await-file");
    fixture.program("waiter", "await_file go");
    let mut child = fake(&fixture, "waiter");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(child.try_wait().unwrap().is_none(), "returned before go");
    if remove_dir {
        drop(fixture);
    } else {
        std::fs::write(fixture.path("go"), "").unwrap();
    }
    let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .expect("still waiting")
        .unwrap();
    status.code()
}

#[tokio::test]
async fn a_fake_program_waits_for_its_file_but_not_past_its_test_directory() {
    assert_eq!(await_file_exit(false).await, Some(0));
    assert_eq!(await_file_exit(true).await, Some(1));
}
