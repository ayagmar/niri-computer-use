//! Screenshot refs (plan §8): what each screenshot taken under the lease captured, kept in
//! memory so a pointer tool can map a pixel of that image back to the layout. Refs belong
//! to one lease: taking or giving up the lease drops them all, and an id is never issued
//! twice by one server, so a ref from an earlier lease is simply unknown.

use std::collections::VecDeque;

use niri_ipc::LogicalOutput;
use tokio::time::Instant;

use crate::observe::{Rect, Screenshot};

/// How many refs a lease keeps; older ones are dropped first.
const KEPT: usize = 64;

/// One screenshot, as a pointer tool needs it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Shot {
    pub(crate) output: String,
    /// The output as niri described it at capture.
    pub(crate) geometry: LogicalOutput,
    /// The captured rectangle in layout coordinates.
    pub(crate) captured: Rect,
    /// Image pixels per logical pixel.
    pub(crate) scale: f64,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// When the capture started.
    pub(crate) taken: Instant,
    /// The event stream's connection at capture: a disconnect drops every ref.
    pub(crate) connection: u64,
}

impl Shot {
    /// What a pointer tool needs from `shot`, whose capture started at `taken` while the
    /// event stream was on `connection`.
    pub(crate) fn of(shot: &Screenshot, taken: Instant, connection: u64) -> Self {
        let metadata = &shot.metadata;
        Self {
            output: metadata.output.clone(),
            geometry: shot.geometry,
            captured: metadata.captured,
            scale: metadata.scale,
            width: metadata.width,
            height: metadata.height,
            taken,
            connection,
        }
    }
}

/// The refs of the current lease.
#[derive(Debug, Default)]
pub(crate) struct Refs {
    /// Leases seen, counted from 1, and refs issued: ids are never reused.
    leases: u64,
    issued: u64,
    /// The lease the kept refs belong to, while one is held.
    lease: Option<u64>,
    shots: VecDeque<(u64, Shot)>,
}

impl Refs {
    /// A new lease: earlier refs are dropped.
    pub(crate) fn start(&mut self) {
        self.leases += 1;
        self.lease = Some(self.leases);
        self.shots.clear();
    }

    /// The lease is gone, and with it its refs.
    pub(crate) fn end(&mut self) {
        self.lease = None;
        self.shots.clear();
    }

    /// The lease a screenshot starting now would belong to.
    pub(crate) const fn lease(&self) -> Option<u64> {
        self.lease
    }

    /// Keeps `shot` and returns its id, if `lease` is still the current one.
    pub(crate) fn insert(&mut self, lease: u64, shot: Shot) -> Option<String> {
        if self.lease != Some(lease) {
            return None;
        }
        self.issued += 1;
        if self.shots.len() == KEPT {
            self.shots.pop_front();
        }
        self.shots.push_back((self.issued, shot));
        Some(format!("shot-{}", self.issued))
    }
}

#[cfg(test)]
mod tests {
    use niri_ipc::Transform;

    use super::*;

    fn shot() -> Shot {
        Shot {
            output: "winit".to_owned(),
            geometry: LogicalOutput {
                x: 0,
                y: 0,
                width: 960,
                height: 720,
                scale: 1.5,
                transform: Transform::Flipped180,
            },
            captured: Rect {
                x: 0,
                y: 0,
                width: 960,
                height: 720,
            },
            scale: 1.5,
            width: 1440,
            height: 1080,
            taken: Instant::now(),
            connection: 1,
        }
    }

    #[tokio::test]
    async fn refs_belong_to_one_lease_and_ids_are_never_reused() {
        let mut refs = Refs::default();
        assert_eq!(refs.lease(), None);
        assert_eq!(refs.insert(1, shot()), None);
        refs.start();
        let first = refs.lease().unwrap();
        assert_eq!(refs.insert(first, shot()).as_deref(), Some("shot-1"));
        refs.end();
        // A capture that started under the old lease isn't kept.
        assert_eq!(refs.insert(first, shot()), None);
        refs.start();
        let second = refs.lease().unwrap();
        assert_ne!(first, second);
        assert_eq!(refs.insert(first, shot()), None);
        assert_eq!(refs.insert(second, shot()).as_deref(), Some("shot-2"));
        assert_eq!(refs.shots.len(), 1);
        for _ in 0..KEPT {
            refs.insert(second, shot());
        }
        assert_eq!(refs.shots.len(), KEPT);
        assert_eq!(refs.shots.front().map(|(id, _)| *id), Some(3));
    }
}
