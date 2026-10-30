//! The pointer tools on two outputs (`--multi`), in a niri built with
//! `harness/niri-test/headless-outputs.patch`, which gives it niri's own headless backend
//! with the outputs `NIRI_TEST_HEADLESS_OUTPUTS` lists. One server holds the lease, with the
//! nested Noctalia as the lock source: `status` must list both outputs as taking the
//! pointer, C4's points must land on `wev` on each output, at its own scale and a lowered
//! one, and a GTK button must count each click on each output.

use std::collections::HashMap;
use std::fs;
use std::time::Duration;

use niri_ipc::{Action, LogicalOutput, Request, Response, Transform, WindowLayout};
use serde_json::json;

use super::{READY, SERVER_DEADLINE, Shot, WAIT, Wev, accuracy_through, send, surface_origin};
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field, structured};
use crate::session::Session;

/// What the harness sets `NIRI_TEST_HEADLESS_OUTPUTS` to: the outputs' physical sizes.
pub(crate) const HEADLESS_OUTPUTS: &str = "1280x720,1920x1080";

/// Their place in the layout: the second at 1.5, left of the first and lower, so its
/// origin is negative.
pub(crate) const OUTPUT_CONFIG: &str = r#"
output "headless-1" {
    scale 1
    position x=0 y=0
}

output "headless-2" {
    scale 1.5
    position x=-1280 y=100
}
"#;

/// The layout `OUTPUT_CONFIG` gives, as niri reports it.
const LAYOUT: [(&str, LogicalOutput); 2] = [
    (
        "headless-1",
        LogicalOutput {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
            scale: 1.0,
            transform: Transform::Normal,
        },
    ),
    (
        "headless-2",
        LogicalOutput {
            x: -1280,
            y: 100,
            width: 1280,
            height: 720,
            scale: 1.5,
            transform: Transform::Normal,
        },
    ),
];
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(140);
const WEV_DEADLINE: Duration = Duration::from_secs(60);
const CLICKS: u32 = 20;

pub(crate) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    layout(session)?;
    let mut client = Client::start(session, server, "harness-multi", SERVER_DEADLINE)?;
    let status = mcp::ready(session, &mut client, "multi-ready", READY)?;
    let taking = field(&status, "/outputs/pointer_outputs");
    if *taking != json!(["headless-1", "headless-2"]) {
        return Err(Failure::new(format!(
            "multi: status lists {taking} as taking the pointer"
        )));
    }
    session.log(&format!("multi status: pointer_outputs {taking}"))?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    wev_on_each(session, &mut client)?;
    gtk_on_each(session, &mut client)?;
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    client.stop()?;
    noctalia.stop().map(drop)
}

/// Requires niri's outputs to be `LAYOUT` and nothing else.
fn layout(session: &mut Session<'_>) -> Result<()> {
    let Response::Outputs(outputs) = session.request(&Request::Outputs)? else {
        return Err(Failure::new("niri answered Outputs with another response"));
    };
    let found: HashMap<String, LogicalOutput> = outputs
        .into_iter()
        .filter_map(|(name, output)| Some((name, output.logical?)))
        .collect();
    let expected: HashMap<String, LogicalOutput> = LAYOUT
        .iter()
        .map(|(name, logical)| ((*name).to_owned(), *logical))
        .collect();
    if found != expected {
        return Err(Failure::new(format!(
            "multi: niri's outputs are {found:?}, expected {expected:?}"
        )));
    }
    session.log(&format!("multi outputs: {found:?}"))
}

/// C4's points on `wev`, moved to each output in turn.
fn wev_on_each(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let log = session.artifact("wev.log");
    let args = ["-oL", "wev"].map(std::ffi::OsString::from);
    let process = session.start("stdbuf", &args, log.clone(), WEV_DEADLINE)?;
    for (name, output) in LAYOUT {
        let layout = move_to(session, "wev", name)?;
        let wev = Wev {
            log: &log,
            surface: surface_origin(&output, &layout)?,
        };
        let target = format!("output:{name}");
        accuracy_through(session, client, &wev, &target)?;
    }
    process.stop().map(drop)
}

/// The GTK fixture's button, clicked `CLICKS` times on each output, must count each click
/// once.
fn gtk_on_each(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let check = "multi GTK activation";
    let Some(process) = super::activation::start_fixture(session, "button", check)? else {
        return Ok(());
    };
    let mut expected = 0;
    for (name, output) in LAYOUT {
        let layout = move_to(session, "org.ncu.Activation", name)?;
        let (x, y) = surface_origin(&output, &layout)?;
        let (width, height) = layout.window_size;
        let centre = (x + f64::from(width) / 2.0, y + f64::from(height) / 2.0);
        let shot = Shot::through(session, client, &format!("output:{name}"), 4000)?;
        let pixel = shot.pixel(centre);
        for _ in 0..CLICKS {
            expected += 1;
            let arguments = json!({"screenshot_ref": shot.id, "x": pixel.0, "y": pixel.1});
            send(session, client, "click", arguments)?;
            counted(session, expected)?;
        }
        session.log(&format!(
            "{check}: {CLICKS}/{CLICKS} clicks counted on {name}"
        ))?;
    }
    process.stop().map(drop)
}

/// Waits until the GTK button has counted `expected` clicks, and no more.
fn counted(session: &mut Session<'_>, expected: u32) -> Result<()> {
    session.wait_until(
        "multi-gtk-click",
        "GTK activation",
        Duration::from_secs(2),
        |session| {
            let count = fs::read_to_string(session.test_dir().root().join("activations"))
                .context("read GTK activation count")?;
            let actual = count.parse::<u32>().ok();
            if actual.is_some_and(|actual| actual > expected) {
                return Err(Failure::new(
                    "GTK button activated more than once per click",
                ));
            }
            Ok((actual == Some(expected)).then_some(()))
        },
    )
}

/// Moves the floating window of `app_id` to `output`, and waits until niri shows it there.
fn move_to(session: &mut Session<'_>, app_id: &str, output: &str) -> Result<WindowLayout> {
    let id = session.wait_until("multi-window", app_id, WAIT, |session| {
        Ok(windows(session)?
            .into_iter()
            .find(|window| window.app_id.as_deref() == Some(app_id))
            .map(|window| window.id))
    })?;
    session.request(&Request::Action(Action::MoveWindowToMonitor {
        id: Some(id),
        output: output.to_owned(),
    }))?;
    let layout = session.wait_until("multi-moved", app_id, WAIT, |session| {
        let Response::Workspaces(workspaces) = session.request(&Request::Workspaces)? else {
            return Err(Failure::new(
                "niri answered Workspaces with another response",
            ));
        };
        let on_output = |workspace: Option<u64>| {
            workspaces
                .iter()
                .any(|shown| Some(shown.id) == workspace && shown.output.as_deref() == Some(output))
        };
        Ok(windows(session)?.into_iter().find_map(|window| {
            let placed = window.id == id
                && window.is_floating
                && on_output(window.workspace_id)
                && window.layout.tile_pos_in_workspace_view.is_some();
            placed.then_some(window.layout)
        }))
    })?;
    session.log(&format!("multi: {app_id} on {output}: {layout:?}"))?;
    Ok(layout)
}

fn windows(session: &mut Session<'_>) -> Result<Vec<niri_ipc::Window>> {
    let Response::Windows(windows) = session.request(&Request::Windows)? else {
        return Err(Failure::new("niri answered Windows with another response"));
    };
    Ok(windows)
}
