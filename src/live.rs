//! The action in progress, for the Noctalia plugin's overlay: one JSON record in the
//! instance's runtime directory, `live.json`, that each phase of an action replaces whole.
//! `start` is written once the desk admits the action, `aim` once it knows where it acts
//! and before its input goes out, `look` right before its evidence is captured, and `end`
//! with its outcome. Each record holds everything known so far, so a reader that sees only
//! one phase misses nothing before it. `watch` forwards the record as a `live` line.
//!
//! The record holds the audit log's metadata of the call's arguments, the output and the
//! output-local logical points or box it acts on, and for a window the id and the `app_id`
//! niri reports for it: never typed text, a title, an element's name or the agent's own
//! `expect.app_id`. Writing it never fails or delays the call: the first failure goes to
//! stderr and the rest are dropped.

use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use niri_ipc::{LogicalOutput, Output, Window, Workspace};
use serde::Serialize;
use serde_json::{Value, json};

use crate::a11y::model;
use crate::audit::Summary;
use crate::coords::{self, LayoutPt, OutputLocalPt, OutputLocalRect};

/// The record's file in the runtime directory.
pub(crate) const FILE: &str = "live.json";

/// The longest record written whole: `watch` skips a longer one, so past it the
/// arguments are replaced by their size.
pub(crate) const LIMIT: usize = 4096;

/// How far an action has got. Only ever moves forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    Start,
    Aim,
    Look,
    End,
}

/// One action as the overlay sees it.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Record {
    /// The process that writes it, the engine or a standalone server.
    pid: u32,
    /// Counts up from 1 in that process: with `pid`, it names the call.
    call: u64,
    /// As in the lease record and the audit log, such as `claude-code/4711`.
    session: String,
    tool: String,
    /// The call's start, as in its audit line.
    ts: String,
    phase: Phase,
    /// The audit log's metadata of the call's arguments.
    args: Value,
    aim: Option<Aim>,
    window: Option<WindowRef>,
    outcome: Option<Summary>,
}

/// Where an action acts: on which output, how large that output is, and the points or the
/// box on it, all in the output's logical pixels as displayed.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct Aim {
    output: String,
    area: Area,
    /// A pointer gesture's points: one, or a drag's start and end.
    points: Vec<OutputLocalPt>,
    /// A floating window's tile.
    #[serde(rename = "box")]
    rect: Option<OutputLocalRect>,
}

/// An output's logical size as displayed, already turned for a quarter-turned output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct Area {
    width: u32,
    height: u32,
}

impl Area {
    const fn of(output: &LogicalOutput) -> Self {
        Self {
            width: output.width,
            height: output.height,
        }
    }
}

/// The window an action is about, as niri reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WindowRef {
    id: u64,
    app_id: Option<String>,
}

impl WindowRef {
    pub(crate) fn of(window: &Window) -> Self {
        Self {
            id: window.id,
            app_id: window.app_id.clone(),
        }
    }
}

