//! The round-trip check a desk passes before it is ready: `wl_display.sync` round trips
//! to its niri while niri redraws every frame, which is when a swap interval of 1 makes
//! each request wait for the parent's frame callback. Over `LIMIT` at the 95th percentile,
//! the desk isn't ready and is stopped: no fallback, no retry.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The slowest 95th-percentile round trip a ready desk may have. A 60 Hz parent that
/// paces niri's swaps gives about 16 ms per frame, and two frames at the 95th percentile;
/// unpaced, the spike measured 0.1 to 1.4 ms.
pub(crate) const LIMIT: Duration = Duration::from_millis(20);

/// How many round trips the check measures.
pub(crate) const BURST: usize = 40;

/// A burst of round trips, in microseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RoundTrips {
    pub(crate) count: usize,
    pub(crate) p50_us: u64,
    pub(crate) p95_us: u64,
    pub(crate) max_us: u64,
}

impl RoundTrips {
    /// The summary of `samples`, `None` when there are none.
    pub(crate) fn of(samples: &[Duration]) -> Option<Self> {
        let mut micros: Vec<u64> = samples
            .iter()
            .map(|sample| u64::try_from(sample.as_micros()).unwrap_or(u64::MAX))
            .collect();
        micros.sort_unstable();
        let at = |quantile: usize| {
            let index = (micros.len() * quantile / 100).min(micros.len().checked_sub(1)?);
            micros.get(index).copied()
        };
        Some(Self {
            count: micros.len(),
            p50_us: at(50)?,
            p95_us: at(95)?,
            max_us: *micros.last()?,
        })
    }

    /// Whether a desk with these round trips is ready, or why not.
    pub(crate) fn verdict(&self) -> Result<(), String> {
        let limit = u64::try_from(LIMIT.as_micros()).unwrap_or(u64::MAX);
        if self.p95_us <= limit {
            return Ok(());
        }
        Err(format!(
            "the desk's niri answered {} round trips in {} ms at the 95th percentile (median {} ms, slowest {} ms), over the {} ms a ready desk may take. \
             Its swaps wait for the parent's frames: the GPU driver didn't take the swap interval switches __GL_SYNC_TO_VBLANK=0 (NVIDIA) or vblank_mode=0 (Mesa), \
             or a windowed desk's window is hidden or covered. The desk was stopped",
            self.count,
            millis(self.p95_us),
            millis(self.p50_us),
            millis(self.max_us),
            LIMIT.as_millis()
        ))
    }
}

/// The check's outcome from the burst's measurement: its summary when the desk may be
/// ready, or why it is refused. A measurement that failed, or measured nothing, refuses
/// too: there is no fallback and no second try.
pub(crate) fn judge(measured: Result<Vec<Duration>, String>) -> Result<RoundTrips, String> {
    let samples = measured.map_err(|error| {
        format!(
            "the round-trip check couldn't measure the desk's niri ({error}). The desk was stopped"
        )
    })?;
    let summary = RoundTrips::of(&samples)
        .ok_or("the round-trip check measured nothing. The desk was stopped")?;
    summary.verdict()?;
    Ok(summary)
}

fn millis(micros: u64) -> String {
    format!("{}.{:01}", micros / 1000, micros % 1000 / 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn burst(fast: usize, slow: usize, slow_ms: u64) -> Vec<Duration> {
        let mut samples = vec![Duration::from_micros(150); fast];
        samples.extend(vec![Duration::from_millis(slow_ms); slow]);
        samples
    }

    /// Catches a desk made ready whose niri stalls on its parent's frames, which would
    /// fail every action with `deadline_exceeded` and leave input dirty, and one made ready
    /// when the check couldn't measure at all.
    #[test]
    fn a_desk_whose_swaps_wait_for_the_parent_is_refused_with_its_numbers() {
        // The spike's headless desk with the default swap interval while an app animated:
        // 31 ms at the 95th percentile, two frames at 60 Hz.
        let error = judge(Ok(burst(10, 30, 31))).unwrap_err();
        assert!(error.contains("40 round trips in 31.0 ms"), "{error}");
        assert!(error.contains("__GL_SYNC_TO_VBLANK=0"), "{error}");
        assert!(error.ends_with("The desk was stopped"), "{error}");
        // A hidden window's host sends a frame callback a second.
        assert!(judge(Ok(burst(0, 40, 980))).is_err());
        let unmeasured = judge(Err("no niri".to_owned())).unwrap_err();
        assert!(unmeasured.contains("couldn't measure"), "{unmeasured}");
        assert!(judge(Ok(Vec::new())).is_err());
    }

    #[test]
    fn a_desk_with_a_few_slow_round_trips_is_ready() {
        let fast = judge(Ok(burst(39, 1, 40))).unwrap();
        assert_eq!(fast.p95_us, 150);
        assert_eq!(fast.max_us, 40_000);
        fast.verdict().unwrap();
        let at_limit = RoundTrips::of(&[LIMIT; 3]).unwrap();
        at_limit.verdict().unwrap();
    }

    #[test]
    fn no_samples_is_no_summary() {
        assert_eq!(RoundTrips::of(&[]), None);
    }
}
