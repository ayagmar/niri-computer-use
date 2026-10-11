//! Named hold points. Each reply a fake gives passes one, named after the fake and the
//! request, and for the mock application the object too: `niri:Version`,
//! `niri:Action:FocusWindow`, `noctalia:status`, `atspi:GetRole:/button`. The points passed
//! since `start` are the trace of the calls in between, in order, so a test can find every
//! reply a call awaited instead of listing them, and hold any one of them back until it
//! releases it. Every fake started on a fixture passes the fixture's points, and a fake
//! program can too: `loginctl` or `wtype` as the program's name.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, oneshot};

use crate::client::WAIT;

#[derive(Debug, Clone, Default)]
pub(crate) struct Points(Arc<Mutex<State>>);

#[derive(Debug, Default)]
struct State {
    /// The points passed since `start`, in order.
    trace: Vec<Passed>,
    /// How many times each point was passed since `start`.
    counts: BTreeMap<String, usize>,
    /// The holds not reached yet, by point and occurrence.
    holds: BTreeMap<(String, usize), Gate>,
    /// How many fakes passed a point and haven't finished that reply yet.
    busy: usize,
}

/// One point passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Passed {
    pub(crate) point: String,
    /// Which time the point was passed since `start`, from 1.
    pub(crate) nth: usize,
    pub(crate) at: Instant,
}

/// A hold not reached yet.
#[derive(Debug)]
struct Gate {
    arrived: Arc<Notify>,
    released: oneshot::Receiver<()>,
}

/// A fake that passed a point, until it finished that reply and what follows it.
#[must_use = "the fake is busy until this drops"]
#[derive(Debug)]
pub(crate) struct Passing(Points);

impl Drop for Passing {
    fn drop(&mut self) {
        self.0.state().busy -= 1;
    }
}

/// A held reply, which waits until the test releases it or drops this.
#[derive(Debug)]
pub(crate) struct Held {
    arrived: Arc<Notify>,
    release: oneshot::Sender<()>,
}

impl Held {
    /// Waits, within `WAIT`, until the reply is held.
    pub(crate) async fn arrived(&self) {
        assert!(
            self.arrived_within(WAIT).await,
            "the held reply was never asked for"
        );
    }

    /// Whether the reply is held within `limit`.
    pub(crate) async fn arrived_within(&self, limit: Duration) -> bool {
        tokio::time::timeout(limit, self.arrived.notified())
            .await
            .is_ok()
    }

    pub(crate) fn release(self) {
        self.release.send(()).ok();
    }
}

impl Points {
    /// Starts a new trace: forgets the points passed and the holds not reached.
    pub(crate) fn start(&self) {
        let mut state = self.state();
        state.trace.clear();
        state.counts.clear();
        state.holds.clear();
    }

    /// The points passed since `start`.
    pub(crate) fn trace(&self) -> Vec<Passed> {
        self.state().trace.clone()
    }

    /// Holds the reply at `point` the `nth` time it is passed since `start`.
    pub(crate) fn hold(&self, point: &str, nth: usize) -> Held {
        let arrived = Arc::new(Notify::new());
        let (release, released) = oneshot::channel();
        let gate = Gate {
            arrived: Arc::clone(&arrived),
            released,
        };
        self.state().holds.insert((point.to_owned(), nth), gate);
        Held { arrived, release }
    }

    /// Holds the reply at `point` the next time it is passed.
    pub(crate) fn hold_next(&self, point: &str) -> Held {
        let nth = self.state().counts.get(point).copied().unwrap_or(0) + 1;
        self.hold(point, nth)
    }

    /// Passes `point`, waiting there while a hold on it says so. The fake is busy with the
    /// reply until the returned guard drops.
    pub(crate) async fn pass(&self, point: String) -> Passing {
        let gate = {
            let mut state = self.state();
            let count = state.counts.entry(point.clone()).or_default();
            *count += 1;
            let nth = *count;
            let gate = state.holds.remove(&(point.clone(), nth));
            state.busy += 1;
            state.trace.push(Passed {
                point,
                nth,
                at: Instant::now(),
            });
            gate
        };
        let passing = Passing(self.clone());
        if let Some(gate) = gate {
            gate.arrived.notify_one();
            gate.released.await.ok();
        }
        passing
    }

    /// Returns once every fake that passed a point finished that reply and what follows
    /// it, such as niri's events after an action.
    pub(crate) async fn idle(&self) {
        let start = Instant::now();
        while self.state().busy > 0 {
            assert!(start.elapsed() < WAIT, "a fake is still busy with a reply");
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Passes the points fake programs reach in `dir`: a program writes
    /// `<point>.<pid>.<n>.arrived` there and waits for `.go`, written once the point is
    /// passed. The program is busy until its process has exited. Ends with `dir`.
    pub(crate) fn serve_programs(&self, dir: PathBuf) {
        tokio::spawn(self.clone().watch_programs(dir));
    }

    async fn watch_programs(self, dir: PathBuf) {
        let mut seen = BTreeSet::new();
        while let Ok(entries) = std::fs::read_dir(&dir) {
            let arrived = entries
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().into_string().ok()?;
                    name.strip_suffix(".arrived").map(str::to_owned)
                })
                .filter(|mark| seen.insert(mark.clone()));
            for mark in arrived {
                tokio::spawn(self.clone().pass_program(dir.clone(), mark));
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    /// Passes the point a program's `mark`, `<point>.<pid>.<n>`, names, lets the program
    /// go on, and stays busy until it has exited, within `WAIT`.
    async fn pass_program(self, dir: PathBuf, mark: String) {
        let mut parts = mark.split('.');
        let (Some(point), Some(pid)) = (parts.next(), parts.next()) else {
            return;
        };
        let passing = self.pass(point.to_owned()).await;
        std::fs::write(dir.join(format!("{mark}.go")), "").ok();
        let process = PathBuf::from(format!("/proc/{pid}"));
        let start = Instant::now();
        while process.exists() && start.elapsed() < WAIT {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        drop(passing);
    }

    /// A test that panicked while holding the lock leaves a sound trace.
    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
