//! The bounded walk of one window's accessible tree, over any source of nodes. It visits
//! nodes depth first in document order, goes into a node's children only while the node
//! is showing, and stops after a fixed number of nodes, `REACH` nodes after it has found
//! more matching nodes than were asked for, or when the request's budget runs out, so a
//! huge or hidden tree can't hold the request.

use std::collections::HashMap;
use std::future::Future;
use std::hash::{DefaultHasher, Hash, Hasher};

use serde::Serialize;

use super::model::{Extents, Link, NameHash, States};
use crate::error::{ErrorName, ToolError};

mod row;

/// How many nodes the walk reads past the match that stops it, so the elements it lists
/// have their look-alike elements nearby read, as the next row of a list is (see `row`).
pub(crate) const REACH: usize = 64;

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
    /// Its children in the same application.
    pub(crate) children: Vec<Child>,
}

/// A child of a node: its object path, and its index among all of the node's children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Child {
    pub(crate) index: i32,
    pub(crate) path: String,
}

/// Where the walk reads nodes from.
pub(crate) trait Source: Sync {
    /// The node at `path`, or `None` when it no longer exists, as when a widget went away
    /// during the walk.
    fn node(&self, path: &str) -> impl Future<Output = Result<Option<Node>, ToolError>> + Send;

    /// Whether the request's whole budget is used up, as opposed to one call's deadline.
    fn spent(&self) -> bool;
}

/// Why a walk ended with nodes left to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Capped {
    /// It read as many nodes as it may.
    NodeCap,
    /// The request's whole budget ran out.
    BudgetExhausted,
}

/// The nodes under a root, in document order, without the root.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Walked {
    pub(crate) nodes: Vec<Node>,
    /// Where each node of `nodes` sits: its parent among them, none under the root, and
    /// its index among the parent's children.
    places: Vec<(Option<usize>, i32)>,
    /// Where each node's subtree ends in `nodes`: the index past its last descendant.
    ends: Vec<usize>,
    /// Whether the walk left something under each node unread.
    open: Vec<bool>,
    /// Whether the walk read everything under the root that is showing.
    complete: bool,
    /// The nodes of each role and name, in document order, and which of them each node
    /// is in: a node's look-alike elements.
    alike: Vec<Vec<usize>>,
    alike_of: Vec<usize>,
    /// A hash of each node's subtree's roles and shape, never of its names, so rows of
    /// one list compare equal while showing different records.
    shapes: Vec<u64>,
    /// Set when the walk stopped early with nodes left that might have matched.
    pub(crate) capped: Option<Capped>,
}

impl Walked {
    /// Each node from the root's child down to the node at `at`, with its index in its
    /// parent and its name's hash.
    pub(crate) fn lineage(&self, at: usize) -> Vec<Link> {
        let mut chain = Vec::new();
        let mut next = Some(at);
        while let Some(this) = next {
            let (Some(node), Some((parent, index))) = (self.nodes.get(this), self.places.get(this))
            else {
                break;
            };
            chain.push(Link {
                path: node.path.clone(),
                index: *index,
                name: NameHash::of(&node.name),
            });
            next = *parent;
        }
        chain.reverse();
        chain
    }
}

/// What the walk is looking for: nodes for which `matches` holds, and how many of them
/// are wanted. The walk stops once it found one more than `wanted`, which is enough to
/// know there are more.
pub(crate) struct Want<F> {
    pub(crate) wanted: usize,
    pub(crate) matches: F,
}

/// Reads at most `cap` nodes under `root`, depth first, going into the children of
/// showing nodes only, matching or not, and stops `REACH` nodes after `want` has more
/// matches than it asked for. A node that is gone by the time it is read is skipped. A
/// deadline once the request's budget is spent ends the walk with the nodes read so far,
/// `BudgetExhausted`. One call's own deadline, as for a hung app, and any other failure
/// end the walk with that error. The cap or the budget past the extra match leaves
/// `capped` unset: every match the walk was asked for was found.
pub(crate) async fn walk<F: Fn(&Node) -> bool + Sync>(
    source: &impl Source,
    root: &Node,
    cap: usize,
    want: Want<F>,
) -> Result<Walked, ToolError> {
    let mut walked = Walked::default();
    let mut found = 0;
    let mut until = cap;
    let mut pending: Vec<(Option<usize>, Child)> = root
        .children
        .iter()
        .rev()
        .map(|child| (None, child.clone()))
        .collect();
    while let Some((parent, child)) = pending.pop() {
        let past = found > want.wanted;
        if walked.nodes.len() >= until {
            if !past {
                walked.capped = Some(Capped::NodeCap);
            }
            pending.push((parent, child));
            break;
        }
        let node = match source.node(&child.path).await {
            Ok(Some(node)) => node,
            Ok(None) => continue,
            Err(error) if error.name == ErrorName::DeadlineExceeded && source.spent() => {
                if !past {
                    walked.capped = Some(Capped::BudgetExhausted);
                }
                pending.push((parent, child));
                break;
            }
            Err(error) => return Err(error),
        };
        let at = walked.nodes.len();
        if node.states.has(super::model::State::Showing) {
            pending.extend(
                node.children
                    .iter()
                    .rev()
                    .map(|grandchild| (Some(at), grandchild.clone())),
            );
        }
        found += usize::from((want.matches)(&node));
        walked.nodes.push(node);
        walked.places.push((parent, child.index));
        if !past && found > want.wanted {
            until = cap.min(walked.nodes.len() + REACH);
        }
    }
    walked.finish(pending.into_iter().map(|(parent, _)| parent));
    Ok(walked)
}

