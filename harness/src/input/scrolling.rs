//! Scrolling a real client by the expected amount: kitty, showing `seq 1 500` with
//! `wheel_scroll_multiplier 5`, scrolls five lines per wheel notch. Its remote control
//! socket, under `TEST_DIR`, tells which line is at the top of the screen, which moves by
//! exactly the lines scrolled.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

use super::{Shot, send};
use crate::failure::{Failure, Result};
use crate::mcp::{Client, field, structured};
use crate::session::Session;

const KITTY_DEADLINE: Duration = Duration::from_secs(60);
/// kitty takes a few seconds to start without a session bus's portal.
const STARTUP: Duration = Duration::from_secs(15);
const LINES: u32 = 500;
/// kitty's lines per notch of a discrete wheel, set rather than left to its default.
const PER_NOTCH: u32 = 5;

pub(super) fn run(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let socket = session.test_dir().run().join("kitty.sock");
    let listen = format!("unix:{}", socket.display());
    let multiplier = format!("wheel_scroll_multiplier={PER_NOTCH}");
    let args: Vec<OsString> = [
        "--config",
        "NONE",
        "--hold",
        "-o",
        "allow_remote_control=yes",
        "-o",
        &multiplier,
        "--listen-on",
        &listen,
        "seq",
        "1",
        &LINES.to_string(),
    ]
    .iter()
    .map(OsString::from)
    .collect();
    let kitty = session.start(
        "kitty",
        &args,
        session.artifact("kitty.log"),
        KITTY_DEADLINE,
    )?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let centre = kitty_centre(session, client)?;
    let start = settled_top(session, &socket)?;
    let shot = Shot::take(session, client, 4000)?;
    let pixel = shot.pixel(centre);
    let up = start - 2 * PER_NOTCH;
    for (notches, top) in [(-2, up), (1, up + PER_NOTCH)] {
        let arguments = json!({
            "screenshot_ref": shot.id, "x": pixel.0, "y": pixel.1, "notches_y": notches
        });
        send(session, client, "scroll", arguments)?;
        expect_top(session, &socket, top)?;
        session.log(&format!(
            "M4 scroll kitty {notches} notches: line {top} at the top, from {start}"
        ))?;
    }
    structured(&client.call(session, "release_desktop", json!({}))?)?;
    kitty.stop().map(drop)
}

/// The layout point at the middle of kitty's window, once it is placed.
fn kitty_centre(session: &mut Session<'_>, client: &mut Client) -> Result<(f64, f64)> {
    session.wait_until("m4-kitty", "kitty's window", STARTUP, |session| {
        let desktop = structured(&client.call(session, "desktop_state", json!({}))?)?;
        Ok(field(&desktop, "/windows")
            .as_array()
            .into_iter()
            .flatten()
            .find(|window| field(window, "/app_id") == "kitty")
            .and_then(centre))
    })
}

/// The window's middle in the layout, on nested niri's only output at the origin.
fn centre(window: &Value) -> Option<(f64, f64)> {
    let layout = field(window, "/layout");
    let x = field(layout, "/tile_pos_in_workspace_view/0").as_f64()?;
    let y = field(layout, "/tile_pos_in_workspace_view/1").as_f64()?;
    let width = field(layout, "/window_size/0").as_f64()?;
    let height = field(layout, "/window_size/1").as_f64()?;
    Some((x + width / 2.0, y + height / 2.0))
}

/// The top line once kitty shows the end of `seq`.
fn settled_top(session: &mut Session<'_>, socket: &Path) -> Result<u32> {
    session.wait_until(
        "m4-kitty-text",
        "seq's output in kitty",
        STARTUP,
        |session| {
            let screen = screen(session, socket);
            Ok(screen
                .as_deref()
                .filter(|text| text.lines().any(|line| line.trim() == LINES.to_string()))
                .and_then(top))
        },
    )
}

/// Waits until the first number on kitty's screen is `line`.
fn expect_top(session: &mut Session<'_>, socket: &Path, line: u32) -> Result<()> {
    let mut last = None;
    let found = session.wait_until("m4-kitty-line", "kitty's top line", STARTUP, |session| {
        last = screen(session, socket).as_deref().and_then(top);
        Ok((last == Some(line)).then_some(()))
    });
    found.map_err(|failure| {
        Failure::new(format!(
            "M4 scroll kitty: expected line {line} at the top, saw {last:?}: {failure}"
        ))
    })
}

/// kitty's visible text, through its remote control socket.
fn screen(session: &Session<'_>, socket: &Path) -> Option<String> {
    let to = format!("unix:{}", socket.display());
    let args: Vec<OsString> = ["@", "--to", &to, "get-text"]
        .iter()
        .map(OsString::from)
        .collect();
    let output = session.run("kitty", &args).ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The first line of the screen's text that is a number.
fn top(screen: &str) -> Option<u32> {
    screen.lines().find_map(|line| line.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_top_line_is_the_first_number_on_screen() {
        assert_eq!(top("\n498\n499\n500\n\n"), Some(498));
        assert_eq!(top("x\n2\n"), Some(2));
        assert_eq!(top(""), None);
        let window = json!({"layout": {
            "tile_pos_in_workspace_view": [10.0, 20.0], "window_size": [400, 300]
        }});
        assert_eq!(centre(&window), Some((210.0, 170.0)));
    }
}
