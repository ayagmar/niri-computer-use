//! M3's nested acceptance: one `niri-computer-use` server holds the lease on the nested
//! niri, with the nested Noctalia as the lock source, and launches, focuses and closes
//! fixture windows (`harness window`) through launch presets. Covered: `launch` giving
//! `one` with a late `app_id` and `ambiguous`, `reuse` with zero, one and two existing
//! windows, `focus_window` and `focus_workspace`, `close_window` giving `closed` and
//! `pending` with its screenshot, and `interrupted` when the harness moves focus during a
//! launch's wait. `niri_action` fullscreens and restores a window, floats it and sets its
//! width, refuses `Spawn`, and a second server whose policy turns on `unrestricted` spawns
//! a fixture with it. Everything the run creates lives under `TEST_DIR`.

use std::fs;
use std::path::Path;
use std::time::Duration;

use niri_ipc::{Action, Request};
use serde_json::{Value, json};

use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field, structured};
use crate::session::Session;

/// Within what is left of the run's deadline.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(110);
const SERVER_DEADLINE: Duration = Duration::from_secs(100);
const READY: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(5);
/// How long the slow fixture waits before it maps its window.
const SLOW_DELAY: &str = "2500";

pub(crate) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    write_policy(session)?;
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let mut client = Client::start(session, server, "harness-m3", SERVER_DEADLINE)?;
    ready(session, &mut client)?;
    let holder = structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    session.log(&format!(
        "M3: holds the lease as {}",
        field(&holder, "/holder/label")
    ))?;
    saves(session, &mut client)?;
    let late = launches(session, &mut client)?;
    reuses(session, &mut client, late)?;
    focuses(session, &mut client, late)?;
    closes(session, &mut client)?;
    interrupted(session, &mut client, late)?;
    layout(session, &mut client)?;
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    client.stop()?;
    unrestricted(session, server)?;
    noctalia.stop().map(drop)
}

/// The presets, each starting this harness as a fixture window, and a capture directory
/// under `TEST_DIR`.
fn write_policy(session: &Session<'_>) -> Result<()> {
    let started = session.test_dir().root().join("slow-started");
    let started = started.to_string_lossy();
    let shots = session.test_dir().root().join("shots");
    // A JSON string is a valid TOML basic string.
    let capture_dir =
        serde_json::to_string(&shots.to_string_lossy()).context("encode capture_dir")?;
    let presets = presets(
        session,
        &[
            ("late", &["--late", "400"]),
            ("two", &["--count", "2"]),
            ("reuse", &[]),
            ("plain", &[]),
            ("keep", &["--keep-open"]),
            ("slow", &["--delay", SLOW_DELAY, "--started", &started]),
            ("sized", &["--resize"]),
        ],
    )?;
    write_file(
        session,
        &format!("capture_dir = {capture_dir}\n\n{presets}"),
    )
}

/// Writes the policy file with one preset per entry, each starting this harness as a
/// fixture window with that `app_id` and the extra arguments. The server reads the file
/// once, when it starts.
pub(crate) fn write_presets(session: &Session<'_>, presets: &[(&str, &[&str])]) -> Result<()> {
    write_file(session, &self::presets(session, presets)?)
}

/// The policy file's `[[preset]]` tables, one per entry.
fn presets(session: &Session<'_>, presets: &[(&str, &[&str])]) -> Result<String> {
    let harness = std::env::current_exe().context("find the harness binary")?;
    let harness = harness
        .to_str()
        .ok_or_else(|| Failure::new("the harness path isn't UTF-8"))?;
    let test_dir = session.test_dir().root();
    let test_dir = test_dir
        .to_str()
        .ok_or_else(|| Failure::new("TEST_DIR isn't UTF-8"))?;
    let entries = presets
        .iter()
        .map(|(name, extra)| {
            let mut argv = vec![harness, "window", test_dir, name];
            argv.extend_from_slice(extra);
            // JSON strings are valid TOML basic strings.
            let argv = serde_json::to_string(&argv).context("encode a preset")?;
            Ok(format!(
                "[[preset]]\nname = \"{name}\"\nargv = {argv}\napp_id = \"{name}\"\n"
            ))
        })
        .collect::<Result<Vec<String>>>()?;
    Ok(entries.join("\n"))
}

