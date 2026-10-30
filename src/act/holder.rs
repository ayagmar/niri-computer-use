//! What holds the keyboard when no window has it. niri reports no window focused while a
//! layer-shell surface, such as a shell panel, has keyboard focus, and a window can't take
//! it back until that surface lets go. A timeout or a refusal then says what niri and
//! Noctalia show, so the agent can close the surface instead of trying again.

use niri_ipc::{Layer, LayerSurface, LayerSurfaceKeyboardInteractivity};

use super::Niri;
use crate::{niri, noctalia};

/// At most this many surfaces are named.
const NAMED: usize = 4;

/// What may hold the keyboard, from niri's layer-shell surfaces and, when Noctalia is
/// installed, its open panel; `None` when neither shows anything. A failed read counts as
/// nothing seen: Noctalia not answering never names a panel.
pub(crate) async fn keyboard_holder(niri: Niri<'_>) -> Option<String> {
    let panel = async {
        if !niri.locks.noctalia {
            return None;
        }
        let status = noctalia::status(niri.locks.env).await.ok()?;
        noctalia::active_panel(&status).ok().flatten()
    };
    let (layers, panel) = tokio::join!(niri::layers(niri.socket), panel);
    holder(&layers.unwrap_or_default(), panel.as_deref())
}

/// Says that Noctalia's `panel`, or the layer-shell surfaces among `layers` that take
/// keyboard focus, may hold it, and how to let it go.
pub(crate) fn holder(layers: &[LayerSurface], panel: Option<&str>) -> Option<String> {
    let taking: Vec<&LayerSurface> = layers
        .iter()
        .filter(|layer| layer.keyboard_interactivity != LayerSurfaceKeyboardInteractivity::None)
        .collect();
    let named = || {
        let mut named: Vec<String> = taking.iter().take(NAMED).map(|layer| name(layer)).collect();
        if taking.len() > NAMED {
            named.push(format!("{} more", taking.len() - NAMED));
        }
        named.join(", ")
    };
    match (panel, taking.is_empty()) {
        (Some(panel), _) => Some(format!(
            "Noctalia's {panel:?} panel is open and may hold keyboard focus; close it with `shell_panel`, or press `Escape` with `key` and `expect: \"none\"`"
        )),
        (None, false) => Some(format!(
            "a layer-shell surface may hold keyboard focus: niri lists {}; press `Escape` with `key` and `expect: \"none\"` to close it, or `shell_panel` for a Noctalia panel",
            named()
        )),
        (None, true) => None,
    }
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
    fn names_the_open_panel_or_the_surfaces_that_take_the_keyboard() {
        use LayerSurfaceKeyboardInteractivity::{Exclusive, None as Never, OnDemand};
        let bar = layer("noctalia-bar", Never);
        assert_eq!(
            holder(std::slice::from_ref(&bar), None),
            None,
            "a bar never takes it"
        );
        assert_eq!(holder(&[], None), None);
        let panel = holder(std::slice::from_ref(&bar), Some("control-center")).unwrap();
        assert!(
            panel.starts_with("Noctalia's \"control-center\" panel is open"),
            "{panel}"
        );
        assert!(panel.contains("`shell_panel`") && panel.contains("Escape"));
        let launcher = layer("fuzzel", Exclusive);
        let held = holder(&[bar, launcher, layer("notes", OnDemand)], None).unwrap();
        assert!(
            held.contains(
                "niri lists \"fuzzel\" (top layer, exclusive keyboard, on DP-1), \"notes\" (top layer, on_demand keyboard, on DP-1);"
            ),
            "{held}"
        );
        assert!(!held.contains("noctalia-bar"), "{held}");
        let many: Vec<LayerSurface> = (0..6).map(|n| layer(&n.to_string(), Exclusive)).collect();
        assert!(holder(&many, None).unwrap().contains(", 2 more;"));
    }
}
