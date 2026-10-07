//! C13: Noctalia in the nested session, driven by the `noctalia-socket` probe. Noctalia
//! starts from the session, so it inherits NESTED and reads the config `harness run`
//! generated. Every command goes to its socket under `TEST_DIR/run`, and the probe can only
//! send `status`, `panel-open control-center` and `panel-close control-center`.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::failure::{Context as _, Failure, Result};
use crate::session::Session;
use crate::{capture, image};

/// Within what is left of the 60 s deadline `harness run` gives the nested run.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(30);
/// Noctalia binds its socket near the end of startup, after its UI (`Application::run`).
const STARTUP: Duration = Duration::from_secs(10);
/// C13's pass rule: each panel change is seen within 2 s of sending it.
const PANEL_WAIT: Duration = Duration::from_secs(2);
const PANEL: &str = "control-center";
/// A drawn panel changes at least one in this many pixels of the capture taken before
/// `panel-open`. The bar's clock changes far fewer.
const DRAWN_SHARE: usize = 100;

pub(crate) fn c13(session: &mut Session<'_>, probe: &str) -> Result<()> {
    let log = session.artifact("noctalia.log");
    let noctalia = session.start("noctalia", &[], log, NOCTALIA_DEADLINE)?;
    let (socket, status) = first_status(session, probe)?;
    session.log(&format!("C13: Noctalia socket {}", socket.display()))?;
    session.log(&format!("C13 status reply: {status:?}"))?;
    if let Some(panel) = active_panel(&status)? {
        return Err(Failure::new(format!(
            "C13: panel {panel:?} was open before panel-open"
        )));
    }
    let closed = settled(session)?;
    change_panel(session, probe, "panel-open", Some(PANEL))?;
    drawn(session, &closed, true)?;
    session.screenshot("success-c13.png")?;
    session.log("saved success-c13.png")?;
    change_panel(session, probe, "panel-close", None)?;
    drawn(session, &closed, false)?;
    noctalia.stop().map(drop)
}

/// Waits for the socket and a reply to `status` on it. Noctalia binds the socket before it
/// listens (`IpcService::start`), so a connect in between is refused. Until the deadline a
/// failed `status` counts as not ready yet, and a timeout reports the last failure.
fn first_status(session: &mut Session<'_>, probe: &str) -> Result<(PathBuf, String)> {
    let mut last = None;
    let ready = session.wait_until(
        "noctalia-ready",
        "a status reply on the nested Noctalia's socket",
        STARTUP,
        |session| {
            let Some(socket) = session.noctalia_socket()? else {
                return Ok(None);
            };
            match send(session, probe, "status") {
                Ok(status) => Ok(Some((socket, status))),
                Err(failure) => {
                    last = Some(failure);
                    Ok(None)
                }
            }
        },
    );
    ready.map_err(|failure| match last {
        Some(last) => Failure::new(format!("{failure}; last status failure: {last}")),
        None => failure,
    })
}

/// The capture the panel checks compare with. Noctalia answers `status` before it has
/// drawn its wallpaper, so this waits until less than half the output is still the nested
/// niri's magenta and fewer than 1% of the pixels changed since the previous capture.
fn settled(session: &mut Session<'_>) -> Result<Vec<u8>> {
    let started = Instant::now();
    let mut previous: Option<Vec<u8>> = None;
    let baseline = session.wait_until(
        "c13-settled",
        "Noctalia's wallpaper drawn and the output still",
        STARTUP,
        |session| {
            let now = capture::nested_ppm(session)?;
            if let Some(previous) = &previous
                && still(previous, &now)?
            {
                return Ok(Some(now));
            }
            previous = Some(now);
            Ok(None)
        },
    )?;
    session.log(&format!(
        "C13 baseline settled after {:.0} ms",
        started.elapsed().as_secs_f64() * 1000.0
    ))?;
    Ok(baseline)
}

fn still(previous: &[u8], now: &[u8]) -> Result<bool> {
    let unreadable = || Failure::new("grim wrote an unreadable PPM");
    let previous = image::ppm(previous).ok_or_else(unreadable)?;
    let now = image::ppm(now).ok_or_else(unreadable)?;
    let difference = image::difference(&previous, &now)
        .ok_or_else(|| Failure::new("the nested output changed size"))?;
    let total = now.width * now.height;
    Ok(image::count(&now, capture::MAGENTA) * 2 < total && !shown(difference.pixels, total))
}

/// Not part of C13's pass rule: waits until the panel is drawn (`open`) or gone again,
/// compared with `closed`, a capture taken before `panel-open`.
fn drawn(session: &mut Session<'_>, closed: &[u8], open: bool) -> Result<()> {
    let closed = image::ppm(closed).ok_or_else(|| Failure::new("grim wrote an unreadable PPM"))?;
    let (step, what) = if open {
        ("c13-drawn", "the panel drawn")
    } else {
        ("c13-gone", "the panel gone")
    };
    let difference = session.wait_until(step, what, PANEL_WAIT, |session| {
        let now = capture::nested_ppm(session)?;
        let now = image::ppm(&now).ok_or_else(|| Failure::new("grim wrote an unreadable PPM"))?;
        let difference = image::difference(&closed, &now)
            .ok_or_else(|| Failure::new("the nested output changed size"))?;
        let total = closed.width * closed.height;
        if whole(difference.pixels, total) {
            return Err(Failure::new(format!(
                "C13 {what}: {} of {total} pixels differ from the baseline; a panel covers \
                 far less, so the output was not what the baseline captured",
                difference.pixels
            )));
        }
        let shown = shown(difference.pixels, total);
        Ok((shown == open).then_some(difference))
    })?;
    session.log(&format!(
        "C13 {what}: {} pixels differ from the capture before panel-open, in {}",
        difference.pixels,
        difference
            .area
            .map_or_else(|| "no area".to_owned(), capture::describe)
    ))
}

