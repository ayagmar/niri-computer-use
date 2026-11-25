//! The environments of a desk's processes, as pure functions of what the host's
//! environment gives and where the desk lives. Each process gets exactly these
//! variables, nothing else of the host's: an allowlist, so a host variable nobody
//! thought of, such as `DISPLAY` for the host's Xwayland or `SSH_AUTH_SOCK`, never
//! reaches the desk.
//!
//! - cage gets only what a headless wlroots needs, in a runtime directory of its own.
//! - The desk's apps, and the nested niri that starts them, get the host's locale, home and
//!   XDG directories, and the desk's runtime directory, private session bus, `TMPDIR`, and
//!   a system bus address with nothing there.
//! - The nested niri also gets its parent display and the swap interval switches.
//! - A desk's server keeps its own environment, the host's, for the lock state, the
//!   notifications and `systemd-run`, and reaches the desk through `overlay`.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use super::layout::Layout;

/// A process's exact environment.
pub(crate) type Vars = BTreeMap<&'static str, OsString>;

/// What the host's environment gives every desk process that has it.
const KEPT: [&str; 27] = [
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TZ",
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_ADDRESS",
    "LC_COLLATE",
    "LC_CTYPE",
    "LC_IDENTIFICATION",
    "LC_MEASUREMENT",
    "LC_MESSAGES",
    "LC_MONETARY",
    "LC_NAME",
    "LC_NUMERIC",
    "LC_PAPER",
    "LC_TELEPHONE",
    "LC_TIME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_CONFIG_DIRS",
    "XDG_DATA_DIRS",
];

/// Turn the swap interval off in NVIDIA's and Mesa's EGL. niri's winit backend never sets
/// it, so with EGL's default of 1 each frame waits for the parent's frame callback, and a
/// hidden or slow parent stalls every request to the nested niri (`docs/decisions.md`).
/// Each driver ignores the other's.
pub(crate) const SWAP_INTERVAL: [(&str, &str); 2] =
    [("__GL_SYNC_TO_VBLANK", "0"), ("vblank_mode", "0")];

/// The host's variables a desk keeps, read once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Host {
    kept: Vars,
}

impl Host {
    /// Reads `KEPT` through `var`; an empty variable counts as unset.
    pub(crate) fn read(var: impl Fn(&str) -> Option<OsString>) -> Self {
        let kept = KEPT
            .into_iter()
            .filter_map(|name| Some((name, var(name).filter(|value| !value.is_empty())?)))
            .collect();
        Self { kept }
    }

    pub(crate) fn get(&self, name: &str) -> Option<&OsString> {
        self.kept.get(name)
    }
}

/// cage's environment: a headless backend, no input devices, and its own runtime directory.
pub(crate) fn cage(host: &Host, layout: &Layout) -> Vars {
    let mut vars = Vars::new();
    if let Some(path) = host.get("PATH") {
        vars.insert("PATH", path.clone());
    }
    vars.insert("XDG_RUNTIME_DIR", layout.cage_dir().into_os_string());
    vars.insert("WLR_BACKENDS", "headless".into());
    vars.insert("WLR_LIBINPUT_NO_DEVICES", "1".into());
    vars
}

/// The session bus daemon's environment: the apps' without a session bus of their own.
pub(crate) fn bus(host: &Host, layout: &Layout) -> Vars {
    let mut vars = host.kept.clone();
    vars.insert("XDG_RUNTIME_DIR", layout.root().as_os_str().to_owned());
    vars.insert("DBUS_SYSTEM_BUS_ADDRESS", system_bus(layout));
    vars
}

/// The environment of the desk's apps and of the accessibility daemons. niri adds
/// `WAYLAND_DISPLAY` and `NIRI_SOCKET` for what it starts.
pub(crate) fn apps(host: &Host, layout: &Layout, session_bus: &str) -> Vars {
    let mut vars = bus(host, layout);
    vars.insert("DBUS_SESSION_BUS_ADDRESS", session_bus.into());
    vars.insert("TMPDIR", layout.tmp().into_os_string());
    vars.insert("XDG_SESSION_TYPE", "wayland".into());
    vars.insert("XDG_CURRENT_DESKTOP", "niri".into());
    vars
}

/// The nested niri's environment: the apps', its parent display, and the swap interval
/// switches, which its config takes back out of what it starts.
pub(crate) fn niri(host: &Host, layout: &Layout, session_bus: &str, parent: &Path) -> Vars {
    let mut vars = apps(host, layout, session_bus);
    vars.insert("WAYLAND_DISPLAY", parent.as_os_str().to_owned());
    for (name, value) in SWAP_INTERVAL {
        vars.insert(name, value.into());
    }
    vars
}

/// The desk's endpoints, which a desk's server reads in place of its own environment's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Endpoints<'a> {
    pub(crate) root: &'a Path,
    pub(crate) niri_socket: &'a Path,
    pub(crate) wayland_display: &'a str,
    pub(crate) session_bus: &'a str,
}

/// What a desk's server reads in place of its own environment's variables: the desk's
/// runtime directory, niri, display and session bus. Everything else stays the host's.
pub(crate) fn overlay(desk: &Endpoints<'_>) -> Vars {
    Vars::from([
        ("XDG_RUNTIME_DIR", desk.root.as_os_str().to_owned()),
        ("NIRI_SOCKET", desk.niri_socket.as_os_str().to_owned()),
        ("WAYLAND_DISPLAY", desk.wayland_display.into()),
        ("DBUS_SESSION_BUS_ADDRESS", desk.session_bus.into()),
    ])
}