impl Walked {
    /// What the row checks ask of every node, worked out once from the nodes and the
    /// parents of the children left unread.
    fn finish(&mut self, unread: impl Iterator<Item = Option<usize>>) {
        self.ends = ends(&self.places);
        self.open = vec![false; self.nodes.len()];
        self.complete = true;
        for parent in unread {
            self.complete = false;
            if let Some(open) = parent.and_then(|parent| self.open.get_mut(parent)) {
                *open = true;
            }
        }
        for index in (0..self.nodes.len()).rev() {
            let parent = self.places.get(index).and_then(|(parent, _)| *parent);
            if self.open.get(index) == Some(&true)
                && let Some(open) = parent.and_then(|parent| self.open.get_mut(parent))
            {
                *open = true;
            }
        }
        self.alike_groups();
        self.shapes = shapes(&self.nodes, &self.places);
    }

    fn alike_groups(&mut self) {
        let mut groups: HashMap<(u32, &str), usize> = HashMap::new();
        let mut alike: Vec<Vec<usize>> = Vec::new();
        let mut alike_of = Vec::with_capacity(self.nodes.len());
        for (index, node) in self.nodes.iter().enumerate() {
            let group = *groups.entry((node.role, &node.name)).or_insert_with(|| {
                alike.push(Vec::new());
                alike.len() - 1
            });
            if let Some(members) = alike.get_mut(group) {
                members.push(index);
            }
            alike_of.push(group);
        }
        self.alike = alike;
        self.alike_of = alike_of;
    }
}

/// Each node's shape: its role and its children's shapes, in order. A node's children
/// follow it in document order, so going backwards finds every child's shape first.
fn shapes(nodes: &[Node], places: &[(Option<usize>, i32)]) -> Vec<u64> {
    let mut children: Vec<Vec<u64>> = vec![Vec::new(); nodes.len()];
    let mut shapes = vec![0; nodes.len()];
    for index in (0..nodes.len()).rev() {
        let mut hasher = DefaultHasher::new();
        nodes.get(index).map(|node| node.role).hash(&mut hasher);
        if let Some(mine) = children.get_mut(index) {
            mine.reverse();
            mine.hash(&mut hasher);
        }
        let shape = hasher.finish();
        if let Some(slot) = shapes.get_mut(index) {
            *slot = shape;
        }
        let parent = places.get(index).and_then(|(parent, _)| *parent);
        if let Some(theirs) = parent.and_then(|parent| children.get_mut(parent)) {
            theirs.push(shape);
        }
    }
    shapes
}