/// Hundredths of a logical pixel are more than the overlay can show, and keep the
/// record's numbers short.
fn rounded(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

impl Aim {
    /// A pointer gesture's `points` in the layout, on the output `name` whose layout area is
    /// `output`. A drag's steps are given as its start and end.
    pub(crate) fn points(name: &str, output: &LogicalOutput, layout: &[LayoutPt]) -> Self {
        let ends = match layout {
            [] => Vec::new(),
            [one] => vec![*one],
            [first, .., last] => vec![*first, *last],
        };
        let points = ends
            .into_iter()
            .map(|point| {
                let local = coords::layout_to_output(point, output);
                OutputLocalPt {
                    x: rounded(local.x),
                    y: rounded(local.y),
                }
            })
            .collect();
        Self {
            output: name.to_owned(),
            area: Area::of(output),
            points,
            rect: None,
        }
    }
}

/// Where `window` is: the output of its workspace and that output's size, and its tile
/// when it floats on its output's active workspace. niri 26.04 gives a tile in the
/// scrolling layout no place, and a hidden workspace's windows aren't where it says.
/// `None` when it is on no output.
pub(crate) fn window_aim(
    window: &Window,
    workspaces: &HashMap<u64, Workspace>,
    outputs: &BTreeMap<String, Output>,
) -> Option<Aim> {
    let workspace = workspaces.get(&window.workspace_id?)?;
    let name = workspace.output.as_ref()?;
    let area = outputs.get(name)?.logical?;
    let rect = workspace
        .is_active
        .then(|| model::floating_tile(&area, window))
        .flatten()
        .map(|tile| OutputLocalRect {
            x: rounded(tile.x - f64::from(area.x)),
            y: rounded(tile.y - f64::from(area.y)),
            width: rounded(tile.width),
            height: rounded(tile.height),
        });
    Some(Aim {
        output: name.clone(),
        area: Area::of(&area),
        points: Vec::new(),
        rect,
    })
}

/// A call that may act, before its record is written: `Live::start` writes it.
#[derive(Debug, Clone)]
pub(crate) struct Cue {
    call: u64,
    session: String,
    tool: String,
    ts: String,
    args: Value,
}

/// The writer of one process's records, shared by its sessions. The action mutex lets
/// one action run at a time, so the latest record is the running action's.
#[derive(Debug)]
pub(crate) struct Live {
    /// `live.json`, none without a runtime directory.
    path: Option<PathBuf>,
    calls: AtomicU64,
    latest: Mutex<Option<Record>>,
    warned: AtomicBool,
}

impl Live {
    /// A writer of `path`, or of nothing.
    pub(crate) const fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            calls: AtomicU64::new(0),
            latest: Mutex::new(None),
            warned: AtomicBool::new(false),
        }
    }

    /// The next call's cue: it is `session`'s `tool` started at `ts`, with the audit
    /// log's metadata `args`.
    pub(crate) fn cue(&self, session: String, tool: &str, ts: &str, args: Value) -> Cue {
        Cue {
            call: self.calls.fetch_add(1, Ordering::Relaxed) + 1,
            session,
            tool: tool.to_owned(),
            ts: ts.to_owned(),
            args,
        }
    }

    /// The call the desk admitted begins.
    pub(crate) fn start(&self, cue: &Cue) {
        let record = Record {
            pid: std::process::id(),
            call: cue.call,
            session: cue.session.clone(),
            tool: cue.tool.clone(),
            ts: cue.ts.clone(),
            phase: Phase::Start,
            args: cue.args.clone(),
            aim: None,
            window: None,
            outcome: None,
        };
        let mut latest = self.latest();
        self.write(&record);
        *latest = Some(record);
    }

    /// The running action acts at `aim`, on `window`.
    pub(crate) fn aim(&self, aim: Option<Aim>, window: Option<WindowRef>) {
        self.advance(None, Phase::Aim, |record| {
            record.aim = aim;
            record.window = window;
        });
    }

    /// The running action's evidence is about to be captured: from now until its end, the
    /// overlay changes nothing on screen.
    pub(crate) fn look(&self) {
        self.advance(None, Phase::Look, |_| {});
    }

    /// `cue`'s call ended with `outcome`. Nothing is written once another call began.
    pub(crate) fn end(&self, cue: &Cue, outcome: Summary) {
        self.advance(Some(cue.call), Phase::End, |record| {
            record.outcome = Some(outcome);
        });
    }

    /// Moves the latest record, when it is `call`'s or any call's, on to `phase` if it is
    /// still before it, and writes it.
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the lock is held across the write, so records reach the file in the order they were made"
    )]
    fn advance(&self, call: Option<u64>, phase: Phase, change: impl FnOnce(&mut Record)) {
        let mut latest = self.latest();
        let Some(record) = latest.as_mut() else {
            return;
        };
        if record.phase >= phase || call.is_some_and(|call| call != record.call) {
            return;
        }
        record.phase = phase;
        change(record);
        self.write(record);
    }

    fn latest(&self) -> std::sync::MutexGuard<'_, Option<Record>> {
        self.latest.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self, record: &Record) {
        let Some(path) = &self.path else {
            return;
        };
        if let Err(error) = replace(path, &encoded(record))
            && !self.warned.swap(true, Ordering::Relaxed)
        {
            crate::cli::print_error(&format!(
                "the overlay's record {}: {error}; later failures aren't reported",
                path.display()
            ));
        }
    }
}

/// `record` as one line, with its arguments as their size when they would take it past
/// `LIMIT`.
fn encoded(record: &Record) -> Vec<u8> {
    let line = serde_json::to_vec(record).unwrap_or_default();
    if line.len() <= LIMIT {
        return line;
    }
    let bytes = serde_json::to_vec(&record.args).map_or(0, |args| args.len());
    let smaller = Record {
        args: json!({ "bytes": bytes }),
        ..record.clone()
    };
    serde_json::to_vec(&smaller).unwrap_or_default()
}

