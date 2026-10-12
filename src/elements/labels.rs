//! Refs bound through a label the app names. An app may say, through `labelled-by` and
//! `label-for` relations, which label labels a control, as Qt Quick's
//! `Accessible.labelledBy` does. A label binds the control's ref to its record only when
//! it labels the control alone, sits outside it and has a name. Where the walk left the
//! control unbound, among lookalikes, the label must also tell it apart from them: another
//! lookalike's label must differ, and none may have the same name. A static caption such
//! as `Remove` in every row tells no record apart, so a row reused in place would pass.
//! The name kept is the one the walk read, which the listing shows; acting reads it again
//! (`identify`).

use std::collections::{HashMap, HashSet};

use crate::a11y::model::{Label, NameHash, Row};
use crate::a11y::{self, Walked};
use crate::error::{ErrorName, ToolError};

/// The labels read for the walked nodes, by their index in the walk.
pub(super) struct Labels {
    /// Each node read, with its label if it has one.
    read: HashMap<usize, Option<Label>>,
    /// Set when the request's budget ran out before every node needed was read.
    pub(super) cut: bool,
}

impl Labels {
    /// Reads the labels that binding the nodes at `listed` needs: each node's, and for
    /// one the walk left unbound, its lookalikes' too. Stops, marked `cut`, once the
    /// request's budget is spent.
    pub(super) async fn read(
        request: &a11y::Request,
        bus: &str,
        walked: &Walked,
        listed: &[usize],
    ) -> Result<Self, ToolError> {
        let paths: HashMap<&str, usize> = walked
            .nodes
            .iter()
            .enumerate()
            .map(|(at, node)| (node.path.as_str(), at))
            .collect();
        let mut labels = Self {
            read: HashMap::new(),
            cut: false,
        };
        for at in needed(walked, listed) {
            if request.spent() {
                labels.cut = true;
                break;
            }
            match label(request, bus, walked, &paths, at).await {
                Ok(label) => {
                    labels.read.insert(at, label);
                }
                Err(error) if error.name == ErrorName::DeadlineExceeded && request.spent() => {
                    labels.cut = true;
                    break;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(labels)
    }

    /// How a ref to the node at `at`, which the walk binds as `row`, is bound once its
    /// label is counted. A named row keeps its own name; a ref the lineage binds gains its
    /// label's check; one the walk left unbound is bound only by a label that tells it
    /// apart from its lookalikes.
    pub(super) fn bind(&self, walked: &Walked, at: usize, row: Row) -> Row {
        let Some(Some(label)) = self.read.get(&at) else {
            return row;
        };
        match row {
            Row::Lineage => Row::Labelled(label.clone()),
            Row::Unbound(_) if self.tells_apart(walked, at, label) => Row::Labelled(label.clone()),
            Row::Own | Row::Labelled(_) | Row::Unbound(_) => row,
        }
    }

    /// Whether `label` tells the node at `at` apart from its lookalikes: every one was
    /// read, at least one has a label, which shows that their labels vary, and none has a
    /// label of the same name. A lookalike with no label, such as a toolbar's button of
    /// the same name, says nothing either way.
    fn tells_apart(&self, walked: &Walked, at: usize, label: &Label) -> bool {
        let mut told = false;
        for other in walked.lookalikes(at) {
            match self.read.get(&other) {
                _ if other == at => {}
                Some(Some(theirs)) if theirs.name == label.name => return false,
                Some(Some(_)) => told = true,
                Some(None) => {}
                None => return false,
            }
        }
        told
    }
}

/// The nodes whose labels binding the nodes at `listed` needs, in that order, each once:
/// every listed node but a named row, and the lookalikes of each the walk left unbound.
fn needed(walked: &Walked, listed: &[usize]) -> Vec<usize> {
    let mut seen = HashSet::new();
    let mut needed = Vec::new();
    for &at in listed {
        let more = if matches!(walked.row(at), Row::Unbound(_)) {
            walked.lookalikes(at)
        } else {
            Vec::new()
        };
        for node in std::iter::once(at).chain(more) {
            if walked.row(node) != Row::Own && seen.insert(node) {
                needed.push(node);
            }
        }
    }
    needed
}

/// The label of the node at `at`, from its relations read now and the walk: the one label
/// that labels it alone (`a11y::Request::label`), when the walk read that label outside
/// the node, with a name, which is the name kept.
async fn label(
    request: &a11y::Request,
    bus: &str,
    walked: &Walked,
    paths: &HashMap<&str, usize>,
    at: usize,
) -> Result<Option<Label>, ToolError> {
    let Some(node) = walked.nodes.get(at) else {
        return Ok(None);
    };
    let Some(path) = request.label((bus, &node.path)).await? else {
        return Ok(None);
    };
    let Some(&read) = paths.get(path.as_str()) else {
        return Ok(None);
    };
    if walked.under(read, at) {
        return Ok(None);
    }
    let name = walked.nodes.get(read).map(|label| label.name.as_str());
    Ok(name
        .filter(|name| !name.trim().is_empty())
        .map(|name| Label {
            path,
            name: NameHash::of(name),
        }))
}
