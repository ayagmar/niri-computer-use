//! Generated configs for the nested niri and its private session bus.

use std::path::Path;

use crate::scale::Scale;

/// Who draws window decorations in the nested niri.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decorations {
    /// niri's default: apps draw their own.
    Client,
    /// `prefer-no-csd`: apps that honour it leave their decorations to niri, which draws
    /// none here, as borders are off.
    Server,
}

impl Decorations {
    /// The supervisor's flag for `Server`.
    pub(crate) const SERVER_FLAG: &'static str = "--ssd";
}

/// The nested niri config. No startup commands, animations, borders or Xwayland, a solid
/// magenta background, a fixed 400x300 floating `wev`, a fixed 560x360 floating `kitty`
/// for the scroll check, the accessibility fixtures floating at 400x300, one test bind, and
/// a stop key running `server`.
pub(crate) fn niri(
    scale: Scale,
    bind_marker: &Path,
    decorations: Decorations,
    server: &Path,
) -> String {
    let marker = bind_marker.display();
    let server = server.display();
    let prefer_no_csd = match decorations {
        Decorations::Client => "",
        Decorations::Server => "prefer-no-csd\n\n",
    };
    format!(
        r##"{prefer_no_csd}hotkey-overlay {{
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
    match app-id="^org\\.ncu\\.Activation$"
    match app-id="^org\\.ncu\\.Copy$"
    open-floating true
    default-floating-position x=0 y=0 relative-to="top-left"
    default-column-width {{ fixed 320; }}
    default-window-height {{ fixed 240; }}
}}

window-rule {{
    match app-id="^org\\.ncu\\.A11y$"
    match app-id="^org\\.ncu\\.List$"
    match app-id="^org\\.ncu\\.Gtk3$"
    match app-id="^org\\.qt-project\\.qml6?$"
    open-floating true
    default-floating-position x=20 y=20 relative-to="top-left"
    default-column-width {{ fixed 400; }}
    default-window-height {{ fixed 300; }}
}}

window-rule {{
    match app-id="^org\\.ncu\\.Pacer$"
    open-floating true
    default-floating-position x=0 y=0 relative-to="bottom-right"
    default-column-width {{ fixed 320; }}
    default-window-height {{ fixed 240; }}
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
    // For `status.stop_key`; nothing presses it.
    Ctrl+Shift+F11 {{ spawn "{server}" "stop"; }}
}}
"##
    )
}

