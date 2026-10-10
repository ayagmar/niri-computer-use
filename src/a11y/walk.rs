//! The bounded walk of one window's accessible tree, over any source of nodes. It visits
//! nodes depth first in document order, goes into a node's children only while the node
//! is showing, and stops after a fixed number of nodes, once it has found more nodes than
//! were asked for, or when the request's time runs out, so a huge or hidden tree can't
//! hold the request.

use std::future::Future;

use serde::Serialize;

use super::model::{Extents, States};
use crate::error::{ErrorName, ToolError};

/// One accessible object, as the walk read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Node {
    pub(crate) path: String,
    pub(crate) role: u32,
    /// Data from the app: never logged.
    pub(crate) name: String,
    pub(crate) states: States,
    /// None without a Component interface.
    pub(crate) extents: Option<Extents>,
    pub(crate) actions: Vec<String>,
    /// The object paths of its children in the same application.
    pub(crate) children: Vec<String>,
}

/// Where the walk reads nodes from.
pub(crate) trait Source: Sync {
    /// The node at `path`, or `None` when it no longer exists, as when a widget went away
    /// during the walk.
    fn node(&self, path: &str) -> impl Future<Output = Result<Option<Node>, ToolError>> + Send;
}

/// Why a walk ended with nodes left to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Capped {
    /// It read as many nodes as it may.
    NodeCap,
    /// The request's time ran out after the first node.
    BudgetExhausted,
}

/// The nodes under a root, in document order, without the root.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Walked {
    pub(crate) nodes: Vec<Node>,
    /// Set when the walk stopped early with nodes left that might have matched.
    pub(crate) capped: Option<Capped>,
}

/// What the walk is looking for: nodes for which `matches` holds, and how many of them
/// are wanted. The walk stops once it found one more than `wanted`, which is enough to
/// know there are more.
pub(crate) struct Want<F> {
    pub(crate) wanted: usize,
    pub(crate) matches: F,
}

