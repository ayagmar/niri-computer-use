//! The everyday workflows: fill in a form, arrange windows, move files in Files, and get
//! past a modal that blocks the task. Each is graded from what the fixture recorded, the
//! files in Files' home or niri's final layout.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use niri_ipc::{Request, Response, Window, Workspace};
use serde_json::Value;

use super::grade::Expectation;
use super::{FIXTURE_DEADLINE, MESSAGE, Setup, start_fixture, windows};
use crate::a11y::Bus;
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::field;
use crate::session::Session;

/// How long an app gets to show its window and join the accessibility bus.
const APP_READY: Duration = Duration::from_secs(30);

/// What `form` asks the agent to enter, as the fixture records it.
pub(super) const NAME: &str = "Ada Lovelace";
pub(super) const EMAIL: &str = "ada@example.com";
pub(super) const PLAN: &str = "Team";

/// The windows `rearrange` opens: two to put side by side and one to move away.
const SIDE_BY_SIDE: [&str; 2] = ["notes", "calendar"];
const MOVED: &str = "music";
/// The workspace `rearrange` asks for, by its index on the output.
const TARGET_WORKSPACE: u8 = 2;

/// The folder `copy-files` asks for, and what `Pictures` holds before it.
const FOLDER: &str = "release-0.1";
const SCREENSHOTS: [&str; 2] = [
    "Screenshot From 2026-10-14 09-12-03.png",
    "Screenshot From 2026-10-14 09-15-47.png",
];
const OTHER_PICTURE: &str = "holiday.png";

/// The GTK 4 fixture in `mode` (`signup` or `draft`), on the accessibility bus.
pub(super) fn start_gtk(session: &mut Session<'_>, setup: &mut Setup, mode: &str) -> Result<u64> {
    let root = session.test_dir().root();
    let file = root.join("workflow.py");
    fs::write(&file, include_str!("../../fixtures/workflow.py"))
        .context(format!("write {}", file.display()))?;
    let seconds = FIXTURE_DEADLINE.as_secs().to_string();
    let args: Vec<OsString> = vec![
        "-I".into(),
        file.into(),
        root.into(),
        mode.into(),
        seconds.into(),
    ];
    let log = session.artifact(&format!("{mode}.log"));
    let process = session.start("python3", &args, log, FIXTURE_DEADLINE)?;
    let pid = process.pid();
    setup.processes.push(process);
    app_window(session, setup, pid)
}

/// Files, showing `Pictures` in a home of its own under `TEST_DIR`, with two screenshots
/// and another picture: real captures of the nested desktop.
pub(super) fn start_files(session: &mut Session<'_>, setup: &mut Setup) -> Result<u64> {
    let pictures = files_home(session).join("Pictures");
    fs::create_dir_all(&pictures).context(format!("create {}", pictures.display()))?;
    let regions = ["0,0 320x200", "0,0 480x300", "0,0 200x200"];
    for (name, region) in SCREENSHOTS.iter().chain([&OTHER_PICTURE]).zip(regions) {
        let args: Vec<OsString> = vec!["-g".into(), region.into(), pictures.join(name).into()];
        session.run("grim", &args)?;
    }
    let mut home = OsString::from("HOME=");
    home.push(files_home(session));
    let args: Vec<OsString> = vec![home, "nautilus".into(), pictures.into()];
    let process = session.start(
        "env",
        &args,
        session.artifact("files.log"),
        FIXTURE_DEADLINE,
    )?;
    let pid = process.pid();
    setup.processes.push(process);
    app_window(session, setup, pid)
}

/// Files' home, which nothing else uses.
fn files_home(session: &Session<'_>) -> PathBuf {
    session.test_dir().root().join("files-home")
}

/// The window of the app with `pid`, once it is up and on the accessibility bus.
fn app_window(session: &mut Session<'_>, setup: &Setup, pid: i32) -> Result<u64> {
    let id = session.wait_until("eval-app", "the app's window", APP_READY, |session| {
        Ok(windows(session)?
            .into_iter()
            .find(|window| window.pid == Some(pid))
            .map(|window| window.id))
    })?;
    if let Some(bus) = &setup.bus {
        session.wait_until("eval-a11y", "the app on the bus", APP_READY, |session| {
            Ok(bus.registered(session, pid)?.then_some(()))
        })?;
    }
    Ok(id)
}