/// The nested niri config for demo recordings: what a niri desktop looks like in use, so
/// animations are on, columns have gaps and rounded corners, and the focused window has
/// a focus ring in the site's accent. Kitty and the chat fixture open as tiled columns
/// wide enough together that focusing one scrolls the other out of view. No binds, no
/// startup commands, no Xwayland.
pub(crate) fn demo_niri(scale: Scale) -> String {
    format!(
        r##"prefer-no-csd

hotkey-overlay {{
    skip-at-startup
}}

xwayland-satellite {{
    off
}}

output "winit" {{
    scale {scale}
}}

layout {{
    gaps 16
    background-color "#0d0f10"
    default-column-width {{ proportion 0.5; }}
    focus-ring {{
        width 3
        active-color "#f2a33a"
        inactive-color "#3a3f44"
    }}
    border {{
        off
    }}
    shadow {{
        on
        softness 30
        spread 4
        offset x=0 y=8
        color "#00000070"
    }}
}}

window-rule {{
    geometry-corner-radius 10
    clip-to-geometry true
}}

window-rule {{
    match app-id="^kitty$"
    default-column-width {{ proportion 0.62; }}
}}

window-rule {{
    match app-id="^org\\.ncu\\.Chat$"
    default-column-width {{ proportion 0.5; }}
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
/// Its wallpaper panel lists `wallpapers`, an empty directory in `TEST_DIR`, instead of
/// the host's pictures directory.
/// The rest stays at the defaults. Its system-bus services, among them logind
/// inhibitors and the Bluetooth and network agents, find no bus in NESTED.
pub(crate) fn noctalia(wallpapers: &Path) -> String {
    noctalia_with(wallpapers, "")
}

/// `noctalia`'s config with this checkout's plugin enabled, running `server`'s `watch`.
pub(crate) fn noctalia_plugin(wallpapers: &Path, server: &Path) -> String {
    let id = crate::input::overlay::PLUGIN_ID;
    let server = serde_json::Value::from(server.to_string_lossy());
    let config = noctalia_with(wallpapers, &format!("enabled = [\"{id}\"]\n"));
    format!("{config}\n[plugin_settings.\"{id}\"]\nbinary = {server}\n")
}

/// `plugins` goes into the `[plugins]` table, which lists no plugin sources.
fn noctalia_with(wallpapers: &Path, plugins: &str) -> String {
    // A JSON string is a valid TOML basic string.
    let wallpapers = serde_json::Value::from(wallpapers.to_string_lossy());
    format!(
        "[shell]
setup_wizard_enabled = false

[weather]
enabled = false

[plugins]
source = []
{plugins}
[wallpaper]
directory = {wallpapers}
"
    )
}

/// The nested Noctalia's config for demo recordings: `noctalia`'s, with a bar that shows
/// what a viewer needs (workspaces, the clock, notifications and the control center) and
/// colours taken from the demo's wallpaper.
pub(crate) fn noctalia_demo(wallpapers: &Path) -> String {
    format!(
        "{}
[theme]
source = \"wallpaper\"

[bar.default]
start = [\"workspaces\"]
center = [\"clock\"]
end = [\"notifications\", \"control-center\"]
",
        noctalia(wallpapers)
    )
}

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
        let config = niri(
            "1.5".parse().unwrap(),
            Path::new("/t/bind-fired"),
            Decorations::Client,
            Path::new("/t/niri-computer-use"),
        );
        assert!(config.contains("    scale 1.5\n"));
        assert!(!config.contains("prefer-no-csd"));
        assert!(config.contains(r#"spawn "touch" "/t/bind-fired";"#));
        assert!(!config.contains("spawn-at-startup"));
    }

    #[test]
    fn server_decorations_prefer_no_csd() {
        let config = niri(
            Scale::ONE,
            Path::new("/t/m"),
            Decorations::Server,
            Path::new("/t/niri-computer-use"),
        );
        assert!(config.starts_with("prefer-no-csd\n\n"));
    }

    #[test]
    fn demo_config_shows_niri_as_used_without_binds_or_startup_commands() {
        let config = demo_niri(Scale::ONE);
        assert!(!config.contains("animations"));
        assert!(!config.contains("binds"));
        assert!(!config.contains("spawn"));
    }

    #[test]
    fn noctalia_config_turns_off_the_wizard_weather_and_plugin_sources() {
        let config = noctalia(Path::new("/t/data/wallpapers"));
        assert!(config.contains("[shell]\nsetup_wizard_enabled = false\n"));
        assert!(config.contains("[weather]\nenabled = false\n"));
        assert!(config.contains("[plugins]\nsource = []\n"));
        assert!(config.contains("[wallpaper]\ndirectory = \"/t/data/wallpapers\"\n"));
    }

    #[test]
    fn demo_noctalia_config_keeps_the_test_settings() {
        let config = noctalia_demo(Path::new("/t/data/wallpapers"));
        assert!(config.starts_with(&noctalia(Path::new("/t/data/wallpapers"))));
    }

    #[test]
    fn dbus_config_listens_only_in_the_given_dir() {
        let config = dbus(Path::new("/t/run"));
        assert!(config.contains("<listen>unix:dir=/t/run</listen>"));
        assert_eq!(config.matches("<listen>").count(), 1);
        assert!(!config.contains("servicedir"));
    }
}
