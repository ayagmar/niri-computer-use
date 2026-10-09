//! Physical modes shared by unit and protocol output fixtures.

/// Reconstructs the fixture's physical size with the existing nearest-integer rounding.
pub(crate) fn from_logical(width: u32, height: u32, scale: f64) -> niri_ipc::Mode {
    niri_ipc::Mode {
        width: format!("{:.0}", f64::from(width) * scale).parse().unwrap(),
        height: format!("{:.0}", f64::from(height) * scale).parse().unwrap(),
        refresh_rate: 60000,
        is_preferred: true,
    }
}
