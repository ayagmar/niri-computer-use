//! Ctrl+C, SIGTERM and SIGHUP for `harness run`. The children run in their own process
//! groups, so the terminal's signals don't reach them; the runner checks this flag while
//! it waits and kills the group itself.

use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};

use crate::failure::{Context as _, Result};

static REQUESTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

pub(crate) fn install() -> Result<()> {
    let flag = REQUESTED.get_or_init(Arc::default);
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        signal_hook::flag::register(signal, Arc::clone(flag))
            .context(format!("handle signal {signal}"))?;
    }
    Ok(())
}

/// Always false in a process that never called `install`, such as the supervisor.
pub(crate) fn requested() -> bool {
    REQUESTED
        .get()
        .is_some_and(|flag| flag.load(Ordering::Relaxed))
}