fn write_file(session: &Session<'_>, policy: &str) -> Result<()> {
    let dir = session.test_dir().config().join("niri-computer-use");
    fs::create_dir_all(&dir).context(format!("create {}", dir.display()))?;
    let path = dir.join("policy.toml");
    fs::write(&path, policy).context(format!("write {}", path.display()))?;
    fs::write(session.artifact("policy.toml"), policy).context("copy the policy file")
}

/// Waits until the server sees niri, the nested Noctalia and an unlocked screen.
fn ready(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let status = mcp::ready(session, client, "m3-ready", READY)?;
    session.log(&format!(
        "M3 status: niri {}, lock {}, presets {}",
        field(&status, "/niri/compat"),
        field(&status, "/lock"),
        field(&status, "/policy/preset_names")
    ))
}

/// `save_path` writes a PNG at the output's own scale whatever `max_width` is, and a
/// second save to the same name is refused, leaving the first.
fn saves(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let outputs = structured(&client.call(session, "outputs", json!({}))?)?;
    let logical = field(&outputs, "/winit/logical");
    let expected = (scaled(logical, "width")?, scaled(logical, "height")?);
    let args = json!({"target": "focused_output", "max_width": 320, "save_path": "full.png"});
    let shot = structured(&client.call(session, "screenshot", args.clone())?)?;
    let path = session.test_dir().root().join("shots/full.png");
    let saved = fs::read(&path).context(format!("read {}", path.display()))?;
    let size = crate::image_header::png_size(&saved);
    expect(
        size == Some(expected)
            && field(&shot, "/saved/width") == expected.0
            && field(&shot, "/saved/height") == expected.1
            && field(&shot, "/width") == 320,
        &format!("a saved {expected:?} PNG beside a 320-wide image, the file {size:?}"),
        &shot.to_string(),
    )?;
    let again = client.call(session, "screenshot", args)?;
    expect(
        field(&again, "/isError") == true
            && field(&again, "/content/0/text")
                .as_str()
                .is_some_and(|text| text.contains("already exists"))
            && fs::read(&path).ok().as_ref() == Some(&saved),
        "a second save to the same name is refused",
        &again.to_string(),
    )?;
    session.log(&format!(
        "M3: screenshot saved {expected:?} at {}: {shot}",
        path.display()
    ))
}

/// One side of a logical output in image pixels at its own scale, truncated as grim does.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "matches grim's `int width = logical width × scale`"
)]
fn scaled(logical: &Value, side: &str) -> Result<u32> {
    let length = field(logical, &format!("/{side}")).as_f64();
    let scale = field(logical, "/scale").as_f64();
    match (length, scale) {
        (Some(length), Some(scale)) => Ok((length * scale) as u32),
        _ => Err(Failure::new(format!(
            "M3: winit has no logical {side}: {logical}"
        ))),
    }
}

/// A late `app_id` still counts as `one`; two windows at once are `ambiguous`. Returns
/// the late window's id.
fn launches(session: &mut Session<'_>, client: &mut Client) -> Result<u64> {
    let late = act(session, client, "launch", json!({"preset": "late"}))?;
    expect_outcome(&late, "one", "launch with a late app_id")?;
    let id = only_window(&late)?;
    expect(
        field(&window(session, client, id)?, "/app_id") == "late",
        "the late window's app_id in desktop_state",
        &late.to_string(),
    )?;
    session.log(&format!("M3: launch with a late app_id: {late}"))?;
    let two = act(session, client, "launch", json!({"preset": "two"}))?;
    expect_outcome(&two, "ambiguous", "launch of two windows")?;
    expect(
        field(&two, "/windows").as_array().map(Vec::len) == Some(2),
        "two windows reported",
        &two.to_string(),
    )?;
    session.log(&format!("M3: launch of two windows: {two}"))?;
    Ok(id)
}

