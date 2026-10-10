//! The bounded walk of one window's accessible tree, over any source of nodes. It visits
//! nodes depth first in document order, goes into a node's children only while the node
//! is showing, and stops after a fixed number of nodes, so a huge or hidden tree can't
//! hold the request.

use std::future::Future;

use super::model::{Extents, States};
use crate::error::ToolError;

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

/// The nodes under a root, in document order, without the root.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Walked {
    pub(crate) nodes: Vec<Node>,
    /// The walk stopped at the node cap with nodes left to read.
    pub(crate) capped: bool,
}

/// Reads at most `cap` nodes under `root`, depth first, going into the children of
/// showing nodes only. A node that is gone by the time it is read is skipped; any other
/// failure ends the walk with it.
pub(crate) async fn walk(
    source: &impl Source,
    root: &Node,
    cap: usize,
) -> Result<Walked, ToolError> {
    let mut walked = Walked::default();
    let mut pending: Vec<String> = root.children.iter().rev().cloned().collect();
    while let Some(path) = pending.pop() {
        if walked.nodes.len() == cap {
            walked.capped = true;
            break;
        }
        let Some(node) = source.node(&path).await? else {
            continue;
        };
        if node.states.has(super::model::State::Showing) {
            pending.extend(node.children.iter().rev().cloned());
        }
        walked.nodes.push(node);
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
    /// fails as a hung app would.
    struct Tree(BTreeMap<&'static str, (bool, Vec<&'static str>)>);

    impl Source for Tree {
        fn node(&self, path: &str) -> impl Future<Output = Result<Option<Node>, ToolError>> + Send {
            if path == "/hung" {
                return std::future::ready(Err(ToolError::new(
                    ErrorName::DeadlineExceeded,
                    "hung",
                )));
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
        let walked = walk(&tree, &root(&["/a", "/gone", "/hidden", "/b"]), 2000)
            .await
            .unwrap();
        assert_eq!(paths(&walked), ["/a", "/a/1", "/a/2", "/hidden", "/b"]);
        assert!(!walked.capped);
    }

    #[tokio::test]
    async fn stops_at_the_node_cap_and_says_so() {
        let tree = Tree(BTreeMap::from([
            ("/a", (true, vec![])),
            ("/b", (true, vec![])),
            ("/c", (true, vec![])),
        ]));
        let walked = walk(&tree, &root(&["/a", "/b", "/c"]), 2).await.unwrap();
        assert_eq!(paths(&walked), ["/a", "/b"]);
        assert!(walked.capped);
        let exact = walk(&tree, &root(&["/a", "/b"]), 2).await.unwrap();
        assert!(!exact.capped);
    }

    #[tokio::test]
    async fn a_failing_node_ends_the_walk_with_its_error() {
        let tree = Tree(BTreeMap::from([("/a", (true, vec!["/hung"]))]));
        let error = walk(&tree, &root(&["/a"]), 2000).await.unwrap_err();
        assert_eq!(error.name, ErrorName::DeadlineExceeded);
    }
}