/// The accessibility bus the GTK apps and the server use, as on the user's desktop.
pub(super) fn start_bus(session: &mut Session<'_>, setup: &mut Setup) -> Result<()> {
    setup.bus = Some(Bus::start_for(session, FIXTURE_DEADLINE)?);
    Ok(())
}

/// Three plain windows that take the size niri gives them, the first one focused.
pub(super) fn start_rearrange(session: &mut Session<'_>, setup: &mut Setup) -> Result<()> {
    for app_id in SIDE_BY_SIDE.iter().chain([&MOVED]) {
        start_fixture(session, setup, app_id, &["--resize"])?;
    }
    setup.users_window = Some(super::focus(session, SIDE_BY_SIDE[0])?);
    Ok(())
}

/// Lines of the JSON Lines file `name` in `TEST_DIR`, none if the fixture never wrote it.
pub(super) fn records(session: &Session<'_>, name: &str) -> Result<Vec<Value>> {
    let path = session.test_dir().root().join(name);
    let text = fs::read_to_string(&path).unwrap_or_default();
    fs::write(session.artifact(name), &text).context(format!("copy {name}"))?;
    text.lines()
        .map(|line| serde_json::from_str(line).context(format!("parse {}", path.display())))
        .collect()
}

/// One submission with each field as asked.
pub(super) fn signed_up(signups: &[Value]) -> Vec<Expectation> {
    let mut expectations = vec![Expectation {
        text: "Submitted the form exactly once",
        passed: signups.len() == 1,
        evidence: format!("{} submissions", signups.len()),
    }];
    let last = signups.last().unwrap_or(&Value::Null);
    let wanted = [
        (
            "The submission had the requested name",
            "/name",
            Value::from(NAME),
        ),
        (
            "The submission had the requested email",
            "/email",
            Value::from(EMAIL),
        ),
        (
            "The submission asked for the newsletter",
            "/newsletter",
            Value::from(true),
        ),
        (
            "The submission had the requested plan",
            "/plan",
            Value::from(PLAN),
        ),
    ];
    for (text, pointer, value) in wanted {
        let got = field(last, pointer);
        expectations.push(Expectation {
            text,
            passed: *got == value,
            evidence: format!("submitted {got}, wanted {value}"),
        });
    }
    expectations
}

/// The autosave dialog came up, was dismissed, and the whole message went out once.
pub(super) fn recovered(events: &[Value], sent: &[Value]) -> Vec<Expectation> {
    let events: Vec<&str> = events
        .iter()
        .filter_map(|event| field(event, "/event").as_str())
        .filter(|event| ["shown", "dismissed"].contains(event))
        .collect();
    let texts: Vec<&str> = sent
        .iter()
        .filter_map(|record| field(record, "/text").as_str())
        .collect();
    vec![
        Expectation {
            text: "Dismissed the autosave dialog that blocked the form",
            passed: events == ["shown", "dismissed"],
            evidence: format!("dialog events: {events:?}"),
        },
        Expectation {
            text: "Sent exactly the message, once",
            passed: texts == [MESSAGE],
            evidence: format!("sent {texts:?}"),
        },
    ]
}

/// Where one window ended up.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Placed {
    pub(super) app_id: String,
    pub(super) output: Option<String>,
    pub(super) workspace: Option<u8>,
    /// The column of a tiled window; none for a floating one.
    pub(super) column: Option<usize>,
    /// The tile's width. niri gives no position for a tiled window, so whether it is in
    /// view can't be read.
    pub(super) width: f64,
    /// Its output's logical width.
    pub(super) output_width: Option<f64>,
}