/// `reuse` with zero matching windows launches, with one focuses it, and with two starts
/// nothing.
fn reuses(session: &mut Session<'_>, client: &mut Client, other: u64) -> Result<()> {
    let reuse = json!({"preset": "reuse", "reuse": true});
    let first = act(session, client, "launch", reuse.clone())?;
    expect_outcome(&first, "one", "reuse with no window")?;
    let id = only_window(&first)?;
    focus(session, client, other)?;
    let again = act(session, client, "launch", reuse.clone())?;
    expect_outcome(&again, "focused", "reuse with one window")?;
    expect(
        only_window(&again)? == id && field(&again, "/focused_window") == id,
        "reuse focused the existing window",
        &again.to_string(),
    )?;
    expect(
        count(session, client, "reuse")? == 1,
        "reuse with one window started nothing",
        &again.to_string(),
    )?;
    session.log(&format!(
        "M3: reuse with zero, then one window: {first}, {again}"
    ))?;
    let second = act(session, client, "launch", json!({"preset": "reuse"}))?;
    expect_outcome(&second, "one", "a second reuse window")?;
    let both = act(session, client, "launch", reuse)?;
    expect(
        field(&both, "/observed") == "ambiguous"
            && field(&both, "/accepted") == false
            && field(&both, "/windows").as_array().map(Vec::len) == Some(2)
            && count(session, client, "reuse")? == 2,
        "reuse with two windows started nothing",
        &both.to_string(),
    )?;
    session.log(&format!("M3: reuse with two windows: {both}"))
}

/// `focus_window` and `focus_workspace` are seen to take effect, with keyboard focus where
/// niri puts it.
fn focuses(session: &mut Session<'_>, client: &mut Client, id: u64) -> Result<()> {
    focus(session, client, id)?;
    let state = structured(&client.call(session, "desktop_state", json!({}))?)?;
    let workspaces = field(&state, "/workspaces")
        .as_array()
        .cloned()
        .unwrap_or_default();
    let focused = workspaces
        .iter()
        .find(|ws| field(ws, "/is_focused") == true)
        .map(|ws| field(ws, "/id").clone());
    let other = workspaces
        .iter()
        .find(|ws| field(ws, "/is_focused") == false)
        .map(|ws| field(ws, "/id").clone());
    let (Some(focused), Some(other)) = (focused, other) else {
        return Err(Failure::new(format!(
            "M3: need a focused and another workspace, saw {}",
            Value::Array(workspaces)
        )));
    };
    // The fixtures all open on the focused workspace, so the other one is empty.
    let away = act(session, client, "focus_workspace", json!({"id": other}))?;
    expect_outcome(&away, "focused", "focus_workspace")?;
    expect(
        field(&away, "/focused_window").is_null(),
        "no window focused on the empty workspace",
        &away.to_string(),
    )?;
    let back = act(session, client, "focus_workspace", json!({"id": focused}))?;
    expect_outcome(&back, "focused", "focus_workspace back")?;
    expect(
        field(&back, "/focused_window") == id,
        "focus back on the window it left",
        &back.to_string(),
    )?;
    session.log(&format!(
        "M3: focus_workspace {other} and back: {away}, {back}"
    ))
}

/// A window that closes is `closed`; one that ignores the request is `pending`, with a
/// screenshot.
fn closes(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let plain = act(session, client, "launch", json!({"preset": "plain"}))?;
    expect_outcome(&plain, "one", "launch plain")?;
    let closed = act(
        session,
        client,
        "close_window",
        json!({"id": only_window(&plain)?}),
    )?;
    expect_outcome(&closed, "closed", "close_window")?;
    session.log(&format!("M3: close_window: {closed}"))?;
    let keep = act(session, client, "launch", json!({"preset": "keep"}))?;
    expect_outcome(&keep, "one", "launch keep")?;
    let result = client.call(session, "close_window", json!({"id": only_window(&keep)?}))?;
    let pending = with_screenshot(&result)?;
    expect_outcome(
        &pending,
        "pending",
        "close_window on a window that stays open",
    )?;
    session.log(&format!(
        "M3: close_window pending, with a screenshot: {pending}"
    ))
}

/// The harness moves focus to `target` while `launch` waits for the slow fixture.
fn interrupted(session: &mut Session<'_>, client: &mut Client, target: u64) -> Result<()> {
    let started = session.test_dir().root().join("slow-started");
    let id = client.start_call("launch", json!({"preset": "slow"}))?;
    // niri has spawned the fixture, so the server is waiting for its window.
    session.wait_until("m3-slow", "the slow fixture started", WAIT, |_| {
        Ok(Path::new(&started).exists().then_some(()))
    })?;
    session.request(&Request::Action(Action::FocusWindow { id: target }))?;
    let result = client.result(session, id)?;
    let outcome = with_screenshot(&result)?;
    expect_outcome(&outcome, "interrupted", "launch while focus moves")?;
    expect(
        field(&outcome, "/focused_window") == target,
        "interrupted names where focus went",
        &outcome.to_string(),
    )?;
    session.log(&format!(
        "M3: launch interrupted, with a screenshot: {outcome}"
    ))
}

