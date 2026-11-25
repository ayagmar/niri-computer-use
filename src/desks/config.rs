//! The configs a desk generates: never the owner's `config.kdl`, whose `spawn-at-startup`
//! would start a second shell, bar or daemon inside the desk. `desk start` validates the
//! niri config with `niri validate` before niri runs, since niri runs its built-in default
//! config, startup commands included, when its config doesn't parse.

use std::fmt::Write as _;
use std::path::Path;

use super::environment::SWAP_INTERVAL;

/// The nested niri's config: no startup commands, no binds of its own (the host's stop key
/// is the host's), no animations, so the agent's screenshots settle at once, and no
/// Xwayland, whose X11 sockets are shared by every session in `/tmp/.X11-unix`. The swap
/// interval switches niri gets are taken back out of what it starts.
pub(crate) fn niri() -> String {
    let mut unset = String::new();
    for (name, _) in SWAP_INTERVAL {
        // `writeln!` to a `String` can't fail.
        writeln!(unset, "    {name} null").ok();
    }
    format!(
        r"hotkey-overlay {{
    skip-at-startup
}}

xwayland-satellite {{
    off
}}

animations {{
    off
}}

environment {{
{unset}}}
"
    )
}

/// The desk's session bus: it listens only in the desk's directory and has no service
/// directories, so nothing on the host is activated through it.
pub(crate) fn bus(listen_dir: &Path) -> String {
    let dir = listen_dir.display();
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>unix:dir={dir}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_niri_config_starts_nothing_and_unsets_the_swap_interval_switches() {
        let config = niri();
        assert!(!config.contains("spawn"), "{config}");
        assert!(!config.contains("binds"), "{config}");
        assert!(
            config.contains("    __GL_SYNC_TO_VBLANK null\n"),
            "{config}"
        );
        assert!(config.contains("    vblank_mode null\n"), "{config}");
    }

    #[test]
    fn the_bus_listens_only_in_the_desk_and_activates_nothing() {
        let config = bus(Path::new("/run/user/1000/ncu-d/web"));
        assert!(config.contains("<listen>unix:dir=/run/user/1000/ncu-d/web</listen>"));
        assert!(!config.contains("servicedir"), "{config}");
        assert!(!config.contains("standard_session_servicedirs"), "{config}");
    }
}