/// Every window niri has, with its workspace's index and output; niri's answers go to
/// `layout.json`.
pub(super) fn placements(session: &mut Session<'_>) -> Result<Vec<Placed>> {
    let Response::Workspaces(workspaces) = session.request(&Request::Workspaces)? else {
        return Err(Failure::new(
            "niri answered Workspaces with another response",
        ));
    };
    let Response::Outputs(outputs) = session.request(&Request::Outputs)? else {
        return Err(Failure::new("niri answered Outputs with another response"));
    };
    let windows = windows(session)?;
    let layout =
        serde_json::json!({"windows": windows, "workspaces": workspaces, "outputs": outputs});
    fs::write(session.artifact("layout.json"), layout.to_string()).context("write layout.json")?;
    let placed = windows
        .into_iter()
        .map(|window| {
            let mut placed = place(&window, &workspaces);
            placed.output_width = placed
                .output
                .as_ref()
                .and_then(|name| outputs.get(name)?.logical)
                .map(|logical| f64::from(logical.width));
            placed
        })
        .collect();
    Ok(placed)
}

fn place(window: &Window, workspaces: &[Workspace]) -> Placed {
    let workspace = workspaces
        .iter()
        .find(|workspace| Some(workspace.id) == window.workspace_id);
    Placed {
        app_id: window.app_id.clone().unwrap_or_default(),
        output_width: None,
        output: workspace.and_then(|workspace| workspace.output.clone()),
        workspace: workspace.map(|workspace| workspace.idx),
        column: window
            .layout
            .pos_in_scrolling_layout
            .map(|(column, _)| column),
        width: window.layout.tile_size.0,
    }
}

/// `notes` and `calendar` in neighbouring columns of one workspace, each about half the
/// output wide and wholly in view; `music` alone on workspace 2 of the same output; and
/// no window opened or closed.
pub(super) fn arranged(placed: &[Placed]) -> Vec<Expectation> {
    let find = |app_id: &str| placed.iter().find(|window| window.app_id == app_id);
    let pair = SIDE_BY_SIDE.map(find);
    let music = find(MOVED);
    let [Some(left), Some(right)] = pair else {
        return vec![missing(&format!("{SIDE_BY_SIDE:?}"))];
    };
    let Some(music) = music else {
        return vec![missing(MOVED)];
    };
    vec![
        neighbours(left, right),
        halves(left, right),
        moved(music, left),
        unchanged(placed),
    ]
}

fn missing(what: &str) -> Expectation {
    Expectation {
        text: "Kept every window open",
        passed: false,
        evidence: format!("no window {what} at the end"),
    }
}

fn neighbours(left: &Placed, right: &Placed) -> Expectation {
    let columns = (left.column, right.column);
    let adjacent = matches!(columns, (Some(a), Some(b)) if a.abs_diff(b) == 1);
    let together = left.workspace.is_some()
        && left.workspace == right.workspace
        && left.output == right.output;
    Expectation {
        text: "Put notes and calendar in neighbouring columns of one workspace",
        passed: adjacent && together,
        evidence: format!(
            "columns {columns:?}, workspaces {:?} and {:?}",
            left.workspace, right.workspace
        ),
    }
}

/// Each tile takes 40 to 60 % of its output's width, so the two fit on it together.
fn halves(left: &Placed, right: &Placed) -> Expectation {
    let share_of = |window: &Placed| Some(window.width / window.output_width?);
    let shares = [left, right].map(share_of);
    Expectation {
        text: "Made notes and calendar each about half the screen wide",
        passed: shares
            .iter()
            .all(|share| share.is_some_and(|share| (0.4..=0.6).contains(&share))),
        evidence: format!(
            "notes {:?} of the output's width, calendar {:?}",
            shares[0], shares[1]
        ),
    }
}

fn moved(music: &Placed, left: &Placed) -> Expectation {
    let passed = music.workspace == Some(TARGET_WORKSPACE)
        && music.output == left.output
        && music.workspace != left.workspace;
    Expectation {
        text: "Moved the music window to workspace 2, away from the other two",
        passed,
        evidence: format!(
            "music on workspace {:?} of {:?}; notes on {:?}",
            music.workspace, music.output, left.workspace
        ),
    }
}

