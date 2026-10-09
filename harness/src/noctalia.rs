//! Noctalia in the nested session, driven through `niri-computer-use`'s shell tools. C13
//! (`make nested NOCTALIA=1`) opens and closes the control center; `make nested-shell`
//! (`shell.rs`) runs the same cycle for every allowlisted panel. Noctalia starts from the
//! session, so it inherits NESTED and reads the config `harness run` generated, and the
//! server, also in NESTED, finds its socket under `TEST_DIR/run`.

use std::ffi::OsString;
use std::time::{Duration, Instant};

use serde_json::json;

use crate::failure::{Failure, Result};
use crate::mcp::{self, Client, field, structured};
use crate::runner::Process;
use crate::session::Session;
use crate::{capture, image};

/// Within what is left of the 60 s deadline `harness run` gives the nested run.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(30);
const SERVER_DEADLINE: Duration = Duration::from_secs(25);
/// Noctalia binds its socket near the end of startup, after its UI (`Application::run`).
const STARTUP: Duration = Duration::from_secs(10);
const PANEL: &str = "control-center";
/// C13's pass rule: each panel change is seen within 2 s of sending it.
const PANEL_WAIT: Duration = Duration::from_secs(2);
/// What the OCR check looks for in the open control center: the title of its first tab
/// (`control-center.tabs.home` in Noctalia's English strings).
const CONTROL_CENTER_WORD: &str = "Home";
/// For the capture checks, which aren't part of C13's rule. Noctalia can report a panel
/// open well before it draws it: one run logged a 1.9 s rendering stall.
const DRAWN_WAIT: Duration = Duration::from_secs(5);
/// A drawn panel changes at least one in this many pixels of the capture taken before
/// `panel-open`. The bar's clock changes far fewer.
const DRAWN_SHARE: usize = 100;

pub(crate) fn c13(session: &mut Session<'_>, server: &str) -> Result<()> {
    let noctalia = start(session, NOCTALIA_DEADLINE)?;
    let mut client = Client::start(session, server, "harness-c13", SERVER_DEADLINE)?;
    let status = mcp::ready(session, &mut client, "c13-ready", STARTUP)?;
    session.log(&format!(
        "C13: Noctalia {}, lock {}",
        field(&status, "/noctalia"),
        field(&status, "/lock")
    ))?;
    no_panel_open(session, &mut client)?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let closed = settled(session)?;
    cycle(session, &mut client, PANEL, Some(&closed))?;
    structured(&client.call(session, "release_desktop", json!({}))?)?;
    client.stop()?;
    noctalia.stop().map(drop)
}

/// Starts the nested Noctalia, which runs until it is stopped or `deadline` passes.
pub(crate) fn start(session: &Session<'_>, deadline: Duration) -> Result<Process> {
    session.start("noctalia", &[], session.artifact("noctalia.log"), deadline)
}

/// `shell_status` reports no open panel.
pub(crate) fn no_panel_open(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let status = structured(&client.call(session, "shell_status", json!({}))?)?;
    if !field(&status, "/activePanelId").is_null() {
        return Err(Failure::new(format!(
            "expected no open panel; shell_status says {status}"
        )));
    }
    session.log(&format!("shell_status, no panel open: {status}"))
}

/// Opens `panel` with `shell_open` and closes it with `shell_close`, each seen in
/// `activePanelId` within the server's two seconds. With `closed`, a capture with no panel
/// open, it also checks the panel is drawn and then gone.
pub(crate) fn cycle(
    session: &mut Session<'_>,
    client: &mut Client,
    panel: &str,
    closed: Option<&[u8]>,
) -> Result<()> {
    change(session, client, "shell_open", panel, Some(panel))?;
    if let Some(closed) = closed {
        drawn(session, closed, panel, true)?;
    }
    let shot = format!("success-{panel}.png");
    session.screenshot(&shot)?;
    session.log(&format!("saved {shot}"))?;
    if panel == PANEL {
        ocr(session, CONTROL_CENTER_WORD)?;
    }
    change(session, client, "shell_close", panel, None)?;
    match closed {
        Some(closed) => drawn(session, closed, panel, false),
        None => session.log(&format!("{panel}: no pixel check")),
    }
}

