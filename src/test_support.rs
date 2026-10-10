//! Helpers shared by unit tests.

pub(crate) mod output_mode;

use std::path::PathBuf;

/// A directory the calling test creates itself, so the test reads and removes nothing
/// else. Creation fails if the name already exists.
pub(crate) fn fresh_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "niri-computer-use-{name}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir(&dir).unwrap();
    dir
}

/// Session `id` of one process, with no policy file and nothing turned on.
pub(crate) fn session(id: u64) -> crate::session::Session {
    let given = crate::session::Given {
        unrestricted: None,
        keyboard: None,
        home: None,
        policy: crate::policy::Source::Missing,
    };
    crate::session::Session::new(
        crate::session::SessionId(id),
        std::process::id(),
        crate::session::Settings::new(given),
    )
}

/// A screenshot ref of a whole 960x720 output at scale 1.5, flipped, taken now.
pub(crate) fn shot() -> crate::refs::Shot {
    use niri_ipc::{LogicalOutput, Transform};

    let geometry = LogicalOutput {
        x: 0,
        y: 0,
        width: 960,
        height: 720,
        scale: 1.5,
        transform: Transform::Flipped180,
    };
    crate::refs::Shot {
        output: "winit".to_owned(),
        geometry,
        motion_geometry: Some(geometry),
        captured: crate::observe::Rect {
            x: 0,
            y: 0,
            width: 960,
            height: 720,
        },
        scale: 1.5,
        width: 1440,
        height: 1080,
        taken: tokio::time::Instant::now(),
        connection: 1,
    }
}