/// `niri_action` on a fixture that takes the size niri configures: fullscreen fills the
/// output and a second toggle gives the old size back, floating is reported, and a fixed
/// width is the window's width. `Spawn` is refused without `unrestricted`.
fn layout(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let outputs = structured(&client.call(session, "outputs", json!({}))?)?;
    let logical = field(&outputs, "/winit/logical");
    let screen = json!([field(logical, "/width"), field(logical, "/height")]);
    let sized = act(session, client, "launch", json!({"preset": "sized"}))?;
    expect_outcome(&sized, "one", "launch sized")?;
    let id = only_window(&sized)?;
    let tiled = field(&window(session, client, id)?, "/layout/window_size").clone();
    let fullscreen = json!({"action": {"FullscreenWindow": {"id": id}}});
    let full = act(session, client, "niri_action", fullscreen.clone())?;
    expect_outcome(&full, "changed", "FullscreenWindow")?;
    expect(
        field(&full, "/window/window_size") == &screen,
        &format!("the fullscreen window is the output's size {screen}"),
        &full.to_string(),
    )?;
    let back = act(session, client, "niri_action", fullscreen)?;
    expect_outcome(&back, "changed", "FullscreenWindow again")?;
    expect(
        field(&back, "/window/window_size") == &tiled,
        &format!("the window is back to {tiled}"),
        &back.to_string(),
    )?;
    session.log(&format!(
        "M3: niri_action fullscreen and back: {full}, {back}"
    ))?;
    let float = json!({"action": {"ToggleWindowFloating": {"id": id}}});
    let floating = act(session, client, "niri_action", float)?;
    expect_outcome(&floating, "changed", "ToggleWindowFloating")?;
    expect(
        field(&floating, "/window/is_floating") == true,
        "the window floats",
        &floating.to_string(),
    )?;
    let width = json!({"action": {"SetWindowWidth": {"id": id, "change": {"SetFixed": 400}}}});
    let wide = act(session, client, "niri_action", width)?;
    expect_outcome(&wide, "changed", "SetWindowWidth")?;
    expect(
        field(&wide, "/window/window_size/0") == 400,
        "the window is 400 wide",
        &wide.to_string(),
    )?;
    session.log(&format!(
        "M3: niri_action float and width: {floating}, {wide}"
    ))?;
    let spawn = json!({"action": {"Spawn": {"command": ["true"]}}});
    let refused = client.call(session, "niri_action", spawn)?;
    expect(
        field(&refused, "/structuredContent/error") == "unrestricted_required",
        "Spawn refused without unrestricted",
        &refused.to_string(),
    )?;
    let closed = act(session, client, "close_window", json!({"id": id}))?;
    expect_outcome(&closed, "closed", "close the sized window")?;
    session.log(&format!("M3: niri_action Spawn refused: {refused}"))
}

/// A server whose policy turns on `unrestricted` spawns a fixture through `niri_action`,
/// which `wait_for` then sees, and talks to the nested Noctalia through `noctalia`.
fn unrestricted(session: &mut Session<'_>, server: &str) -> Result<()> {
    write_file(session, "unrestricted = true\n")?;
    let mut client = Client::start(session, server, "harness-m3-unrestricted", SERVER_DEADLINE)?;
    ready(session, &mut client)?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let harness = std::env::current_exe().context("find the harness binary")?;
    let test_dir = session.test_dir().root();
    let command = json!([harness, "window", test_dir, "spawned"]);
    let spawn = json!({"action": {"Spawn": {"command": command}}});
    let spawned = act(session, &mut client, "niri_action", spawn)?;
    expect_outcome(&spawned, "sent", "Spawn with unrestricted")?;
    let until = json!({"until": {"window": {"app_id": "spawned"}}, "timeout_ms": 5000});
    let seen = structured(&client.call(session, "wait_for", until)?)?;
    let id = match field(&seen, "/windows").as_array().map(Vec::as_slice) {
        Some([id]) if field(&seen, "/observed") == "met" => id.clone(),
        _ => return Err(Failure::new(format!("M3: no spawned window: {seen}"))),
    };
    let closed = act(session, &mut client, "close_window", json!({"id": id}))?;
    expect_outcome(&closed, "closed", "close the spawned window")?;
    session.log(&format!(
        "M3: niri_action Spawn with unrestricted: {spawned}, {seen}"
    ))?;
    noctalia(session, &mut client)?;
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    client.stop()
}

