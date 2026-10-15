//! The hello: the first line each way on a connection to the engine, before MCP. The
//! bridge says which build it is, which niri and display it found, and what its client's
//! environment gives the session; the engine takes it as a new session or refuses it.
//! Pure, apart from reading the identity of this binary and of the process at the other end.

use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::policy;
use crate::session::Given;

const VERSION: u32 = 3;
/// The release this build is, as a bridge names it in its hello.
const RELEASE: &str = env!("CARGO_PKG_VERSION");
/// A hello line, newline included. The policy file's text, at most `MAX_POLICY`, can grow
/// up to six times in JSON.
pub(crate) const MAX_LINE: usize = 512 * 1024;
/// The policy file a bridge sends; a longer one is sent as unreadable.
pub(crate) const MAX_POLICY: usize = 64 * 1024;
/// How long each side waits for the other's hello line.
pub(crate) const DEADLINE: Duration = Duration::from_secs(2);

/// This build of the binary: the file `/proc/self/exe` names. Two processes run the same
/// build when they run the same file, unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Exe {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
}

impl Exe {
    pub(crate) fn current() -> Result<Self, String> {
        Self::of(Path::new("/proc/self/exe"))
    }

    /// The build in the file at `path`.
    fn of(path: &Path) -> Result<Self, String> {
        let meta =
            std::fs::metadata(path).map_err(|error| format!("stat {}: {error}", path.display()))?;
        Ok(Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
        })
    }
}

/// The file this process was started from, and whether that path still holds this build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Build {
    /// As `/proc/self/exe` names it; null if it can't be read.
    pub(crate) exe: Option<PathBuf>,
    /// The path now holds another file or none, as after a reinstall: this process still
    /// runs the build it started with.
    pub(crate) replaced: bool,
}

impl Build {
    pub(crate) fn current() -> Self {
        Self::at(Path::new("/proc/self/exe"))
    }

    /// The build process `pid` runs, with no file when its link can't be read, as for a
    /// process that has ended or a PID of 0.
    pub(crate) fn of(pid: u32) -> Self {
        Self::at(&PathBuf::from(format!("/proc/{pid}/exe")))
    }

    /// From a process's `exe` link.
    fn at(link: &Path) -> Self {
        let exe = std::fs::read_link(link).ok().map(|path| {
            // The kernel names a file removed or replaced since with this suffix.
            let kept = path.as_os_str().as_bytes().strip_suffix(b" (deleted)");
            kept.map(|kept| PathBuf::from(std::ffi::OsStr::from_bytes(kept)))
                .unwrap_or(path)
        });
        let running = Exe::of(link).ok();
        let at_path = exe.as_deref().and_then(|path| Exe::of(path).ok());
        Self {
            replaced: running.is_none() || at_path != running,
            exe,
        }
    }

    /// Whether another build has replaced the file this process was started from, as an
    /// install does: restarting the process runs that build.
    const fn stale(&self) -> bool {
        self.exe.is_some() && self.replaced
    }

    /// Where the process runs from, as a refusal names it.
    fn whence(&self) -> String {
        match &self.exe {
            Some(file) if self.replaced => {
                format!(
                    " from {}, which another build has since replaced",
                    file.display()
                )
            }
            Some(file) => format!(" from {}", file.display()),
            None => String::new(),
        }
    }
}

/// What the user does when an engine running `engine` refuses a client whose server runs
/// `client`, another build: restart whichever an install replaced, as restarting it runs
/// the installed build; when neither was, both builds are installed, and restarting
/// either runs the same build again.
pub(crate) fn restart(engine: &Build, client: &Build) -> String {
    match (engine.stale(), client.stale(), &engine.exe, &client.exe) {
        (true, true, ..) => "restart this MCP client and the ones the engine serves, so that each runs the build now installed".to_owned(),
        (true, false, ..) => "restart the MCP clients the engine serves, so that it exits and the next client starts an engine of the installed build".to_owned(),
        (false, true, ..) => "restart this MCP client, so that its server runs the build now installed".to_owned(),
        (false, false, Some(theirs), Some(ours)) => format!(
            "both builds are still installed: point this MCP client at {}, or the ones the engine serves at {}, so that all of them run one build",
            theirs.display(),
            ours.display()
        ),
        (false, false, ..) => "restart this MCP client if another build has since replaced its server's file; otherwise point it at the file the engine runs, so that both run one build".to_owned(),
    }
}