/// Where each node's subtree ends, from the nodes' places in document order: a node's
/// descendants follow it, so its subtree ends where its last child's does.
fn ends(places: &[(Option<usize>, i32)]) -> Vec<usize> {
    let mut ends: Vec<usize> = (1..=places.len()).collect();
    for (index, (parent, _)) in places.iter().enumerate().rev() {
        let end = ends.get(index).copied().unwrap_or(index + 1);
        if let Some(parent_end) = parent.and_then(|parent| ends.get_mut(parent)) {
            *parent_end = (*parent_end).max(end);
        }
    }
    ends
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::a11y::model::State;

    const SHOWING: u32 = (1 << 25) | (1 << 30);

    /// An in-memory tree: path → (showing, children), each node named after its path, or
    /// what follows a `#` in it. Missing paths are gone. `/late` times out with the request's budget spent, `/hung`
    /// times out on its own call with budget left, as a hung app does, and `/lost` fails
    /// as a lost bus connection does.
    pub(super) struct Tree {
        nodes: BTreeMap<&'static str, (bool, Vec<&'static str>)>,
        spent: AtomicBool,
    }

    impl Tree {
        pub(super) fn new<const N: usize>(
            nodes: [(&'static str, (bool, Vec<&'static str>)); N],
        ) -> Self {
            Self::of(nodes.into())
        }

        pub(super) fn of(nodes: Vec<(&'static str, (bool, Vec<&'static str>))>) -> Self {
            Self {
                nodes: nodes.into_iter().collect(),
                spent: AtomicBool::new(false),
            }
        }
    }

    impl Source for Tree {
        fn node(&self, path: &str) -> impl Future<Output = Result<Option<Node>, ToolError>> + Send {
            let failure = match path {
                "/late" => {
                    self.spent.store(true, Ordering::Relaxed);
                    Some(ErrorName::DeadlineExceeded)
                }
                "/hung" => Some(ErrorName::DeadlineExceeded),
                "/lost" => Some(ErrorName::UpstreamError),
                _ => None,
            };
            if let Some(name) = failure {
                return std::future::ready(Err(ToolError::new(name, path)));
            }
            std::future::ready(Ok(self.nodes.get(path).map(|(showing, children)| Node {
                path: path.to_owned(),
                role: 43,
                name: path
                    .split_once('#')
                    .map_or_else(|| format!("name of {path}"), |(_, name)| name.to_owned()),
                states: States::from_words(&[if *showing { SHOWING } else { 0 }]),
                extents: None,
                actions: Vec::new(),
                children: kids(children),
            })))
        }

        fn spent(&self) -> bool {
            self.spent.load(Ordering::Relaxed)
        }
    }

    /// Children at their index in the list.
    fn kids(paths: &[&str]) -> Vec<Child> {
        (0..)
            .zip(paths)
            .map(|(index, &path)| Child {
                index,
                path: path.to_owned(),
            })
            .collect()
    }

    pub(super) fn root(children: &[&str]) -> Node {
        Node {
            path: "/root".to_owned(),
            role: 23,
            name: String::new(),
            states: States::from_words(&[SHOWING]),
            extents: None,
            actions: Vec::new(),
            children: kids(children),
        }
    }

    fn paths(walked: &Walked) -> Vec<&str> {
        walked.nodes.iter().map(|node| node.path.as_str()).collect()
    }

    pub(super) fn everything() -> Want<fn(&Node) -> bool> {
        Want {
            wanted: usize::MAX,
            matches: |_| true,
        }
    }

    /// Wants `wanted` showing nodes whose path starts with `prefix`.
    pub(super) fn showing_under(
        prefix: &'static str,
        wanted: usize,
    ) -> Want<impl Fn(&Node) -> bool + Sync> {
        Want {
            wanted,
            matches: move |node: &Node| {
                node.states.has(State::Showing) && node.path.starts_with(prefix)
            },
        }
    }

    #[tokio::test]
    async fn visits_depth_first_in_document_order_and_skips_hidden_subtrees() {
        let tree = Tree::new([
            ("/a", (true, vec!["/a/1", "/a/2"])),
            ("/a/1", (true, vec![])),
            ("/a/2", (true, vec![])),
            ("/hidden", (false, vec!["/hidden/1"])),
            ("/hidden/1", (true, vec![])),
            ("/b", (true, vec![])),
        ]);
        let tree_root = root(&["/a", "/gone", "/hidden", "/b"]);
        let walked = walk(&tree, &tree_root, 2000, everything()).await.unwrap();
        assert_eq!(paths(&walked), ["/a", "/a/1", "/a/2", "/hidden", "/b"]);
        assert_eq!(walked.capped, None);
    }

    #[tokio::test]
    async fn a_nodes_lineage_is_each_ancestors_place_from_the_roots_child_down() {
        let tree = Tree::new([
            ("/a", (true, vec!["/a/1", "/a/2"])),
            ("/a/1", (true, vec![])),
            ("/a/2", (true, vec!["/a/2/x"])),
            ("/a/2/x", (true, vec![])),
            ("/b", (true, vec![])),
        ]);
        let walked = walk(&tree, &root(&["/gone", "/a", "/b"]), 2000, everything())
            .await
            .unwrap();
        let at = |path: &str| walked.nodes.iter().position(|node| node.path == path);
        let place = |path: &str, index| Link {
            path: path.to_owned(),
            index,
            name: NameHash::of(&format!("name of {path}")),
        };
        assert_eq!(
            walked.lineage(at("/a/2/x").unwrap()),
            [place("/a", 1), place("/a/2", 1), place("/a/2/x", 0)]
        );
        // A gone sibling keeps its place in the parent's list.
        assert_eq!(walked.lineage(at("/b").unwrap()), [place("/b", 2)]);
    }

    #[tokio::test]
    async fn stops_at_the_node_cap_however_few_match() {
        let tree = Tree::new([
            ("/a", (true, vec![])),
            ("/b", (true, vec![])),
            ("/c", (true, vec![])),
        ]);
        let walked = walk(&tree, &root(&["/a", "/b", "/c"]), 2, showing_under("/x", 1))
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
    async fn finds_matches_under_parents_that_dont_match_and_reads_on_past_the_limit() {
        let tree = Tree::new([
            ("/a", (true, vec!["/a/1", "/b/1"])),
            ("/a/1", (true, vec![])),
            ("/b/1", (true, vec![])),
            ("/hidden", (false, vec!["/b/hidden"])),
            ("/b/hidden", (true, vec![])),
            ("/b/2", (true, vec![])),
            ("/b/3", (true, vec![])),
            ("/b/4", (true, vec![])),
        ]);
        let tree_root = root(&["/a", "/hidden", "/b/2", "/b/3", "/b/4"]);
        let walked = walk(&tree, &tree_root, 2000, showing_under("/b", 2))
            .await
            .unwrap();
        // /b/1 sits under /a, which doesn't match; /b/hidden is never read. /b/3 is the
        // match past the limit, and /b/4 is read on the way to `REACH` past it.
        assert_eq!(
            paths(&walked),
            ["/a", "/a/1", "/b/1", "/hidden", "/b/2", "/b/3", "/b/4"]
        );
        assert_eq!(walked.capped, None);
    }

    // Past the match that stops it, the walk reads `REACH` more nodes and no further,
    // and running into the cap there doesn't count as cut short.
    #[tokio::test]
    async fn reads_reach_nodes_past_the_extra_match_and_no_further() {
        let names: Vec<&'static str> = (0..REACH + 10)
            .map(|n| &*Box::leak(format!("/n{n:03}").into_boxed_str()))
            .collect();
        let tree = Tree::of(names.iter().map(|&path| (path, (true, vec![]))).collect());
        let walked = walk(&tree, &root(&names), 2000, showing_under("/n000", 0))
            .await
            .unwrap();
        assert_eq!(walked.nodes.len(), 1 + REACH);
        assert_eq!(walked.capped, None);
        let capped = walk(&tree, &root(&names), 10, showing_under("/n000", 0))
            .await
            .unwrap();
        assert_eq!(capped.nodes.len(), 10);
        assert_eq!(capped.capped, None);
    }

    #[tokio::test]
    async fn exactly_the_limit_walks_the_whole_tree() {
        let tree = Tree::new([
            ("/b/1", (true, vec![])),
            ("/a", (true, vec![])),
            ("/b/2", (true, vec![])),
        ]);
        let tree_root = root(&["/b/1", "/a", "/b/2"]);
        let walked = walk(&tree, &tree_root, 2000, showing_under("/b", 2))
            .await
            .unwrap();
        assert_eq!(paths(&walked), ["/b/1", "/a", "/b/2"]);
        assert_eq!(walked.capped, None);
    }

    #[tokio::test]
    async fn a_spent_budget_keeps_what_was_read_before_or_after_a_match() {
        let early = Tree::new([("/b/1", (true, vec![])), ("/a", (true, vec![]))]);
        let before = walk(
            &early,
            &root(&["/late", "/b/1"]),
            2000,
            showing_under("/b", 5),
        )
        .await
        .unwrap();
        assert_eq!(paths(&before), Vec::<&str>::new());
        assert_eq!(before.capped, Some(Capped::BudgetExhausted));
        let late = Tree::new([("/b/1", (true, vec![])), ("/a", (true, vec![]))]);
        let tree_root = root(&["/b/1", "/a", "/late"]);
        let after = walk(&late, &tree_root, 2000, showing_under("/b", 5))
            .await
            .unwrap();
        assert_eq!(paths(&after), ["/b/1", "/a"]);
        assert_eq!(after.capped, Some(Capped::BudgetExhausted));
    }

    #[tokio::test]
    async fn one_calls_deadline_and_a_lost_bus_are_errors_even_after_nodes() {
        let tree = Tree::new([("/a", (true, vec![]))]);
        for (failing, name) in [
            ("/hung", ErrorName::DeadlineExceeded),
            ("/lost", ErrorName::UpstreamError),
        ] {
            let error = walk(&tree, &root(&["/a", failing]), 2000, everything())
                .await
                .unwrap_err();
            assert_eq!(error.name, name, "{failing}");
        }
    }
}
