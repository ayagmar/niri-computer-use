//! A directory per test holding the server's whole environment: fake programs on `PATH`,
//! a runtime directory for the niri and Noctalia sockets, and a state directory for the
//! audit log. The server starts with only these variables set.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// Programs the fake programs may use. They run with their own `PATH`, so the server's
/// `PATH` holds nothing but the fakes.
const UTILITIES: [&str; 4] = ["cat", "head", "sleep", "tr"];

pub(crate) const DISPLAY: &str = "wayland-test";

/// Held while writing a fake program and while starting a server. A child forked during a
/// write would inherit the open file until it execs, and running the program meanwhile
/// fails with "Text file busy".
pub(crate) static SPAWNING: Mutex<()> = Mutex::new(());
pub(crate) const SESSION: &str = "7";

#[derive(Debug)]
pub(crate) struct Fixture {
    pub(crate) dir: PathBuf,
    env: BTreeMap<&'static str, OsString>,
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
        for sub in ["bin", "utils", "run", "state"] {
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
        ]);
        let fixture = Self { dir, env };
        let longest = fixture.noctalia_socket().as_os_str().len();
        assert!(
            longest < 108,
            "fixture name {name:?} makes socket paths too long"
        );
        fixture
    }

    pub(crate) fn env(&self) -> &BTreeMap<&'static str, OsString> {
        &self.env
    }

    pub(crate) fn set(&mut self, name: &'static str, value: impl Into<OsString>) {
        self.env.insert(name, value.into());
    }

    pub(crate) fn unset(&mut self, name: &'static str) {
        self.env.remove(name);
    }

    pub(crate) fn niri_socket(&self) -> PathBuf {
        self.dir.join("run/niri.test.sock")
    }

    pub(crate) fn noctalia_socket(&self) -> PathBuf {
        self.dir.join(format!("run/noctalia-{DISPLAY}.sock"))
    }

    pub(crate) fn audit_log(&self) -> PathBuf {
        self.dir.join("state/niri-computer-use/audit.jsonl")
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }

    /// A fake program on the server's `PATH`: a shell script that sees the test directory
    /// as `$DIR`. Programs record their arguments in `<name>.args` when they need to.
    pub(crate) fn program(&self, name: &str, body: &str) {
        let script = format!(
            "#!/bin/sh\nPATH='{}'\nDIR='{}'\n{body}\n",
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
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.dir).ok();
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