fn unchanged(placed: &[Placed]) -> Expectation {
    let ids: Vec<&str> = placed.iter().map(|window| window.app_id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    Expectation {
        text: "Opened and closed no windows",
        passed: sorted == ["calendar", "music", "notes"],
        evidence: format!("windows at the end: {ids:?}"),
    }
}

/// What `copy-files` left in `Pictures`: names and contents, folders by their entries.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Folder {
    pub(super) files: Vec<(String, Vec<u8>)>,
    pub(super) folders: Vec<(String, Self)>,
}

impl Folder {
    pub(super) fn read(path: &Path) -> Result<Self> {
        let mut folder = Self::default();
        for entry in fs::read_dir(path).context(format!("read {}", path.display()))? {
            let entry = entry.context(format!("read {}", path.display()))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().context("read a file type")?.is_dir() {
                folder.folders.push((name, Self::read(&entry.path())?));
            } else {
                let bytes = fs::read(entry.path()).context(format!("read {name}"))?;
                folder.files.push((name, bytes));
            }
        }
        folder.files.sort();
        folder.folders.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(folder)
    }

    fn names(&self) -> Vec<&str> {
        let files = self.files.iter().map(|(name, _)| name.as_str());
        files
            .chain(self.folders.iter().map(|(name, _)| name.as_str()))
            .collect()
    }
}

/// The folder `copy-files` works in.
pub(super) fn pictures(session: &Session<'_>) -> PathBuf {
    files_home(session).join("Pictures")
}

