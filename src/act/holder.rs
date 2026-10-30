//! What holds the keyboard when no window has it. niri reports no window focused while its
//! overview is open or a layer-shell surface, such as a shell panel, has keyboard focus,
//! and a window can't take it back until that lets go. A timeout or a refusal then says what niri and
//! Noctalia show, so the agent can close the surface instead of trying again.

use std::time::Duration;

use niri_ipc::{Layer, LayerSurface, LayerSurfaceKeyboardInteractivity};

use super::Niri;
use crate::{niri, noctalia};

/// At most this many surfaces are named.
const NAMED: usize = 4;
/// How long the reads behind a hint may take together. A refusal or a timeout is returned
/// with no hint when they take longer, so a hung Noctalia doesn't hold it up.
const BUDGET: Duration = Duration::from_millis(300);

/// How to close what holds the keyboard with a key.
const ESCAPE: &str = "press `Escape` with `key` and `expect: \"none\"`";

/// What may hold the keyboard, from niri's overview and layer-shell surfaces and, when
/// Noctalia is installed, its open panel; `None` when none shows anything. A failed read,
/// or reads that take longer than `BUDGET`, count as nothing seen: Noctalia not answering
/// never names a panel.
pub(crate) async fn keyboard_holder(niri: Niri<'_>) -> Option<String> {
    let panel = async {
        if !niri.locks.noctalia {
            return None;
        }
        let status = noctalia::status(niri.locks.env).await.ok()?;
        noctalia::active_panel(&status).ok().flatten()
    };
    let seen = async {
        let (overview, layers, panel) = tokio::join!(
            niri::overview_open(niri.socket),
            niri::layers(niri.socket),
            panel
        );
        holder(
            overview.unwrap_or(false),
            &layers.unwrap_or_default(),
            panel.as_deref(),
        )
    };
    tokio::time::timeout(BUDGET, seen).await.ok().flatten()
}

/// Says that niri's overview, Noctalia's `panel`, or the layer-shell surfaces among
/// `layers` that take keyboard focus may hold it, and how to let it go: the overview
/// first, then the panel, or else the surfaces with exclusive keyboard focus, or else,
/// more weakly, the on-demand ones, which niri gives the keyboard only once clicked.
pub(crate) fn holder(
    overview: bool,
    layers: &[LayerSurface],
    panel: Option<&str>,
) -> Option<String> {
    let overview = overview.then_some(
        "niri's overview is open and holds keyboard focus; close it with `niri_action` `CloseOverview`".to_owned(),
    );
    let held = panel.map_or_else(
        || surfaces(layers),
        |panel| Some(format!(
            "Noctalia's {panel:?} panel is open and may hold keyboard focus; close it with `shell_panel`, or {ESCAPE}"
        )),
    );
    let said: Vec<String> = [overview, held].into_iter().flatten().collect();
    (!said.is_empty()).then(|| said.join("; "))
}

/// The layer-shell surfaces among `layers` that may hold the keyboard, as `holder` says.
fn surfaces(layers: &[LayerSurface]) -> Option<String> {
    use LayerSurfaceKeyboardInteractivity::{Exclusive, OnDemand};
    let with = |keyboard| -> Vec<&LayerSurface> {
        layers
            .iter()
            .filter(|layer| layer.keyboard_interactivity == keyboard)
            .collect()
    };
    let (exclusive, on_demand) = (with(Exclusive), with(OnDemand));
    if !exclusive.is_empty() {
        return Some(format!(
            "a layer-shell surface with exclusive keyboard focus holds it: niri lists {}; {ESCAPE} to close it, or `shell_panel` for a Noctalia panel",
            named(&exclusive)
        ));
    }
    (!on_demand.is_empty()).then(|| {
        format!(
            "if you clicked one, a layer-shell surface may hold keyboard focus: niri lists {}; {ESCAPE} to close it, or `shell_panel` for a Noctalia panel",
            named(&on_demand)
        )
    })
}

/// The first `NAMED` of `surfaces`, and how many more there are.
fn named(surfaces: &[&LayerSurface]) -> String {
    let mut named: Vec<String> = surfaces
        .iter()
        .take(NAMED)
        .map(|layer| name(layer))
        .collect();
    if surfaces.len() > NAMED {
        named.push(format!("{} more", surfaces.len() - NAMED));
    }
    named.join(", ")
}

/// A surface as niri lists it: its namespace, layer, keyboard mode and output.
fn name(layer: &LayerSurface) -> String {
    let level = match layer.layer {
        Layer::Background => "background",
        Layer::Bottom => "bottom",
        Layer::Top => "top",
        Layer::Overlay => "overlay",
    };
    let keyboard = match layer.keyboard_interactivity {
        LayerSurfaceKeyboardInteractivity::Exclusive => "exclusive",
        LayerSurfaceKeyboardInteractivity::OnDemand => "on_demand",
        LayerSurfaceKeyboardInteractivity::None => "none",
    };
    format!(
        "{:?} ({level} layer, {keyboard} keyboard, on {})",
        layer.namespace, layer.output
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(namespace: &str, keyboard: LayerSurfaceKeyboardInteractivity) -> LayerSurface {
        LayerSurface {
            namespace: namespace.to_owned(),
            output: "DP-1".to_owned(),
            layer: Layer::Top,
            keyboard_interactivity: keyboard,
        }
    }

    #[test]
    fn names_the_overview_the_open_panel_or_the_surfaces_that_take_the_keyboard() {
        use LayerSurfaceKeyboardInteractivity::{Exclusive, None as Never, OnDemand};
        let bar = layer("noctalia-bar", Never);
        assert_eq!(
            holder(false, std::slice::from_ref(&bar), None),
            None,
            "a bar never takes it"
        );
        assert_eq!(holder(false, &[], None), None);
        let panel = holder(false, std::slice::from_ref(&bar), Some("control-center")).unwrap();
        assert!(
            panel.starts_with("Noctalia's \"control-center\" panel is open"),
            "{panel}"
        );
        assert!(panel.contains("`shell_panel`") && panel.contains("Escape"));
        let notes = layer("notes", OnDemand);
        let held = holder(
            false,
            &[bar.clone(), layer("fuzzel", Exclusive), notes.clone()],
            None,
        )
        .unwrap();
        assert!(
            held.starts_with(
                "a layer-shell surface with exclusive keyboard focus holds it: niri lists \"fuzzel\" (top layer, exclusive keyboard, on DP-1);"
            ),
            "{held}"
        );
        assert!(
            !held.contains("noctalia-bar") && !held.contains("notes"),
            "{held}"
        );
        let many: Vec<LayerSurface> = (0..6).map(|n| layer(&n.to_string(), Exclusive)).collect();
        assert!(holder(false, &many, None).unwrap().contains(", 2 more;"));
        // An on-demand surface takes the keyboard only once clicked, so it is named only
        // as a maybe, and the overview, which also leaves no window focused, comes first.
        let clicked = holder(false, &[bar, notes.clone()], None).unwrap();
        assert!(
            clicked.starts_with("if you clicked one, a layer-shell surface may hold keyboard focus: niri lists \"notes\""),
            "{clicked}"
        );
        let overview = holder(true, &[notes], Some("launcher")).unwrap();
        assert!(
            overview.starts_with("niri's overview is open and holds keyboard focus; close it with `niri_action` `CloseOverview`; Noctalia's \"launcher\" panel"),
            "{overview}"
        );
        assert_eq!(
            holder(true, &[], None).as_deref(),
            Some(
                "niri's overview is open and holds keyboard focus; close it with `niri_action` `CloseOverview`"
            )
        );
    }
}
