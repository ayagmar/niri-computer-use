//! `elements`' work. niri says where the window is, right when asked; the accessibility
//! bus says what is in it and where relative to the window (see `a11y::model`). Names are
//! the app's data: they are returned, never logged.

use std::path::Path;

use niri_ipc::Window;
use serde::Serialize;

use crate::a11y::model::{self, Extents, Filter, LayoutBox, Placement, Unmappable};
use crate::a11y::{self, A11y};
use crate::coords::LayoutPt;
use crate::error::{CallError, ToolError};
use crate::niri;
use crate::policy::{self, Loaded};

/// What `elements` was asked for.
#[derive(Debug, Clone)]
pub(crate) struct Ask {
    pub(crate) window_id: u64,
    pub(crate) filter: Filter,
    pub(crate) limit: usize,
}

/// What `elements` returns.
#[derive(Debug, Serialize)]
pub(crate) struct Listing {
    pub(crate) window_id: u64,
    pub(crate) elements: Vec<Listed>,
    /// More elements matched than `limit`.
    pub(crate) truncated: bool,
    /// How many accessible objects the walk read.
    pub(crate) walked: usize,
    /// The walk stopped at its node cap, so elements further on are missing.
    pub(crate) capped: bool,
}

/// One element.
#[derive(Debug, Serialize)]
pub(crate) struct Listed {
    pub(crate) role: &'static str,
    /// The app's text: untrusted data.
    pub(crate) name: String,
    pub(crate) states: Vec<&'static str>,
    pub(crate) actions: Vec<String>,
    pub(crate) layout_box: Option<LayoutBox>,
    pub(crate) unmappable: Option<Unmappable>,
}

/// A window as niri reports it right now, and where its geometry starts in the layout.
#[derive(Debug)]
struct Placed {
    window: Window,
    origin: Option<LayoutPt>,
}

/// niri's window `id` right now, with its place in the layout, from fresh requests.
async fn placed(socket: Option<&Path>, id: u64) -> Result<Option<Placed>, ToolError> {
    let (windows, workspaces, outputs) = tokio::join!(
        niri::windows(socket),
        niri::workspaces(socket),
        niri::outputs(socket)
    );
    let Some(window) = windows?.into_iter().find(|window| window.id == id) else {
        return Ok(None);
    };
    let output = window
        .workspace_id
        .and_then(|workspace| workspaces.ok()?.into_iter().find(|w| w.id == workspace))
        .and_then(|workspace| workspace.output)
        .and_then(|name| outputs.ok()?.remove(&name)?.logical);
    let origin = output.and_then(|output| model::window_origin(&output, &window.layout));
    Ok(Some(Placed { window, origin }))
}

/// Lists the accessible elements of window `ask.window_id`.
pub(crate) async fn list(
    socket: Option<&Path>,
    a11y: &A11y,
    policy: &Loaded,
    ask: &Ask,
) -> Result<Listing, CallError> {
    let placed = placed(socket, ask.window_id).await?.ok_or_else(|| {
        CallError::InvalidArguments(format!("no window with id {}", ask.window_id))
    })?;
    let window = &placed.window;
    if let Some(refused) = policy::refuse_window(policy, window.id, window.app_id.as_deref()) {
        return Err(refused.into());
    }
    let pid = window.pid.ok_or_else(|| {
        a11y::not_accessible(format!("niri knows no process for window {}", window.id))
    })?;
    let request = a11y.request(a11y::BUDGET).await?;
    let app = request.app(pid).await?;
    let frame = request
        .frame(&app, window.layout.window_size, window.title.as_deref())
        .await?;
    let walked = request.walk(&app.bus, &frame.node).await?;
    let mut matching = walked.nodes.iter().filter(|node| {
        let role = model::role_name(node.role);
        node.states.has(model::State::Showing)
            && (ask.filter.role.is_some() || !node.name.is_empty() || !node.actions.is_empty())
            && ask.filter.matches(role, &node.name)
    });
    let mut elements = Vec::new();
    for node in matching.by_ref().take(ask.limit) {
        let placement = Placement {
            states: node.states,
            extents: node.extents.unwrap_or(Extents {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }),
            frame_fits: frame.fits,
            origin: placed.origin,
        };
        let placed_box = model::place(placement);
        elements.push(Listed {
            role: model::role_name(node.role),
            name: node.name.clone(),
            states: node.states.names(),
            actions: node.actions.clone(),
            layout_box: placed_box.ok(),
            unmappable: placed_box.err(),
        });
    }
    Ok(Listing {
        window_id: window.id,
        truncated: matching.next().is_some(),
        elements,
        walked: walked.nodes.len(),
        capped: walked.capped,
    })
}