/// Replaces `path` with `bytes` through a file of this process's beside it, so a reader
/// sees the old record or the new one, whole.
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut written = path.as_os_str().to_owned();
    written.push(format!(".{}.tmp", std::process::id()));
    let written = PathBuf::from(written);
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&written)?;
    file.write_all(bytes)?;
    std::fs::rename(&written, path).inspect_err(|_| {
        std::fs::remove_file(&written).ok();
    })
}

#[cfg(test)]
mod tests {
    use niri_ipc::{Transform, WindowLayout};

    use super::*;
    use crate::coords::{Capture, ImagePx};

    /// The lines the plugin's tests read too, so a change to the format on either side
    /// shows on both.
    const FIXTURE: &str = include_str!("../noctalia/niri-computer-use/tests/fixtures/live.jsonl");

    fn fixture(name: &str) -> Value {
        FIXTURE
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .find(|line| line["_case"] == name)
            .map_or_else(
                || Value::String(format!("no case {name}")),
                |mut line| {
                    line.as_object_mut().unwrap().remove("_case");
                    line
                },
            )
    }

    fn written(path: &Path) -> Value {
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    /// The record with this process's PID, as the fixture has `1`.
    fn as_fixture(mut record: Value) -> Value {
        record["pid"] = json!(1);
        record
    }

    /// An output at scale 1.25, left of the primary one.
    const LEFT: LogicalOutput = LogicalOutput {
        x: -2048,
        y: 0,
        width: 2048,
        height: 1152,
        scale: 1.25,
        transform: Transform::Normal,
    };

    fn window(id: u64, workspace: Option<u64>, floating: Option<(f64, f64)>) -> Window {
        Window {
            id,
            title: Some("a title".to_owned()),
            app_id: Some("kitty".to_owned()),
            pid: None,
            workspace_id: workspace,
            is_focused: true,
            is_floating: floating.is_some(),
            is_urgent: false,
            layout: WindowLayout {
                pos_in_scrolling_layout: floating.is_none().then_some((1, 1)),
                tile_size: (800.0, 600.0),
                window_size: (800, 600),
                tile_pos_in_workspace_view: floating,
                window_offset_in_tile: (0.0, 0.0),
            },
            focus_timestamp: None,
        }
    }

    fn workspace(id: u64, active: bool) -> Workspace {
        Workspace {
            id,
            idx: 1,
            name: None,
            output: Some("HDMI-A-1".to_owned()),
            is_urgent: false,
            is_active: active,
            is_focused: active,
            active_window_id: None,
        }
    }

    fn outputs() -> BTreeMap<String, Output> {
        let output = Output {
            name: "HDMI-A-1".to_owned(),
            logical: Some(LEFT),
            ..crate::test_support::wayland::output()
        };
        BTreeMap::from([(output.name.clone(), output)])
    }

    fn live(name: &str) -> (PathBuf, Live) {
        let dir = crate::test_support::fresh_dir(name);
        let live = Live::new(Some(dir.join(FILE)));
        (dir, live)
    }

    // The bug: a point on an output left of the primary one, at a fractional scale, put
    // the robot off by the output's origin, or at the image's pixel instead of the point.
    #[test]
    fn a_click_aims_at_its_point_on_its_output() {
        let (dir, live) = live("live-click");
        let capture = Capture {
            origin: LayoutPt { x: -2048.0, y: 0.0 },
            scale: 1.25,
        };
        let point = coords::image_to_layout(ImagePx { x: 812, y: 400 }, capture);
        let args = json!({"screenshot_ref": "shot-1-1", "x": 812, "y": 400});
        let cue = live.cue(
            "claude-code/4711".into(),
            "click",
            "2026-10-30T12:00:00.123Z",
            args,
        );
        live.start(&cue);
        live.aim(Some(Aim::points("HDMI-A-1", &LEFT, &[point])), None);
        assert_eq!(as_fixture(written(&dir.join(FILE))), fixture("click_aim"));
        live.end(
            &cue,
            Summary {
                accepted: Some(true),
                observed: Some("sent".into()),
                error: None,
            },
        );
        assert_eq!(as_fixture(written(&dir.join(FILE))), fixture("click_end"));
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
        assert_eq!(left.len(), 1, "only live.json is left: {left:?}");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_drag_aims_at_its_start_and_end_only() {
        let steps: Vec<LayoutPt> = (0..=10)
            .map(|step| LayoutPt {
                x: 10.0f64.mul_add(f64::from(step), -2048.0),
                y: 5.0,
            })
            .collect();
        let aim = Aim::points("HDMI-A-1", &LEFT, &steps);
        assert_eq!(
            aim.points,
            [
                OutputLocalPt { x: 0.0, y: 5.0 },
                OutputLocalPt { x: 100.0, y: 5.0 }
            ]
        );
    }

    // The bug: the robot placed where niri says a window is when niri doesn't place it, or
    // where a hidden workspace's window would be.
    #[test]
    fn a_window_is_placed_only_while_it_floats_on_a_shown_workspace() {
        let workspaces = HashMap::from([(1, workspace(1, true)), (2, workspace(2, false))]);
        let floating = window(7, Some(1), Some((100.0, 50.0)));
        let aim = window_aim(&floating, &workspaces, &outputs()).unwrap();
        assert_eq!((aim.output.as_str(), aim.area.width), ("HDMI-A-1", 2048));
        assert_eq!(
            aim.rect,
            Some(OutputLocalRect {
                x: 100.0,
                y: 50.0,
                width: 800.0,
                height: 600.0
            })
        );
        let tiled = window(7, Some(1), None);
        assert_eq!(
            window_aim(&tiled, &workspaces, &outputs()).unwrap().rect,
            None
        );
        let hidden = window(7, Some(2), Some((100.0, 50.0)));
        assert_eq!(
            window_aim(&hidden, &workspaces, &outputs()).unwrap().rect,
            None
        );
        assert_eq!(
            window_aim(&window(7, None, None), &workspaces, &outputs()),
            None
        );
    }

    #[test]
    fn typing_into_a_tiled_window_names_it_and_its_output() {
        let (dir, live) = live("live-type");
        let workspaces = HashMap::from([(1, workspace(1, true))]);
        let kitty = window(12, Some(1), None);
        let args = json!({"text_len": 24, "expect": {"window_id": 12}});
        let cue = live.cue("pi/9".into(), "type_text", "2026-10-30T12:00:01.000Z", args);
        live.start(&cue);
        live.aim(
            window_aim(&kitty, &workspaces, &outputs()),
            Some(WindowRef::of(&kitty)),
        );
        assert_eq!(as_fixture(written(&dir.join(FILE))), fixture("type_aim"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    // The bug: an end, or an aim, from a call another call already replaced, rewinding
    // what the overlay reads to a call that is over.
    #[test]
    fn phases_only_move_forward_and_a_replaced_call_writes_nothing() {
        let (dir, live) = live("live-phases");
        let first = live.cue("pi/9".into(), "key", "t1", json!({}));
        live.start(&first);
        live.look();
        live.aim(None, None);
        assert_eq!(written(&dir.join(FILE))["phase"], "look");
        let second = live.cue("pi/9".into(), "key", "t2", json!({}));
        live.start(&second);
        live.end(&first, Summary::default());
        let now = written(&dir.join(FILE));
        assert_eq!(
            (now["call"].as_u64(), now["phase"].as_str()),
            (Some(2), Some("start"))
        );
        live.end(&second, Summary::default());
        live.aim(None, None);
        assert_eq!(written(&dir.join(FILE))["phase"], "end");
        std::fs::remove_dir_all(dir).unwrap();
    }

    // The bug: a call with large arguments wrote a record `watch` skips, so the robot
    // never moved for it.
    #[test]
    fn arguments_that_would_pass_the_limit_become_their_size() {
        let (dir, live) = live("live-limit");
        let args = json!({"fields": vec!["x".repeat(64); 100]});
        let cue = live.cue("pi/9".into(), "niri_action", "t", args);
        live.start(&cue);
        let bytes = std::fs::read(dir.join(FILE)).unwrap();
        assert!(bytes.len() <= LIMIT, "{}", bytes.len());
        assert!(written(&dir.join(FILE))["args"]["bytes"].as_u64() > Some(6000));
        std::fs::remove_dir_all(dir).unwrap();
    }

    // The bug: decoration failing or stopping an action when its record can't be written.
    #[test]
    fn a_record_that_cant_be_written_is_dropped_and_reported_once() {
        let dir = crate::test_support::fresh_dir("live-missing");
        let live = Live::new(Some(dir.join("gone").join(FILE)));
        let cue = live.cue("pi/9".into(), "key", "t", json!({}));
        live.start(&cue);
        live.aim(None, None);
        assert!(live.warned.load(Ordering::Relaxed));
        assert!(!dir.join("gone").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
