//! A private headless parent display. No host Wayland socket or bus is inherited.

use std::fs;
use std::os::unix::fs::{DirBuilderExt as _, FileTypeExt as _};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::environment::Env;
use crate::failure::{Context as _, Failure, Result};
use crate::runner::{self, ChildEnv, Group, Invocation, Process, Sink};

const STARTUP: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub(crate) struct Headless {
    process: Process,
}

impl Headless {
    pub(crate) fn start(
        root: &Path,
        parent: &Env,
        artifacts: &Path,
        deadline: Duration,
    ) -> Result<Self> {
        let runtime = root.join("cage");
        let display = runtime.join("wayland-0");
        if display.as_os_str().len() >= 108 {
            return Err(Failure::new(
                "headless Wayland socket path must be under 108 bytes",
            ));
        }
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&runtime)
            .context(format!("create {}", runtime.display()))?;
        let env = environment(&runtime, parent);
        let mut process = runner::start(&Invocation {
            program: "cage",
            args: vec![
                "-d".into(),
                "--".into(),
                "sleep".into(),
                deadline.as_secs().to_string().into(),
            ],
            env: ChildEnv::Exact(&env),
            output: Sink::File(artifacts.join("cage.log")),
            group: Group::Own,
            deadline,
        })?;
        let end = Instant::now() + STARTUP;
        loop {
            process.ensure_running()?;
            if fs::symlink_metadata(&display).is_ok_and(|entry| entry.file_type().is_socket()) {
                return Ok(Self { process });
            }
            if Instant::now() >= end {
                return Err(Failure::new("cage startup deadline exceeded; see cage.log"));
            }
            pause();
        }
    }

    pub(crate) fn stop(self) -> Result<()> {
        self.process.stop().map(|_| ())
    }
}

fn environment(runtime: &Path, parent: &Env) -> Env {
    Env::from([
        ("PATH", parent["PATH"].clone()),
        ("XDG_RUNTIME_DIR", runtime.as_os_str().to_owned()),
        ("WLR_BACKENDS", "headless".into()),
        ("WLR_LIBINPUT_NO_DEVICES", "1".into()),
    ])
}

#[expect(
    clippy::disallowed_methods,
    reason = "the synchronous harness has no async runtime; startup polling is bounded"
)]
fn pause() {
    std::thread::sleep(Duration::from_millis(50));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_headless_parent_reports_its_exit_without_visible_fallback() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = std::env::temp_dir().join(format!("ncu-cage-failure-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        let script = root.join("bin/cage");
        fs::write(
            &script,
            "#!/bin/sh\nprintf 'fixture cage failed\\n' >&2\nexit 7\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
        let parent = Env::from([("PATH", root.join("bin").into_os_string())]);
        let error = Headless::start(&root, &parent, &root, Duration::from_secs(3)).unwrap_err();
        assert!(error.to_string().contains("exit status: 7"), "{error}");
        assert!(
            fs::read_to_string(root.join("cage.log"))
                .unwrap()
                .contains("fixture cage failed")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn headless_parent_cannot_connect_to_host_displays_or_buses() {
        let parent = Env::from([
            ("PATH", "/fake/bin".into()),
            ("WAYLAND_DISPLAY", "/host/wayland-1".into()),
            ("NIRI_SOCKET", "/host/niri.sock".into()),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/host/bus".into()),
        ]);
        let isolated = environment(Path::new("/private/cage"), &parent);
        assert_eq!(isolated.len(), 4);
        assert_eq!(isolated["XDG_RUNTIME_DIR"], "/private/cage");
        assert_eq!(isolated["WLR_BACKENDS"], "headless");
        assert_eq!(isolated["WLR_LIBINPUT_NO_DEVICES"], "1");
        for key in ["WAYLAND_DISPLAY", "NIRI_SOCKET", "DBUS_SESSION_BUS_ADDRESS"] {
            assert!(!isolated.contains_key(key));
        }
    }
}
