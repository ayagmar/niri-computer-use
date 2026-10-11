//! `clipboard_read` returns only what was copied during the lease. Without the lease it
//! refuses; a copy the user made before `acquire_desktop` is withheld as
//! `copied_before_lease`; a GTK `Copy link` button the agent clicks puts a link on the
//! clipboard that it then reads, as an app's copy-link button does. A copy marked secret
//! during the lease is `paste`'s check (see `paste::secret`). Optional dev fixture, like
//! A05's button.

use std::fs;
use std::time::Duration;

use serde_json::{Value, json};

use super::paste::{OWNERS_TEXT, own, withheld};
use super::{Shot, send};
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{Client, field, structured};
use crate::session::Session;

const APP_ID: &str = "org.ncu.Copy";
/// What the fixture's button copies.
const LINK: &str = "https://example.invalid/ncu-copied-link";

pub(super) fn run(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let Some(process) = super::activation::start_fixture(session, "copy", "Clipboard lease")?
    else {
        return Ok(());
    };
    refused(session, client, "without the lease")?;
    let (owner, _) = own(session, "copy-before-lease")?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    withheld(session, client, OWNERS_TEXT, "copied_before_lease")?;
    click_copy(session, client)?;
    // The button's copy took the selection from the owner, which then exits.
    owner.wait()?;
    let read = structured(&client.call(session, "clipboard_read", json!({}))?)?;
    if read != json!({"text": LINK, "reason": null}) {
        return Err(Failure::new(format!(
            "Clipboard lease: clipboard_read after Copy link: {read}, not the link"
        )));
    }
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    refused(session, client, "after release_desktop")?;
    process.stop()?;
    session.log(
        "Clipboard lease: clipboard_read: lease_required without the lease and after release_desktop; a copy made before acquire_desktop withheld as copied_before_lease, with its text in neither the result nor the audit log; the link a clicked GTK Copy link button copied read back",
    )
}

/// Clicks the fixture's one button, which fills its window, and waits for its copy.
fn click_copy(session: &mut Session<'_>, client: &mut Client) -> Result<()> {
    let centre = session.wait_until(
        "lease-copy-ready",
        "the Copy link window",
        Duration::from_secs(15),
        |session| {
            let desktop = structured(&client.call(session, "desktop_state", json!({}))?)?;
            Ok(field(&desktop, "/windows")
                .as_array()
                .into_iter()
                .flatten()
                .find(|window| field(window, "/app_id") == APP_ID)
                .and_then(super::scrolling::centre))
        },
    )?;
    let shot = Shot::take(session, client, 4000)?;
    let pixel = shot.pixel(centre);
    send(
        session,
        client,
        "click",
        json!({"screenshot_ref": shot.id, "x": pixel.0, "y": pixel.1}),
    )?;
    session.wait_until(
        "lease-copied",
        "the button's copy",
        Duration::from_secs(2),
        |session| {
            let copies = fs::read_to_string(session.test_dir().root().join("copies"))
                .context("read the Copy link count")?;
            Ok((copies == "1").then_some(()))
        },
    )
}

fn refused(session: &mut Session<'_>, client: &mut Client, when: &str) -> Result<()> {
    let result: Value = client.call(session, "clipboard_read", json!({}))?;
    if field(&result, "/structuredContent/error") != "lease_required" {
        return Err(Failure::new(format!(
            "Clipboard lease: clipboard_read {when}: {result}, not lease_required"
        )));
    }
    Ok(())
}
