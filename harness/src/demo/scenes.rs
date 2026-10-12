//! The scripted scenes. Each is what an agent would call for the request in its note, at a
//! pace a viewer can follow: every call holds still afterwards so its effect is on screen.

use std::time::Duration;

use serde_json::{Value, json};

use super::{Desktop, Director, Scene};
use crate::failure::{Failure, Result};
use crate::mcp::field;

const SHORT: Duration = Duration::from_millis(1200);
const MEDIUM: Duration = Duration::from_millis(1700);
const LONG: Duration = Duration::from_millis(2300);

pub(super) fn play(
    director: &mut Director<'_, '_>,
    scene: Scene,
    desktop: &Desktop,
    server: &str,
) -> Result<()> {
    match scene {
        Scene::Errand => errand(director, desktop),
        Scene::Layout => layout(director, desktop),
        Scene::Keyboard => keyboard(director, desktop),
        Scene::Stop => stop(director, desktop, server),
    }
}

fn errand(director: &mut Director<'_, '_>, desktop: &Desktop) -> Result<()> {
    let chat = desktop.chat;
    let expect = json!({"window_id": chat});
    director.note("Tell #release the review moved to 3pm, then put me back where I was.")?;
    director.ok("acquire_desktop", &json!({}), SHORT)?;
    director.ok("desktop_state", &json!({}), SHORT)?;
    director.ok("focus", &json!({"window_id": chat}), MEDIUM)?;
    let elements = director.ok("elements", &json!({"window_id": chat}), LONG)?;
    let message = element(&elements, "Message")?;
    let send = element(&elements, "Send")?;
    let text = "The release review moved to 3pm.";
    director.ok(
        "set_element_text",
        &json!({"element": message, "text": text, "expect": expect}),
        MEDIUM,
    )?;
    director.ok(
        "activate_element",
        &json!({"element": send, "expect": expect}),
        LONG,
    )?;
    let released = director.ok("release_desktop", &json!({"restore_focus": true}), LONG)?;
    if field(&released, "/restored/focused_window") != desktop.users_window {
        return Err(Failure::new(format!(
            "demo: focus not back on the user's window: {released}"
        )));
    }
    Ok(())
}

fn layout(director: &mut Director<'_, '_>, desktop: &Desktop) -> Result<()> {
    let chat = desktop.chat;
    let notes = desktop
        .notes
        .ok_or_else(|| Failure::new("the layout scene needs the notes window"))?;
    director.note("Give the chat more room, then show my notes fullscreen for a moment.")?;
    director.ok("acquire_desktop", &json!({}), SHORT)?;
    director.ok("desktop_state", &json!({}), SHORT)?;
    director.ok("focus", &json!({"window_id": chat}), MEDIUM)?;
    let wider = json!({"SetWindowWidth": {"id": chat, "change": {"SetProportion": 75.0}}});
    director.ok("niri_action", &json!({"action": wider}), MEDIUM)?;
    director.ok("focus", &json!({"window_id": notes}), MEDIUM)?;
    let fullscreen = json!({"action": {"FullscreenWindow": {"id": notes}}});
    director.ok("niri_action", &fullscreen, LONG)?;
    director.ok("niri_action", &fullscreen, MEDIUM)?;
    director.ok("release_desktop", &json!({"restore_focus": true}), LONG)?;
    Ok(())
}

fn keyboard(director: &mut Director<'_, '_>, desktop: &Desktop) -> Result<()> {
    let chat = json!({"window_id": desktop.chat});
    let text = "On my way.";
    director.note("Say in #release that I'm on my way.")?;
    director.ok("acquire_desktop", &json!({}), SHORT)?;
    director.refused(
        "type_text",
        &json!({"text": text, "expect": chat, "submit": true}),
        "focus_mismatch",
        LONG,
    )?;
    director.ok("focus", &json!({"window_id": desktop.chat}), MEDIUM)?;
    director.ok(
        "type_text",
        &json!({"text": text, "expect": chat, "submit": true}),
        LONG,
    )?;
    director.ok("release_desktop", &json!({"restore_focus": true}), LONG)?;
    Ok(())
}

fn stop(director: &mut Director<'_, '_>, desktop: &Desktop, server: &str) -> Result<()> {
    let chat = json!({"window_id": desktop.chat});
    director.note("Post the release notes in #release.")?;
    director.ok("acquire_desktop", &json!({}), SHORT)?;
    director.ok("focus", &json!({"window_id": desktop.chat}), MEDIUM)?;
    director.ok(
        "type_text",
        &json!({"text": "Release notes: the stop key now", "expect": chat}),
        SHORT,
    )?;
    director.user(server, &["stop"], SHORT)?;
    director.refused(
        "type_text",
        &json!({"text": " takes the desktop back.", "expect": chat}),
        "stopped",
        LONG,
    )
}

/// The `element_ref` of the element named `name` in an `elements` result.
fn element(elements: &Value, name: &str) -> Result<Value> {
    field(elements, "/elements")
        .as_array()
        .into_iter()
        .flatten()
        .find(|element| field(element, "/name") == name)
        .map(|element| field(element, "/element_ref").clone())
        .filter(Value::is_string)
        .ok_or_else(|| Failure::new(format!("demo: no element named {name} in {elements}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_an_element_ref_by_its_exact_name() {
        let elements = json!({"elements": [
            {"name": "Message history", "element_ref": "elem-1"},
            {"name": "Message", "element_ref": "elem-2"},
            {"name": "Send", "element_ref": null},
        ]});
        assert_eq!(element(&elements, "Message").unwrap(), "elem-2");
        assert!(element(&elements, "Send").is_err());
        assert!(element(&elements, "Close").is_err());
    }
}