/// What a client's server adds to an `engine_version` refusal by an engine of 0.1.1 or
/// older, which says only that it runs another build, from what the engine process,
/// PID `pid` or 0 if unknown, and this one run.
pub(crate) fn older_engine(pid: u32) -> String {
    let engine = Build::of(pid);
    let client = Build::current();
    let named = if pid == 0 {
        String::new()
    } else {
        format!(", PID {pid},")
    };
    format!(
        "that engine{named} runs niri-computer-use 0.1.1 or older{}, and this client's server runs {RELEASE}{}: {}",
        engine.whence(),
        client.whence(),
        restart(&engine, &client)
    )
}

/// What a bridge sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Hello {
    #[serde(rename = "hello")]
    version: u32,
    /// The bridge's release, such as `0.1.1`. Bridges of 0.1.1 and older send none, and
    /// engines of 0.1.1 and older ignore it.
    #[serde(default)]
    release: Option<String>,
    exe: Exe,
    niri_socket: PathBuf,
    wayland_socket: Option<PathBuf>,
    /// Where the bridge looks for `loginctl` and `noctalia`.
    desktop_path: String,
    unrestricted: Option<String>,
    confirm_lease: Option<String>,
    tools: Option<String>,
    home: Option<PathBuf>,
    path: Option<String>,
    policy: policy::Source,
}

impl Hello {
    /// The hello of a bridge running `exe` for the niri on `niri_socket` and the display
    /// on `wayland_socket`, looking for the desktop's programs in `desktop_path`, for a
    /// client whose environment gave `given`. A policy file over `MAX_POLICY` is sent as
    /// unreadable. Fails on a value that isn't UTF-8.
    pub(crate) fn new(
        exe: Exe,
        niri_socket: &Path,
        wayland_socket: Option<&Path>,
        desktop_path: &str,
        given: Given,
    ) -> Result<Self, String> {
        let utf8 = |name: &str, value: Option<std::ffi::OsString>| {
            value
                .map(|value| {
                    value
                        .into_string()
                        .map_err(|_| format!("{name} isn't UTF-8"))
                })
                .transpose()
        };
        let policy = match given.policy {
            policy::Source::Text { path, text } if text.len() > MAX_POLICY => {
                policy::Source::Unreadable {
                    path,
                    error: format!("it is over {MAX_POLICY} bytes, the most a shared engine takes"),
                }
            }
            other @ (policy::Source::NoConfigDir
            | policy::Source::Missing
            | policy::Source::Unreadable { .. }
            | policy::Source::Text { .. }) => other,
        };
        Ok(Self {
            version: VERSION,
            release: Some(RELEASE.to_owned()),
            exe,
            niri_socket: niri_socket.to_path_buf(),
            wayland_socket: wayland_socket.map(Path::to_path_buf),
            desktop_path: desktop_path.to_owned(),
            unrestricted: utf8("NIRI_COMPUTER_USE_UNRESTRICTED", given.unrestricted)?,
            confirm_lease: utf8("NIRI_COMPUTER_USE_CONFIRM_LEASE", given.confirm_lease)?,
            tools: utf8("NIRI_COMPUTER_USE_TOOLS", given.tools)?,
            home: given.home,
            path: utf8("PATH", given.path)?,
            policy,
        })
    }

    /// What the client's environment gave its session.
    pub(crate) fn given(self) -> Given {
        Given {
            unrestricted: self.unrestricted.map(Into::into),
            confirm_lease: self.confirm_lease.map(Into::into),
            tools: self.tools.map(Into::into),
            home: self.home,
            path: self.path.map(Into::into),
            policy: self.policy,
        }
    }
}

/// What the engine answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reply {
    Engine {
        pid: u32,
    },
    Refused {
        error: Refusal,
        detail: String,
        /// The engine's release. Engines of 0.1.1 and older send none, and their bridges
        /// ignore it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        release: Option<String>,
    },
}