/// Whether `changed` of `total` pixels count as a drawn panel: at least 1%. Below that,
/// the panel counts as gone, even if the bar's clock changed in between.
const fn shown(changed: usize, total: usize) -> bool {
    changed * DRAWN_SHARE >= total
}

/// At least 90% of the pixels changed. The control center covered about 23% of the output
/// at scale 1 and 53% at scale 1.5; a baseline taken before the wallpaper changes all of it.
const fn whole(changed: usize, total: usize) -> bool {
    changed * 10 >= total * 9
}

/// Sends `<verb> control-center`, requires Noctalia's `ok`, and waits for `activePanelId`
/// to become `expected` within 2 s of sending.
fn change_panel(
    session: &mut Session<'_>,
    probe: &str,
    verb: &str,
    expected: Option<&str>,
) -> Result<()> {
    let command = format!("{verb} {PANEL}");
    let sent = Instant::now();
    let reply = send(session, probe, &command)?;
    if reply != "ok\n" {
        return Err(Failure::new(format!(
            "C13 {command}: expected \"ok\\n\", got {reply:?}"
        )));
    }
    let step = format!("c13-{verb}");
    let what = format!("activePanelId {expected:?}");
    let left = PANEL_WAIT.saturating_sub(sent.elapsed());
    let status = session.wait_until(&step, &what, left, |session| {
        let status = send(session, probe, "status")?;
        Ok((active_panel(&status)?.as_deref() == expected).then_some(status))
    })?;
    session.log(&format!(
        "C13 {command}: activePanelId {expected:?} after {:.0} ms: pass; status reply: {status:?}",
        sent.elapsed().as_secs_f64() * 1000.0
    ))
}

/// Runs the probe on the nested Noctalia's socket. A Noctalia `error:` reply or an I/O
/// error fails the probe, and the runner keeps its stderr.
fn send(session: &Session<'_>, probe: &str, command: &str) -> Result<String> {
    let socket = session
        .noctalia_socket()?
        .ok_or_else(|| Failure::new("the nested Noctalia's socket is gone"))?;
    let output = session.run(probe, &[socket.into_os_string(), OsString::from(command)])?;
    String::from_utf8(output.stdout).context(format!("Noctalia's reply to {command}"))
}

/// `activePanelId` from a `status` reply: the panel id, or `None` for JSON null. Anything
/// else fails, the way plan §4 says the server must treat a reply that does not parse.
fn active_panel(status: &str) -> Result<Option<String>> {
    let value: Value =
        serde_json::from_str(status).context(format!("Noctalia status {status:?}"))?;
    match value.get("activePanelId") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(panel)) => Ok(Some(panel.clone())),
        Some(Value::Bool(_) | Value::Number(_) | Value::Array(_) | Value::Object(_)) | None => {
            Err(Failure::new(format!(
                "Noctalia status has no activePanelId string or null: {status:?}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape of Noctalia 5.2.1's reply (`application_ipc.cpp`, the `status` handler).
    fn status(active: &str) -> String {
        format!(
            "{{\n  \"barVisible\": true,\n  \"panelOpen\": true,\n  \"activePanelId\": {active},\n  \"locked\": false\n}}\n"
        )
    }

    fn ppm(width: usize, height: usize, pixel: impl Fn(usize) -> [u8; 3]) -> Vec<u8> {
        let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
        for index in 0..width * height {
            bytes.extend_from_slice(&pixel(index));
        }
        bytes
    }

    #[test]
    fn the_baseline_needs_the_wallpaper_and_a_still_output() {
        let magenta = ppm(10, 10, |_| capture::MAGENTA);
        let wallpaper = ppm(10, 10, |_| [0x10, 0x10, 0x40]);
        let half = ppm(10, 10, |index| {
            if index < 50 {
                capture::MAGENTA
            } else {
                [0x10, 0x10, 0x40]
            }
        });
        let bar = ppm(10, 10, |index| {
            if index < 10 {
                [0xEE, 0xEE, 0xEE]
            } else {
                [0x10, 0x10, 0x40]
            }
        });
        assert!(!still(&magenta, &magenta).unwrap());
        assert!(!still(&half, &half).unwrap());
        assert!(!still(&magenta, &wallpaper).unwrap());
        assert!(!still(&wallpaper, &bar).unwrap());
        assert!(still(&wallpaper, &wallpaper).unwrap());
        assert!(still(&bar, &bar).unwrap());
    }

    #[test]
    fn a_panel_is_drawn_from_one_percent_of_the_pixels() {
        let total = 960 * 720;
        assert!(shown(6912, total));
        assert!(!shown(6911, total));
        assert!(!shown(0, total));
    }

    #[test]
    fn nearly_the_whole_output_changing_is_not_a_panel() {
        let total = 960 * 720;
        assert!(whole(total, total));
        assert!(whole(622_080, total));
        assert!(!whole(622_079, total));
        assert!(!whole(370_450, total));
    }

    #[test]
    fn reads_the_active_panel_or_null() {
        assert_eq!(
            active_panel(&status("\"control-center\""))
                .unwrap()
                .as_deref(),
            Some("control-center")
        );
        assert_eq!(active_panel(&status("null")).unwrap(), None);
    }

    #[test]
    fn a_status_that_does_not_parse_or_lacks_the_panel_fails() {
        for reply in [
            String::new(),
            "ok\n".to_owned(),
            "{\"panelOpen\": false}\n".to_owned(),
            status("1"),
            status("false"),
            "{\"activePanelId\": \"control-center\"".to_owned(),
        ] {
            assert!(active_panel(&reply).is_err(), "{reply:?}");
        }
    }
}