/// The two screenshots, unchanged, alone in the new folder, and nothing else in
/// `Pictures` touched.
pub(super) fn moved_files(before: &Folder, after: &Folder) -> Vec<Expectation> {
    let screenshots: Vec<&(String, Vec<u8>)> = before
        .files
        .iter()
        .filter(|(name, _)| SCREENSHOTS.contains(&name.as_str()))
        .collect();
    let folder = after.folders.iter().find(|(name, _)| name == FOLDER);
    let inside = folder.map(|(_, folder)| folder);
    let moved = inside.is_some_and(|inside| {
        inside.folders.is_empty() && inside.files.iter().collect::<Vec<_>>() == screenshots
    });
    let kept: Vec<&(String, Vec<u8>)> = before
        .files
        .iter()
        .filter(|(name, _)| !SCREENSHOTS.contains(&name.as_str()))
        .collect();
    let rest = folder.is_some()
        && after.folders.len() == 1
        && after.files.iter().collect::<Vec<_>>() == kept;
    vec![
        Expectation {
            text: "Made the folder and moved both screenshots into it unchanged, and nothing else",
            passed: moved,
            evidence: format!(
                "{FOLDER} holds {:?}",
                inside.map(Folder::names).unwrap_or_default()
            ),
        },
        Expectation {
            text: "Left the rest of Pictures as it was",
            passed: rest,
            evidence: format!("Pictures holds {:?}", after.names()),
        },
    ]
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn passed(expectations: &[Expectation]) -> Vec<bool> {
        expectations.iter().map(|e| e.passed).collect()
    }

    #[test]
    fn only_one_submission_with_every_field_as_asked_signs_up() {
        let right = json!({"name": NAME, "email": EMAIL, "newsletter": true, "plan": PLAN});
        assert!(
            signed_up(std::slice::from_ref(&right))
                .iter()
                .all(|e| e.passed)
        );
        // The plan was left at its default.
        let free = json!({"name": NAME, "email": EMAIL, "newsletter": true, "plan": "Free"});
        assert_eq!(passed(&signed_up(&[free])), [true, true, true, true, false]);
        // Submitted before the check box, then again.
        let early = json!({"name": NAME, "email": EMAIL, "newsletter": false, "plan": PLAN});
        assert!(!signed_up(&[early, right])[0].passed);
        assert!(!signed_up(&[]).iter().any(|e| e.passed));
    }

    #[test]
    fn a_message_cut_by_the_dialog_or_sent_twice_is_not_recovered() {
        let events = [
            json!({"event": "text", "text": "M"}),
            json!({"event": "shown"}),
            json!({"event": "dialog-key", "key": "e"}),
            json!({"event": "dismissed", "how": "ok"}),
        ];
        let whole = [json!({"text": MESSAGE})];
        assert_eq!(passed(&recovered(&events, &whole)), [true, true]);
        let cut = [json!({"text": "Mting moved to 3pm"})];
        assert_eq!(passed(&recovered(&events, &cut)), [true, false]);
        let twice = [json!({"text": "M"}), json!({"text": MESSAGE})];
        assert_eq!(passed(&recovered(&events, &twice)), [true, false]);
        // Still up at the end: the message can't have gone out.
        assert_eq!(passed(&recovered(&events[..3], &[])), [false, false]);
    }

    fn placed(app_id: &str, workspace: u8, column: Option<usize>, width: f64) -> Placed {
        Placed {
            app_id: app_id.to_owned(),
            output: Some("winit".to_owned()),
            workspace: Some(workspace),
            column,
            width,
            output_width: Some(1280.0),
        }
    }

    #[test]
    fn two_halves_side_by_side_and_one_window_moved_is_arranged() {
        let done = [
            placed("notes", 1, Some(1), 616.0),
            placed("calendar", 1, Some(2), 616.0),
            placed("music", 2, Some(1), 616.0),
        ];
        assert_eq!(passed(&arranged(&done)), [true; 4]);
    }

    #[test]
    fn stacked_or_off_screen_or_unmoved_windows_are_not_arranged() {
        // Stacked in one column.
        let stacked = [
            placed("notes", 1, Some(1), 616.0),
            placed("calendar", 1, Some(1), 616.0),
            placed("music", 2, Some(1), 616.0),
        ];
        assert!(!arranged(&stacked)[0].passed);
        // Left at their default widths, too wide to fit side by side.
        let scrolled = [
            placed("notes", 1, Some(1), 840.0),
            placed("calendar", 1, Some(2), 840.0),
            placed("music", 2, Some(1), 840.0),
        ];
        assert_eq!(passed(&arranged(&scrolled)), [true, false, true, true]);
        // Music went along with them.
        let together = [
            placed("notes", 2, Some(1), 616.0),
            placed("calendar", 2, Some(2), 616.0),
            placed("music", 2, Some(3), 616.0),
        ];
        assert!(!arranged(&together)[2].passed);
        // Closed instead of moved.
        assert_eq!(passed(&arranged(&stacked[..2])), [false]);
    }

    fn folder(files: &[(&str, &[u8])], folders: Vec<(String, Folder)>) -> Folder {
        Folder {
            files: files
                .iter()
                .map(|(name, bytes)| ((*name).to_owned(), bytes.to_vec()))
                .collect(),
            folders,
        }
    }

    #[test]
    fn only_both_screenshots_moved_whole_into_the_new_folder_counts() {
        let [one, two] = SCREENSHOTS;
        let before = folder(&[(one, b"1"), (two, b"2"), (OTHER_PICTURE, b"h")], vec![]);
        let into = |files: &[(&str, &[u8])]| vec![(FOLDER.to_owned(), folder(files, vec![]))];
        let done = folder(&[(OTHER_PICTURE, b"h")], into(&[(one, b"1"), (two, b"2")]));
        assert_eq!(passed(&moved_files(&before, &done)), [true, true]);
        // Copied, not moved.
        let copied = folder(
            &[(one, b"1"), (two, b"2"), (OTHER_PICTURE, b"h")],
            into(&[(one, b"1"), (two, b"2")]),
        );
        assert_eq!(passed(&moved_files(&before, &copied)), [true, false]);
        // One screenshot left behind.
        let half = folder(&[(two, b"2"), (OTHER_PICTURE, b"h")], into(&[(one, b"1")]));
        assert_eq!(passed(&moved_files(&before, &half)), [false, false]);
        // The other picture went along.
        let all = folder(
            &[],
            into(&[(one, b"1"), (two, b"2"), (OTHER_PICTURE, b"h")]),
        );
        assert_eq!(passed(&moved_files(&before, &all)), [false, false]);
        // Moved into a folder with another name.
        let renamed = folder(
            &[(OTHER_PICTURE, b"h")],
            vec![(
                "release 0.1".to_owned(),
                folder(&[(one, b"1"), (two, b"2")], vec![]),
            )],
        );
        assert_eq!(passed(&moved_files(&before, &renamed)), [false, false]);
    }
}