/// Reads at most `cap` nodes under `root`, depth first, going into the children of
/// showing nodes only, and stops early once `want` has more than it asked for. A node that
/// is gone by the time it is read is skipped. A deadline after the first node ends the
/// walk with the nodes read so far, `BudgetExhausted`; at the first node it is the error,
/// as for a hung app. Any other failure ends the walk with it.
pub(crate) async fn walk<F: Fn(&Node) -> bool + Sync>(
    source: &impl Source,
    root: &Node,
    cap: usize,
    want: Want<F>,
) -> Result<Walked, ToolError> {
    let mut walked = Walked::default();
    let mut found = 0;
    let mut pending: Vec<String> = root.children.iter().rev().cloned().collect();
    while let Some(path) = pending.pop() {
        if walked.nodes.len() == cap {
            walked.capped = Some(Capped::NodeCap);
            break;
        }
        let node = match source.node(&path).await {
            Ok(Some(node)) => node,
            Ok(None) => continue,
            Err(error) if error.name == ErrorName::DeadlineExceeded && !walked.nodes.is_empty() => {
                walked.capped = Some(Capped::BudgetExhausted);
                break;
            }
            Err(error) => return Err(error),
        };
        if node.states.has(super::model::State::Showing) {
            pending.extend(node.children.iter().rev().cloned());
        }
        found += usize::from((want.matches)(&node));
        walked.nodes.push(node);
        if found > want.wanted {
            break;
        }
    }
    Ok(walked)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::error::ErrorName;

    const SHOWING: u32 = (1 << 25) | (1 << 30);

    /// An in-memory tree: path → (showing, children). Missing paths are gone; `/hung`
    /// fails as a hung app or a spent budget would, and `/broken` with a D-Bus error.
    struct Tree(BTreeMap<&'static str, (bool, Vec<&'static str>)>);

    impl Source for Tree {
        fn node(&self, path: &str) -> impl Future<Output = Result<Option<Node>, ToolError>> + Send {
            let failure = match path {
                "/hung" => Some(ErrorName::DeadlineExceeded),
                "/broken" => Some(ErrorName::UpstreamError),
                _ => None,
            };
            if let Some(name) = failure {
                return std::future::ready(Err(ToolError::new(name, path)));
            }
            std::future::ready(Ok(self.0.get(path).map(|(showing, children)| Node {
                path: path.to_owned(),
                role: 43,
                name: String::new(),
                states: States::from_words(&[if *showing { SHOWING } else { 0 }]),
                extents: None,
                actions: Vec::new(),
                children: children.iter().map(|&child| child.to_owned()).collect(),
            })))
        }
    }

    fn root(children: &[&str]) -> Node {
        Node {
            path: "/root".to_owned(),
            role: 23,
            name: String::new(),
            states: States::from_words(&[SHOWING]),
            extents: None,
            actions: Vec::new(),
            children: children.iter().map(|&child| child.to_owned()).collect(),
        }
    }

    fn paths(walked: &Walked) -> Vec<&str> {
        walked.nodes.iter().map(|node| node.path.as_str()).collect()
    }

    fn everything() -> Want<fn(&Node) -> bool> {
        Want {
            wanted: usize::MAX,
            matches: |_| true,
        }
    }

    #[tokio::test]
    async fn visits_depth_first_in_document_order_and_skips_hidden_subtrees() {
        let tree = Tree(BTreeMap::from([
            ("/a", (true, vec!["/a/1", "/a/2"])),
            ("/a/1", (true, vec![])),
            ("/a/2", (true, vec![])),
            ("/hidden", (false, vec!["/hidden/1"])),
            ("/hidden/1", (true, vec![])),
            ("/b", (true, vec![])),
        ]));
        let walked = walk(
            &tree,
            &root(&["/a", "/gone", "/hidden", "/b"]),
            2000,
            everything(),
        )
        .await
        .unwrap();
        assert_eq!(paths(&walked), ["/a", "/a/1", "/a/2", "/hidden", "/b"]);
        assert_eq!(walked.capped, None);
    }

    #[tokio::test]
    async fn stops_at_the_node_cap_and_says_so() {
        let tree = Tree(BTreeMap::from([
            ("/a", (true, vec![])),
            ("/b", (true, vec![])),
            ("/c", (true, vec![])),
        ]));
        let walked = walk(&tree, &root(&["/a", "/b", "/c"]), 2, everything())
            .await
            .unwrap();
        assert_eq!(paths(&walked), ["/a", "/b"]);
        assert_eq!(walked.capped, Some(Capped::NodeCap));
        let exact = walk(&tree, &root(&["/a", "/b"]), 2, everything())
            .await
            .unwrap();
        assert_eq!(exact.capped, None);
    }

    #[tokio::test]
    async fn stops_once_it_found_more_than_wanted() {
        let tree = Tree(BTreeMap::from([
            ("/a", (true, vec!["/a/1"])),
            ("/a/1", (true, vec![])),
            ("/b", (false, vec![])),
            ("/c", (false, vec![])),
            ("/d", (false, vec![])),
        ]));
        // Hidden nodes are the ones wanted here; two are asked for, so the third ends it.
        let hidden = Want {
            wanted: 2,
            matches: |node: &Node| !node.states.has(super::super::model::State::Showing),
        };
        let walked = walk(&tree, &root(&["/a", "/b", "/c", "/d", "/e"]), 2000, hidden)
            .await
            .unwrap();
        assert_eq!(paths(&walked), ["/a", "/a/1", "/b", "/c", "/d"]);
        assert_eq!(walked.capped, None);
    }

    #[tokio::test]
    async fn a_deadline_after_the_first_node_keeps_what_was_read() {
        let tree = Tree(BTreeMap::from([
            ("/a", (true, vec!["/a/1"])),
            ("/a/1", (true, vec![])),
        ]));
        let walked = walk(&tree, &root(&["/a", "/hung", "/b"]), 2000, everything())
            .await
            .unwrap();
        assert_eq!(paths(&walked), ["/a", "/a/1"]);
        assert_eq!(walked.capped, Some(Capped::BudgetExhausted));
    }

    #[tokio::test]
    async fn a_deadline_at_the_first_node_is_a_hung_app() {
        let tree = Tree(BTreeMap::new());
        let error = walk(&tree, &root(&["/gone", "/hung", "/a"]), 2000, everything())
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
    }

    #[tokio::test]
    async fn any_other_failure_ends_the_walk_with_its_error() {
        let tree = Tree(BTreeMap::from([("/a", (true, vec!["/broken"]))]));
        let error = walk(&tree, &root(&["/a"]), 2000, everything())
            .await
            .unwrap_err();
        assert_eq!(error.name, ErrorName::UpstreamError);
    }
}
