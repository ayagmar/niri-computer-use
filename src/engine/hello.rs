//! The hello: the first line each way on a connection to the engine, before MCP. The
//! bridge says which build it is, which niri and display it found, and what its client's
//! environment gives the session; the engine takes it as a new session or refuses it.
//! Pure, apart from reading this binary's identity.

use std::os::unix::fs::MetadataExt as _;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::policy;
use crate::session::Given;

const VERSION: u32 = 1;
/// A hello line, newline included. The policy file's text, at most `MAX_POLICY`, can grow
/// up to six times in JSON.
pub(crate) const MAX_LINE: usize = 512 * 1024;
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
        let meta = std::fs::metadata("/proc/self/exe")
            .map_err(|error| format!("stat /proc/self/exe: {error}"))?;
        Ok(Self {
            dev: meta.dev(),
            ino: meta.ino(),
            size: meta.size(),
            mtime: meta.mtime(),
            mtime_nsec: meta.mtime_nsec(),
        })
    }
}

/// What a bridge sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Hello {
    #[serde(rename = "hello")]
    version: u32,
    exe: Exe,
    niri_socket: PathBuf,
    wayland_socket: Option<PathBuf>,
    unrestricted: Option<String>,
    keyboard: Option<String>,
    home: Option<PathBuf>,
    policy: policy::Source,
}

impl Hello {
    /// What the client's environment gave its session.
    pub(crate) fn given(self) -> Given {
        Given {
            unrestricted: self.unrestricted.map(Into::into),
            keyboard: self.keyboard.map(Into::into),
            home: self.home,
            policy: self.policy,
        }
    }
}

/// What the engine answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reply {
    Engine { pid: u32 },
    Refused { error: Refusal, detail: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Refusal {
    /// The bridge found another niri or display than the engine serves.
    SessionMismatch,
    /// The bridge runs another build, or speaks another hello.
    EngineVersion,
    /// The engine serves as many connections as it takes.
    EngineBusy,
    /// The line isn't a hello.
    BadHello,
}

/// What the engine compares a hello with: its own build, niri and display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Own {
    pub(crate) exe: Exe,
    pub(crate) niri_socket: PathBuf,
    pub(crate) wayland_socket: Option<PathBuf>,
}

impl Own {
    /// Takes `line` as a hello, or says why not.
    pub(crate) fn take(&self, line: &[u8]) -> Result<Hello, (Refusal, String)> {
        let version = serde_json::from_slice::<serde_json::Value>(line)
            .ok()
            .and_then(|value| value.get("hello")?.as_u64());
        if version.is_some_and(|version| version != u64::from(VERSION)) {
            return Err((
                Refusal::EngineVersion,
                format!("the bridge speaks hello {version:?}; this engine speaks {VERSION}"),
            ));
        }
        let hello: Hello = serde_json::from_slice(line)
            .map_err(|error| (Refusal::BadHello, format!("read the hello: {error}")))?;
        if hello.exe != self.exe {
            return Err((
                Refusal::EngineVersion,
                "the engine runs another build of niri-computer-use".to_owned(),
            ));
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
        Ok(hello)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn own() -> Own {
        Own {
            exe: Exe {
                dev: 1,
                ino: 2,
                size: 3,
                mtime: 4,
                mtime_nsec: 5,
            },
            niri_socket: PathBuf::from("/run/user/1000/niri.wayland-1.42.sock"),
            wayland_socket: Some(PathBuf::from("/run/user/1000/wayland-1")),
        }
    }

    /// A bridge's hello for `exe`, niri and the display, with the client's settings.
    fn hello(exe: Exe, niri: &str, display: Option<&str>) -> Hello {
        Hello {
            version: VERSION,
            exe,
            niri_socket: PathBuf::from(niri),
            wayland_socket: display.map(PathBuf::from),
            unrestricted: Some("1".to_owned()),
            keyboard: None,
            home: Some(PathBuf::from("/home/me")),
            policy: policy::Source::Text {
                path: PathBuf::from("/home/me/.config/niri-computer-use/policy.toml"),
                text: "unrestricted = true\n".to_owned(),
            },
        }
    }

    const NIRI: &str = "/run/user/1000/niri.wayland-1.42.sock";
    const DISPLAY: Option<&str> = Some("/run/user/1000/wayland-1");

    fn refusal(own: &Own, sent: &Hello) -> Refusal {
        own.take(&serde_json::to_vec(sent).unwrap()).unwrap_err().0
    }

    #[test]
    fn a_matching_hello_brings_the_clients_environment() {
        let own = own();
        let sent = hello(own.exe, NIRI, DISPLAY);
        let taken = own.take(&serde_json::to_vec(&sent).unwrap()).unwrap();
        assert_eq!(
            taken.given(),
            Given {
                unrestricted: Some("1".into()),
                keyboard: None,
                home: Some(PathBuf::from("/home/me")),
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
        let newer = Hello {
            version: 2,
            ..hello(own.exe, NIRI, DISPLAY)
        };
        assert_eq!(refusal(&own, &newer), Refusal::EngineVersion);
        assert_eq!(
            own.take(b"GET / HTTP/1.1").unwrap_err().0,
            Refusal::BadHello
        );
    }
}
