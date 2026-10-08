//! The niri version rule. Major and minor must match the pinned `niri-ipc`; a different
//! patch only warns. Anything unreadable fails closed.

use serde::Serialize;

/// The pinned `niri-ipc` version. A test keeps it equal to the pin in `Cargo.toml`.
pub(crate) const IPC_CRATE: &str = "26.4.0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Compat {
    Ok,
    PatchWarning,
    ReadOnly,
}

/// Compares niri's `Request::Version` reply, such as `26.04 (8ed0da4)`, with the pin.
pub(crate) fn compat(niri: &str) -> Compat {
    let release = niri.split_once(" (").map_or(niri, |(release, _)| release);
    let (Some(running), Some(pinned)) = (numbers(release), numbers(IPC_CRATE)) else {
        return Compat::ReadOnly;
    };
    match (running.as_slice(), pinned.as_slice()) {
        ([major, minor, rest @ ..], [pin_major, pin_minor, pin_patch])
            if major == pin_major && minor == pin_minor =>
        {
            match rest {
                [] => Compat::Ok,
                [patch] if patch == pin_patch => Compat::Ok,
                [_] => Compat::PatchWarning,
                [_, _, ..] => Compat::ReadOnly,
            }
        }
        _ => Compat::ReadOnly,
    }
}

/// `26.04` is 26 and 4.
fn numbers(version: &str) -> Option<Vec<u32>> {
    version.split('.').map(|part| part.parse().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pin_matches_cargo_toml() {
        let manifest = include_str!("../../Cargo.toml");
        assert!(manifest.contains(&format!("niri-ipc = \"={IPC_CRATE}\"")));
    }

    #[test]
    fn major_and_minor_must_match_and_patch_only_warns() {
        assert_eq!(compat("26.04 (8ed0da4)"), Compat::Ok);
        assert_eq!(compat("26.04"), Compat::Ok);
        assert_eq!(compat("26.04.0 (abc)"), Compat::Ok);
        assert_eq!(compat("26.04.1 (abc)"), Compat::PatchWarning);
        assert_eq!(compat("26.10 (abc)"), Compat::ReadOnly);
        assert_eq!(compat("25.04 (abc)"), Compat::ReadOnly);
        assert_eq!(compat("27.4"), Compat::ReadOnly);
    }

    #[test]
    fn an_unreadable_version_fails_closed() {
        for version in ["", "26", "26.x", "unknown (abc)", "26.04.1.2", "26.04-rc1"] {
            assert_eq!(compat(version), Compat::ReadOnly, "{version:?}");
        }
    }
}