fn system_bus(layout: &Layout) -> OsString {
    let mut address = OsString::from("unix:path=");
    address.push(layout.no_system_bus());
    address
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desks::layout::Desks;
    use crate::desks::name::DeskName;

    /// A host session's variables, endpoints included.
    fn host_vars(name: &str) -> Option<OsString> {
        let value = match name {
            "PATH" => "/usr/bin",
            "HOME" => "/home/u",
            "LANG" => "C.UTF-8",
            "XDG_RUNTIME_DIR" => "/run/user/1000",
            "WAYLAND_DISPLAY" => "wayland-1",
            "WAYLAND_SOCKET" => "3",
            "NIRI_SOCKET" => "/run/user/1000/niri.wayland-1.5050.sock",
            "DISPLAY" => ":0",
            "DBUS_SESSION_BUS_ADDRESS" => "unix:path=/run/user/1000/bus",
            "DBUS_SYSTEM_BUS_ADDRESS" => "unix:path=/run/dbus/system_bus_socket",
            "AT_SPI_BUS_ADDRESS" => "unix:path=/run/user/1000/at-spi/bus",
            "SSH_AUTH_SOCK" => "/run/user/1000/ssh-agent",
            "XDG_SESSION_ID" => "2",
            "LC_TIME" => "",
            _ => return None,
        };
        Some(value.into())
    }

    fn layout() -> Layout {
        Desks::of(Path::new("/run/user/1000"))
            .desk(&DeskName::parse("web").unwrap())
            .unwrap()
    }

    const BUS: &str = "unix:path=/run/user/1000/ncu-d/web/dbus-x,guid=1";

    /// No desk process may reach the host's display, niri, buses, Xwayland or agent.
    fn reaches_nothing_of_the_host(vars: &Vars) {
        for name in [
            "NIRI_SOCKET",
            "WAYLAND_SOCKET",
            "DISPLAY",
            "AT_SPI_BUS_ADDRESS",
            "SSH_AUTH_SOCK",
            "XDG_SESSION_ID",
        ] {
            assert!(!vars.contains_key(name), "{name}");
        }
        for (name, value) in vars {
            let value = value.to_string_lossy();
            for host in [
                "/run/user/1000/bus",
                "/run/dbus/system_bus_socket",
                "wayland-1",
            ] {
                assert!(!value.ends_with(host), "{name}={value}");
            }
        }
    }

    #[test]
    fn apps_get_the_desks_endpoints_and_none_of_the_hosts() {
        let host = Host::read(host_vars);
        let apps = apps(&host, &layout(), BUS);
        reaches_nothing_of_the_host(&apps);
        assert!(!apps.contains_key("WAYLAND_DISPLAY"));
        assert_eq!(apps["XDG_RUNTIME_DIR"], "/run/user/1000/ncu-d/web");
        assert_eq!(apps["DBUS_SESSION_BUS_ADDRESS"], BUS);
        assert_eq!(
            apps["DBUS_SYSTEM_BUS_ADDRESS"],
            "unix:path=/run/user/1000/ncu-d/web/no-system-bus"
        );
        assert_eq!(apps["TMPDIR"], "/run/user/1000/ncu-d/web/tmp");
        assert_eq!(apps["HOME"], "/home/u");
        assert_eq!(apps["LANG"], "C.UTF-8");
        assert!(!apps.contains_key("LC_TIME"), "an empty variable is unset");
        assert!(!apps.contains_key("__GL_SYNC_TO_VBLANK"));
    }

    #[test]
    fn the_nested_niri_gets_its_parent_and_the_swap_interval_switches() {
        let host = Host::read(host_vars);
        let parent = layout().cage_display();
        let niri = niri(&host, &layout(), BUS, &parent);
        assert_eq!(niri["WAYLAND_DISPLAY"], parent.as_os_str());
        assert_eq!(niri["__GL_SYNC_TO_VBLANK"], "0");
        assert_eq!(niri["vblank_mode"], "0");
        let without_parent: Vars = niri
            .into_iter()
            .filter(|(name, _)| *name != "WAYLAND_DISPLAY")
            .collect();
        reaches_nothing_of_the_host(&without_parent);
    }

    #[test]
    fn cage_gets_a_headless_backend_and_nothing_else() {
        let cage = cage(&Host::read(host_vars), &layout());
        assert_eq!(
            cage.keys().copied().collect::<Vec<_>>(),
            [
                "PATH",
                "WLR_BACKENDS",
                "WLR_LIBINPUT_NO_DEVICES",
                "XDG_RUNTIME_DIR"
            ]
        );
        assert_eq!(cage["XDG_RUNTIME_DIR"], "/run/user/1000/ncu-d/web/cage");
        assert_eq!(cage["WLR_BACKENDS"], "headless");
    }

    #[test]
    fn a_desks_server_reads_the_desk_for_niri_the_display_and_the_session_bus_only() {
        let root = layout().root().to_path_buf();
        let socket = root.join("niri.wayland-1.42.sock");
        let overlay = overlay(&Endpoints {
            root: &root,
            niri_socket: &socket,
            wayland_display: "wayland-1",
            session_bus: BUS,
        });
        assert_eq!(
            overlay.keys().copied().collect::<Vec<_>>(),
            [
                "DBUS_SESSION_BUS_ADDRESS",
                "NIRI_SOCKET",
                "WAYLAND_DISPLAY",
                "XDG_RUNTIME_DIR"
            ]
        );
        assert_eq!(overlay["NIRI_SOCKET"], socket.as_os_str());
    }
}
