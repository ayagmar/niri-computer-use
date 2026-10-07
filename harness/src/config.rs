//! Generated configs for the nested niri and its private session bus.

use std::path::Path;

use crate::scale::Scale;

/// The nested niri config. No startup commands, animations, borders or Xwayland, a solid
/// magenta background, a fixed 400x300 floating `wev`, and one test bind.
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

binds {{
    Ctrl+Shift+F12 {{ spawn "touch" "{marker}"; }}
}}
"##
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
        let config = niri("1.5".parse().unwrap(), Path::new("/t/bind-fired"));
        assert!(config.contains("    scale 1.5\n"));
        assert!(config.contains(r#"spawn "touch" "/t/bind-fired";"#));
        assert!(!config.contains("spawn-at-startup"));
    }

    #[test]
    fn dbus_config_listens_only_in_the_given_dir() {
        let config = dbus(Path::new("/t/run"));
        assert!(config.contains("<listen>unix:dir=/t/run</listen>"));
        assert_eq!(config.matches("<listen>").count(), 1);
        assert!(!config.contains("servicedir"));
    }
}