/// `noctalia` returns the nested Noctalia's reply, and its `error:` reply as
/// `upstream_error` with Noctalia's text.
fn noctalia(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let status = act(session, client, "noctalia", json!({"args": ["status"]}))?;
    expect_outcome(&status, "sent", "noctalia status")?;
    let reply = field(&status, "/noctalia/reply")
        .as_str()
        .unwrap_or_default();
    let parsed: Option<Value> = serde_json::from_str(reply).ok();
    expect(
        parsed
            .as_ref()
            .is_some_and(|answer| field(answer, "/locked") == false),
        "Noctalia's status reply, unlocked",
        &status.to_string(),
    )?;
    let unknown = client.call(session, "noctalia", json!({"args": ["no-such-command"]}))?;
    expect(
        field(&unknown, "/structuredContent/error") == "upstream_error"
            && field(&unknown, "/structuredContent/detail")
                .as_str()
                .is_some_and(|detail| detail.starts_with("Noctalia replied: error:")),
        "an unknown command is Noctalia's error",
        &unknown.to_string(),
    )?;
    session.log(&format!("M3: noctalia passthrough: {status}, {unknown}"))
}

fn focus(session: &mut Session<'_>, client: &mut Client, id: u64) -> Result<()> {
    let focused = act(session, client, "focus_window", json!({"id": id}))?;
    expect_outcome(&focused, "focused", "focus_window")?;
    session.log(&format!("M3: focus_window {id}: {focused}"))
}

/// An action that must succeed without a screenshot, as outcomes that aren't in doubt do.
fn act(session: &mut Session<'_>, client: &mut Client, tool: &str, args: Value) -> Result<Value> {
    let result = client.call(session, tool, args)?;
    let content = field(&result, "/content").as_array().map_or(0, Vec::len);
    expect(
        content == 1,
        &format!("{tool} without a screenshot"),
        &result.to_string(),
    )?;
    structured(&result)
}

/// The outcome of a result that must carry a screenshot of the focused output, the image
/// after the text.
fn with_screenshot(result: &Value) -> Result<Value> {
    let outcome = structured(result)?;
    expect(
        field(result, "/content/1/type") == "image"
            && field(result, "/content/1/mimeType") == "image/jpeg"
            && field(&outcome, "/screenshot/output") == "winit",
        "a screenshot with the outcome",
        &outcome.to_string(),
    )?;
    Ok(outcome)
}

/// The one id in an outcome's `windows`.
fn only_window(outcome: &Value) -> Result<u64> {
    match field(outcome, "/windows").as_array().map(Vec::as_slice) {
        Some([id]) => id
            .as_u64()
            .ok_or_else(|| Failure::new(format!("M3: window id isn't a number in {outcome}"))),
        _ => Err(Failure::new(format!(
            "M3: expected one window in {outcome}"
        ))),
    }
}

/// The window with `id` in `desktop_state`.
fn window(session: &mut Session<'_>, client: &mut Client, id: u64) -> Result<Value> {
    let state = structured(&client.call(session, "desktop_state", json!({}))?)?;
    windows(&state)
        .find(|window| field(window, "/id") == id)
        .cloned()
        .ok_or_else(|| Failure::new(format!("M3: no window {id} in desktop_state")))
}

/// How many windows with `app_id` `desktop_state` lists.
fn count(session: &mut Session<'_>, client: &mut Client, app_id: &str) -> Result<usize> {
    let state = structured(&client.call(session, "desktop_state", json!({}))?)?;
    Ok(windows(&state)
        .filter(|window| field(window, "/app_id") == app_id)
        .count())
}

fn windows(state: &Value) -> impl Iterator<Item = &Value> {
    field(state, "/windows").as_array().into_iter().flatten()
}

/// `observed`, after an action niri accepted, so a check can't pass through an outcome
/// reached without sending anything.
fn expect_outcome(outcome: &Value, observed: &str, what: &str) -> Result<()> {
    expect(
        field(outcome, "/observed") == observed && field(outcome, "/accepted") == true,
        &format!("{what}: accepted and observed {observed}"),
        &outcome.to_string(),
    )
}

/// The value at a JSON pointer, or null.
fn expect(holds: bool, what: &str, seen: &str) -> Result<()> {
    if holds {
        Ok(())
    } else {
        Err(Failure::new(format!("M3: {what} failed; saw {seen}")))
    }
}