/// Calls `tool` on `panel` and requires that Noctalia accepted it and the server saw
/// `activePanelId` become `expected`. The server finds Noctalia's socket from NESTED, so
/// first the socket must resolve under `TEST_DIR/run`: a panel command never reaches the
/// host's Noctalia.
fn change(
    session: &mut Session<'_>,
    client: &mut Client,
    tool: &str,
    panel: &str,
    expected: Option<&str>,
) -> Result<()> {
    let observed = if expected.is_some() {
        "opened"
    } else {
        "closed"
    };
    session
        .noctalia_socket()?
        .ok_or_else(|| Failure::new("the nested Noctalia's socket is gone"))?;
    let sent = Instant::now();
    let outcome = structured(&client.call(session, tool, json!({"panel": panel}))?)?;
    let seen = field(&outcome, "/accepted") == true
        && field(&outcome, "/observed") == observed
        && field(&outcome, "/shell/active_panel").as_str() == expected;
    if !seen {
        return Err(Failure::new(format!(
            "{tool} {panel}: expected accepted and {observed}; saw {outcome}"
        )));
    }
    // The server's own wait starts after Noctalia's reply, so the rule is checked here,
    // from before the call.
    let elapsed = sent.elapsed();
    if elapsed > PANEL_WAIT {
        return Err(Failure::new(format!(
            "{tool} {panel}: {observed} only after {elapsed:?}, over {PANEL_WAIT:?}"
        )));
    }
    session.log(&format!(
        "{tool} {panel}: {observed} in {:.0} ms: {outcome}",
        elapsed.as_secs_f64() * 1000.0
    ))
}

/// Dev-only, and only with `tesseract` on `PATH`: reads the nested output at twice its
/// scale and requires `word` among the words found. Only whether it was found is logged.
fn ocr(session: &mut Session<'_>, word: &str) -> Result<()> {
    if !on_path("tesseract") {
        return session.log("OCR: skipped, tesseract is not on PATH");
    }
    let image = session.test_dir().root().join("ocr.png");
    let capture = ["-s", "2", "-o", "winit"].map(OsString::from);
    let mut args = capture.to_vec();
    args.push(image.clone().into());
    session.run("grim", &args)?;
    let read = [
        image.into_os_string(),
        "stdout".into(),
        "--psm".into(),
        "11".into(),
    ];
    let text = session.run("tesseract", &read)?.stdout;
    let found = String::from_utf8_lossy(&text)
        .split_whitespace()
        .any(|found| found == word);
    if !found {
        return Err(Failure::new(format!(
            "OCR: {word:?} isn't among the words tesseract read"
        )));
    }
    session.log(&format!("OCR: found {word:?}"))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(program).is_file()))
}

/// The capture the panel checks compare with. Noctalia answers `status` before it has
/// drawn its wallpaper, so this waits until less than half the output is still the nested
/// niri's magenta and fewer than 1% of the pixels changed since the previous capture.
pub(crate) fn settled(session: &mut Session<'_>) -> Result<Vec<u8>> {
    let started = Instant::now();
    let mut previous: Option<Vec<u8>> = None;
    let baseline = session.wait_until(
        "noctalia-settled",
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
        "Noctalia's baseline settled after {:.0} ms",
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

/// Not part of C13's pass rule: waits until `panel` is drawn (`open`) or gone again,
/// compared with `closed`, a capture taken with no panel open.
fn drawn(session: &mut Session<'_>, closed: &[u8], panel: &str, open: bool) -> Result<()> {
    let closed = image::ppm(closed).ok_or_else(|| Failure::new("grim wrote an unreadable PPM"))?;
    let (step, what) = if open {
        (format!("{panel}-drawn"), format!("{panel} drawn"))
    } else {
        (format!("{panel}-gone"), format!("{panel} gone"))
    };
    let difference = session.wait_until(&step, &what, DRAWN_WAIT, |session| {
        let now = capture::nested_ppm(session)?;
        let now = image::ppm(&now).ok_or_else(|| Failure::new("grim wrote an unreadable PPM"))?;
        let difference = image::difference(&closed, &now)
            .ok_or_else(|| Failure::new("the nested output changed size"))?;
        let total = closed.width * closed.height;
        if whole(difference.pixels, total) {
            return Err(Failure::new(format!(
                "{what}: {} of {total} pixels differ from the baseline; a panel covers far \
                 less, so the output was not what the baseline captured",
                difference.pixels
            )));
        }
        let shown = shown(difference.pixels, total);
        Ok((shown == open).then_some(difference))
    })?;
    session.log(&format!(
        "{what}: {} pixels differ from the capture with no panel open, in {}",
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

/// At least 90% of the pixels changed. The control center changed 22–24% of the output at
/// scale 1 and 50–67% at scale 1.5; a baseline taken before the wallpaper changes all of it.
const fn whole(changed: usize, total: usize) -> bool {
    changed * 10 >= total * 9
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
