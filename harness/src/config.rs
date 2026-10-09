//! Generated configs for the nested niri and its private session bus.

use std::path::Path;

use crate::scale::Scale;

/// The nested niri config. No startup commands, animations, borders or Xwayland, a solid
/// magenta background, a fixed 400x300 floating `wev`, a fixed 560x360 floating `kitty`
/// for the scroll check, and one test bind.
pub(crate) fn niri(scale: Scale, bind_marker: &Path) -> String {
    let marker = bind_marker.display();
    format!(
        r##"hotkey-overlay {{
    skip-at-startup
}}

xwayland-satellite {{
    off
}}

animations {{
    off
}}

output "winit" {{
    scale {scale}
}}

layout {{
    background-color "#ff00ff"
    border {{
        off
    }}
    focus-ring {{
        off
    }}
}}

window-rule {{
    match app-id="^wev$"
    open-floating true
    default-floating-position x=0 y=0 relative-to="top-left"
    default-column-width {{ fixed 400; }}
    default-window-height {{ fixed 300; }}
}}

window-rule {{
    match app-id="^kitty$"
    open-floating true
    default-floating-position x=0 y=0 relative-to="top-left"
    default-column-width {{ fixed 560; }}
    default-window-height {{ fixed 360; }}
}}

binds {{
    Ctrl+Shift+F12 {{ spawn "touch" "{marker}"; }}
}}
"##
    )
}

/// The nested Noctalia's config. With a fresh state directory Noctalia 5.2.1 opens its
/// setup wizard as a panel on startup (`application_ui.cpp`), and weather fetches from the
/// network. At every start Noctalia also clones each enabled git plugin source that isn't
/// cached yet, by default the official and community repositories on GitHub
/// (`ensureEnabledMaterialized` in `plugin_manager.cpp`), in a process group of its own that
/// outlives the run (`process.cpp`). An explicit empty `source` array leaves no sources:
/// the defaults only apply when the array is absent (`config_service.cpp`).
/// The rest stays at the defaults. Its system-bus services, among them logind
/// inhibitors and the Bluetooth and network agents, find no bus in NESTED.
pub(crate) const NOCTALIA: &str = r"[shell]
setup_wizard_enabled = false

[weather]
enabled = false

[plugins]
source = []
";

/// The private session bus config. It listens only inside `listen_dir` and has no service
/// directories, so nothing on the host gets activated through it.
pub(crate) fn dbus(listen_dir: &Path) -> String {
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
    fn niri_config_sets_scale_and_bind_marker() {
        let config = niri("1.5".parse().unwrap(), Path::new("/t/bind-fired"));
        assert!(config.contains("    scale 1.5\n"));
        assert!(config.contains(r#"spawn "touch" "/t/bind-fired";"#));
        assert!(!config.contains("spawn-at-startup"));
    }

    #[test]
    fn noctalia_config_turns_off_the_wizard_weather_and_plugin_sources() {
        assert!(NOCTALIA.contains("[shell]\nsetup_wizard_enabled = false\n"));
        assert!(NOCTALIA.contains("[weather]\nenabled = false\n"));
        assert!(NOCTALIA.contains("[plugins]\nsource = []\n"));
    }

    #[test]
    fn dbus_config_listens_only_in_the_given_dir() {
        let config = dbus(Path::new("/t/run"));
        assert!(config.contains("<listen>unix:dir=/t/run</listen>"));
        assert_eq!(config.matches("<listen>").count(), 1);
        assert!(!config.contains("servicedir"));
    }
}