impl Reply {
    /// This engine's refusal.
    pub(crate) fn refused(error: Refusal, detail: String) -> Self {
        Self::Refused {
            error,
            detail,
            release: Some(RELEASE.to_owned()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Refusal {
    /// The bridge found another niri or display than the engine serves, or looks for the
    /// desktop's programs elsewhere.
    SessionMismatch,
    /// The bridge runs another build, or speaks another hello.
    EngineVersion,
    /// The engine serves as many connections as it takes.
    EngineBusy,
    /// The line isn't a hello.
    BadHello,
    /// The engine couldn't open the client's working directory, where it starts the
    /// client's programs.
    NoDirectory,
}

/// The display's socket with every symlink resolved, as a hello names it, so two
/// spellings of one socket's path match; as given when it can't be resolved.
pub(crate) fn resolved(display: &Path) -> PathBuf {
    std::fs::canonicalize(display).unwrap_or_else(|_| display.to_path_buf())
}

/// What the engine compares a hello with: its own build, its resolved niri socket and
/// display, and where it looked for the desktop's programs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Own {
    pub(crate) pid: u32,
    pub(crate) exe: Exe,
    /// The file the engine was started from, as `Build` names it.
    pub(crate) file: Option<PathBuf>,
    pub(crate) niri_socket: PathBuf,
    pub(crate) wayland_socket: Option<PathBuf>,
    pub(crate) desktop_path: String,
}

impl Own {
    /// Takes `line` as a hello from a process running `peer`, or says why not.
    pub(crate) fn take(&self, line: &[u8], peer: &Build) -> Result<Hello, (Refusal, String)> {
        let value = serde_json::from_slice::<serde_json::Value>(line).ok();
        let version = value
            .as_ref()
            .and_then(|value| value.get("hello")?.as_u64());
        if let Some(version) = version.filter(|version| *version != u64::from(VERSION)) {
            let theirs =
                format!(" (one speaking hello {version}, where this engine speaks {VERSION})");
            return Err((Refusal::EngineVersion, self.another_build(&theirs, peer)));
        }
        let hello: Hello = serde_json::from_slice(line)
            .map_err(|error| (Refusal::BadHello, format!("read the hello: {error}")))?;
        if hello.exe != self.exe {
            let theirs = hello.release.as_ref().map_or_else(
                || " (0.1.1 or older)".to_owned(),
                |release| format!(" ({release})"),
            );
            return Err((Refusal::EngineVersion, self.another_build(&theirs, peer)));
        }
        if hello.niri_socket != self.niri_socket || hello.wayland_socket != self.wayland_socket {
            return Err((
                Refusal::SessionMismatch,
                format!(
                    "the bridge found niri at {} and the display at {:?}; this engine serves {} and {:?}",
                    hello.niri_socket.display(),
                    hello.wayland_socket,
                    self.niri_socket.display(),
                    self.wayland_socket,
                ),
            ));
        }
        // Each client of an engine gets the `loginctl` and `noctalia` it found, so a
        // client's own setting can't change them for the others.
        if hello.desktop_path != self.desktop_path {
            return Err((
                Refusal::SessionMismatch,
                format!(
                    "the bridge looks for loginctl and noctalia in {:?}; this engine looked in {:?}",
                    hello.desktop_path, self.desktop_path,
                ),
            ));
        }
        Ok(hello)
    }

    /// Why a bridge of another build, `theirs` saying what it is, run by a process
    /// running `peer`, is refused, and what the user does about it. An engine refuses such
    /// a bridge for as long as it runs, which is until its last client leaves, so retrying
    /// doesn't help: restarting or reconfiguring a client does.
    fn another_build(&self, theirs: &str, peer: &Build) -> String {
        let own = Build {
            replaced: self
                .file
                .as_deref()
                .is_none_or(|file| Exe::of(file).ok() != Some(self.exe)),
            exe: self.file.clone(),
        };
        format!(
            "this engine, PID {}, runs niri-computer-use {RELEASE}{}; it keeps that build until its last client leaves, and this client's server runs another build{theirs}{}: {}. Until then the engine refuses this client, whatever it retries",
            self.pid,
            own.whence(),
            peer.whence(),
            restart(&own, peer)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own() -> Own {
        Own {
            pid: 42,
            exe: Exe {
                dev: 1,
                ino: 2,
                size: 3,
                mtime: 4,
                mtime_nsec: 5,
            },
            file: None,
            niri_socket: PathBuf::from("/run/user/1000/niri.wayland-1.42.sock"),
            wayland_socket: Some(PathBuf::from("/run/user/1000/wayland-1")),
            desktop_path: "/usr/bin".to_owned(),
        }
    }

    /// A bridge's hello for `exe`, niri and the display, with the client's settings.
    fn hello(exe: Exe, niri: &str, display: Option<&str>) -> Hello {
        Hello {
            version: VERSION,
            release: Some(RELEASE.to_owned()),
            exe,
            niri_socket: PathBuf::from(niri),
            wayland_socket: display.map(PathBuf::from),
            desktop_path: "/usr/bin".to_owned(),
            unrestricted: Some("1".to_owned()),
            confirm_lease: Some("1".to_owned()),
            tools: Some("act".to_owned()),
            home: Some(PathBuf::from("/home/me")),
            path: Some("bin::/usr/bin".to_owned()),
            policy: policy::Source::Text {
                path: PathBuf::from("/home/me/.config/niri-computer-use/policy.toml"),
                text: "unrestricted = true\n".to_owned(),
            },
        }
    }

    const NIRI: &str = "/run/user/1000/niri.wayland-1.42.sock";
    /// A bridge whose process the engine couldn't read.
    const UNKNOWN: Build = Build {
        exe: None,
        replaced: true,
    };
    const DISPLAY: Option<&str> = Some("/run/user/1000/wayland-1");

    fn refusal(own: &Own, sent: &Hello) -> Refusal {
        own.take(&serde_json::to_vec(sent).unwrap(), &UNKNOWN)
            .unwrap_err()
            .0
    }

    #[test]
    fn a_matching_hello_brings_the_clients_environment() {
        let own = own();
        let sent = hello(own.exe, NIRI, DISPLAY);
        let taken = own
            .take(&serde_json::to_vec(&sent).unwrap(), &UNKNOWN)
            .unwrap();
        assert_eq!(
            taken.given(),
            Given {
                unrestricted: Some("1".into()),
                confirm_lease: Some("1".into()),
                tools: Some("act".into()),
                home: Some(PathBuf::from("/home/me")),
                path: Some("bin::/usr/bin".into()),
                policy: sent.policy,
            }
        );
    }

    #[test]
    fn another_build_niri_or_display_is_refused() {
        let own = own();
        let rebuilt = Exe { size: 9, ..own.exe };
        assert_eq!(
            refusal(&own, &hello(rebuilt, NIRI, DISPLAY)),
            Refusal::EngineVersion
        );
        let other_niri = "/run/user/1000/niri.wayland-2.7.sock";
        assert_eq!(
            refusal(&own, &hello(own.exe, other_niri, DISPLAY)),
            Refusal::SessionMismatch
        );
        let other_display = Some("/run/user/1000/wayland-2");
        assert_eq!(
            refusal(&own, &hello(own.exe, NIRI, other_display)),
            Refusal::SessionMismatch
        );
        assert_eq!(
            refusal(&own, &hello(own.exe, NIRI, None)),
            Refusal::SessionMismatch
        );
        let elsewhere = Hello {
            desktop_path: "/opt/bin".to_owned(),
            ..hello(own.exe, NIRI, DISPLAY)
        };
        assert_eq!(refusal(&own, &elsewhere), Refusal::SessionMismatch);
        let newer = Hello {
            version: VERSION + 1,
            ..hello(own.exe, NIRI, DISPLAY)
        };
        assert_eq!(refusal(&own, &newer), Refusal::EngineVersion);
        // A bridge from before `desktop_path` speaks hello 1, an older version, not a
        // malformed hello.
        let mut previous = serde_json::to_value(hello(own.exe, NIRI, DISPLAY)).unwrap();
        previous["hello"] = 1.into();
        previous.as_object_mut().unwrap().remove("desktop_path");
        let previous = serde_json::to_vec(&previous).unwrap();
        assert_eq!(
            own.take(&previous, &UNKNOWN).unwrap_err().0,
            Refusal::EngineVersion
        );
        assert_eq!(
            own.take(b"GET / HTTP/1.1", &UNKNOWN).unwrap_err().0,
            Refusal::BadHello
        );
    }

    /// The hello lines of 0.1.0's and 0.1.1's bridges, as they send them, with `exe` for
    /// their file.
    fn released_hello(release: &str, exe: Exe) -> Vec<u8> {
        let mut hello = serde_json::to_value(hello(exe, NIRI, DISPLAY)).unwrap();
        let hello = hello.as_object_mut().unwrap();
        hello.remove("release");
        if release == "0.1.0" {
            hello.insert("hello".to_owned(), 2.into());
            hello.insert("keyboard".to_owned(), serde_json::Value::Null);
        }
        serde_json::to_vec(&hello).unwrap()
    }

    /// An engine of this build started from a file in a fresh directory, which the test
    /// removes.
    fn installed_engine(name: &str) -> (Own, PathBuf) {
        let dir = crate::test_support::fresh_dir(name);
        let file = dir.join("niri-computer-use");
        std::fs::write(&file, "the build the engine started from").unwrap();
        let own = Own {
            exe: Exe::of(&file).unwrap(),
            file: Some(file),
            ..own()
        };
        (own, dir)
    }

    /// The server of a client started from `file`, which another build has since
    /// replaced, as after an upgrade, when `replaced`.
    fn bridge_from(file: &str, replaced: bool) -> Build {
        Build {
            exe: Some(PathBuf::from(file)),
            replaced,
        }
    }

    /// After an upgrade, a client still running the old build meets the new build's
    /// engine, which refuses it for as long as it runs: its detail names both builds and
    /// says to restart that client, as its own bridge's words say only that the next call
    /// tries again.
    #[test]
    fn a_bridge_an_upgrade_replaced_is_told_to_restart_its_client() {
        let (own, dir) = installed_engine("hello-upgrade");
        let file = own.file.clone().unwrap();
        let old = Exe { size: 9, ..own.exe };
        let peer = bridge_from(&file.to_string_lossy(), true);
        for release in ["0.1.0", "0.1.1"] {
            let (error, detail) = own.take(&released_hello(release, old), &peer).unwrap_err();
            assert_eq!(error, Refusal::EngineVersion);
            assert!(
                detail.contains(&format!(
                    "niri-computer-use {RELEASE} from {};",
                    file.display()
                )) && detail
                    .contains("which another build has since replaced: restart this MCP client"),
                "{release}: {detail}"
            );
        }
        let newer = Hello {
            release: Some("9.9.9".to_owned()),
            ..hello(old, NIRI, DISPLAY)
        };
        let (_, detail) = own
            .take(&serde_json::to_vec(&newer).unwrap(), &peer)
            .unwrap_err();
        assert!(detail.contains("another build (9.9.9)"), "{detail}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Two installs, such as one by `cargo install` and one by the install script, each
    /// with clients: neither file was replaced, so restarting either client runs the same
    /// build again, and the engine says to point the clients at one file instead.
    #[test]
    fn a_bridge_of_another_install_is_told_to_run_one_build() {
        let (own, dir) = installed_engine("hello-two-installs");
        let file = own.file.clone().unwrap();
        let cargo = "/home/me/.cargo/bin/niri-computer-use";
        let other = Exe { size: 9, ..own.exe };
        let (error, detail) = own
            .take(
                &serde_json::to_vec(&hello(other, NIRI, DISPLAY)).unwrap(),
                &bridge_from(cargo, false),
            )
            .unwrap_err();
        assert_eq!(error, Refusal::EngineVersion);
        assert!(
            detail.contains(&format!(
                "both builds are still installed: point this MCP client at {}, or the ones the engine serves at {cargo}",
                file.display()
            )) && !detail.contains("restart"),
            "{detail}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// An engine whose own file an upgrade replaced keeps its build until its last client
    /// leaves: a client of the installed build is told to restart the engine's clients.
    #[test]
    fn an_engine_an_upgrade_replaced_says_to_restart_its_clients() {
        let (own, dir) = installed_engine("hello-replaced");
        let file = own.file.clone().unwrap();
        std::fs::remove_file(&file).unwrap();
        std::fs::write(&file, "the installed build, replacing it").unwrap();
        let installed = Exe::of(&file).unwrap();
        let (error, detail) = own
            .take(
                &serde_json::to_vec(&hello(installed, NIRI, DISPLAY)).unwrap(),
                &bridge_from(&file.to_string_lossy(), false),
            )
            .unwrap_err();
        assert_eq!(error, Refusal::EngineVersion);
        assert!(
            detail.contains("which another build has since replaced; it keeps that build")
                && detail.contains("restart the MCP clients the engine serves"),
            "{detail}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_policy_file_too_long_to_send_counts_as_unreadable() {
        let own = own();
        let given = |text: String| Given {
            unrestricted: None,
            confirm_lease: None,
            tools: None,
            home: None,
            path: None,
            policy: policy::Source::Text {
                path: PathBuf::from("/p.toml"),
                text,
            },
        };
        let sent = |text| {
            Hello::new(
                own.exe,
                &own.niri_socket,
                None,
                &own.desktop_path,
                given(text),
            )
            .unwrap()
        };
        assert!(matches!(
            sent("#".repeat(MAX_POLICY)).given().policy,
            policy::Source::Text { .. }
        ));
        assert!(matches!(
            sent("#".repeat(MAX_POLICY + 1)).given().policy,
            policy::Source::Unreadable { .. }
        ));
    }
}
