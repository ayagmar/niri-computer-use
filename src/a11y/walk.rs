//! The bounded walk of one window's accessible tree, over any source of nodes. It visits
//! nodes depth first in document order, goes into a node's children only while the node
//! is showing, and stops after a fixed number of nodes, once it has found more matching
//! nodes than were asked for, or when the request's budget runs out, so a huge or hidden
//! tree can't hold the request. The children of a big container, a node with more than
//! `BIG` children such as a long list, table or grid, are read only after everything
//! outside big containers, so one grid early in the tree can't use up the walk before
//! the window's own controls after it are read.

use std::collections::{HashMap, VecDeque};
use std::future::Future;

use serde::Serialize;

use super::model::{self, Extents, Link, NameHash, States};
use crate::error::{ErrorName, ToolError};

mod row;

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
    /// Set when the app refused to list its children with `InvalidArgs`, as at-spi2-atk
    /// does for more than 65,536 of them; `children` is then empty, though it has some.
    pub(crate) children_refused: bool,
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
    /// The app refused to list a showing node's children, so they went unread, though the
    /// walk read on past them.
    ChildrenRefused,
}

/// The nodes under a root, in the order the walk read them, without the root: document
/// order, but for big containers' children, which come after everything else.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Walked {
    pub(crate) nodes: Vec<Node>,
    /// Where each node of `nodes` sits: its parent among them, none under the root, and
    /// its index among the parent's children.
    places: Vec<(Option<usize>, i32)>,
    /// How many of `nodes` have each node's role and name, itself among them.
    alike: Vec<usize>,
    /// Each node's nearest row, itself or above it: see `row`.
    rows: Vec<Option<usize>>,
    /// Set when the walk stopped early with nodes left that might have matched.
    pub(crate) capped: Option<Capped>,
    /// Set when nodes went unread though the walk wasn't capped: it stopped once it found
    /// more matches than wanted, with nodes left, or a node was gone before it was read,
    /// with whatever was under it.
    missed: bool,
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
/// showing nodes only, matching or not, and into a big container's only once nothing
/// outside big containers is left; it stops early once `want` has more matches than
/// it asked for. A node that is gone by the time it is read is skipped. A deadline once
/// the request's budget is spent ends the walk with the nodes read so far,
/// `BudgetExhausted`. One call's own deadline, as for a hung app, and any other failure
/// end the walk with that error.
pub(crate) async fn walk<F: Fn(&Node) -> bool + Sync>(
    source: &impl Source,
    root: &Node,
    cap: usize,
    want: Want<F>,
) -> Result<Walked, ToolError> {
    let mut walked = Walked::default();
    let mut found = 0;
    let mut refused = false;
    let mut pending = Pending::default();
    pending.push(None, &root.children);
    while let Some((parent, child)) = pending.pop() {
        if walked.nodes.len() == cap {
            walked.capped = Some(Capped::NodeCap);
            break;
        }
        let node = match source.node(&child.path).await {
            Ok(Some(node)) => node,
            Ok(None) => {
                walked.missed = true;
                continue;
            }
            Err(error) if error.name == ErrorName::DeadlineExceeded && source.spent() => {
                walked.capped = Some(Capped::BudgetExhausted);
                break;
            }
            Err(error) => return Err(error),
        };
        let at = walked.nodes.len();
        if node.states.has(model::State::Showing) {
            refused |= node.children_refused;
            if node.children.len() > BIG {
                pending.defer(at, &node.children);
            } else {
                pending.push(Some(at), &node.children);
            }
        }
        found += usize::from((want.matches)(&node));
        walked.nodes.push(node);
        walked.places.push((parent, child.index));
        if found > want.wanted {
            walked.missed |= !pending.is_empty();
            break;
        }
    }
    if refused {
        walked.capped = walked.capped.or(Some(Capped::ChildrenRefused));
    }
    walked.finish();
    Ok(walked)
}

/// A node with more children than this is a big container. In the windows measured for
/// it, every container with more than 11 children held a list's records, from 104 to
/// 15,003 of them (docs/decisions.md).
const BIG: usize = 64;

/// The nodes left to read: those to read next, depth first, and the big containers whose
/// children wait until nothing else is left, in the order they were found.
#[derive(Default)]
struct Pending {
    next: Vec<(Option<usize>, Child)>,
    deferred: VecDeque<(usize, Vec<Child>)>,
}

impl Pending {
    /// Reads `children` of the node at `parent` next, in document order.
    fn push(&mut self, parent: Option<usize>, children: &[Child]) {
        self.next
            .extend(children.iter().rev().map(|child| (parent, child.clone())));
    }

    /// Reads `children` of the big container at `parent` once nothing else is left.
    fn defer(&mut self, parent: usize, children: &[Child]) {
        self.deferred.push_back((parent, children.to_vec()));
    }

    /// The next node to read and its parent's place, or `None` when none is left.
    fn pop(&mut self) -> Option<(Option<usize>, Child)> {
        while self.next.is_empty() {
            let (parent, children) = self.deferred.pop_front()?;
            self.push(Some(parent), &children);
        }
        self.next.pop()
    }

    fn is_empty(&self) -> bool {
        self.next.is_empty() && self.deferred.is_empty()
    }
}

impl Walked {
    /// What `row` asks of every node, worked out once for all of them.
    fn finish(&mut self) {
        self.alike = alike(&self.nodes);
        let nodes = &self.nodes;
        self.rows = rows(&self.places, |index| {
            nodes
                .get(index)
                .is_some_and(|node| ROWS.contains(&model::role_name(node.role)))
        });
    }
}

/// The roles of a list's, a table's or a tree's rows, each showing one record.
const ROWS: [&str; 3] = ["list_item", "table_row", "tree_item"];

/// For each node, how many nodes share its role and name.
fn alike(nodes: &[Node]) -> Vec<usize> {
    let mut counts: HashMap<(u32, &str), usize> = HashMap::new();
    for node in nodes {
        *counts.entry((node.role, &node.name)).or_default() += 1;
    }
    nodes
        .iter()
        .map(|node| {
            counts
                .get(&(node.role, node.name.as_str()))
                .copied()
                .unwrap_or(1)
        })
        .collect()
}

/// For each node, the nearest node that `is_row`, itself or above it, in one pass: a
/// node's parent is read before it, so its answer is already known.
fn rows(places: &[(Option<usize>, i32)], is_row: impl Fn(usize) -> bool) -> Vec<Option<usize>> {
    let mut rows: Vec<Option<usize>> = Vec::with_capacity(places.len());
    for (index, (parent, _)) in places.iter().enumerate() {
        let above = parent.and_then(|parent| rows.get(parent).copied().flatten());
        rows.push(if is_row(index) { Some(index) } else { above });
    }
    rows
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::a11y::model::{Row, State, Unbound};

    const SHOWING: u32 = (1 << 25) | (1 << 30);

    /// An in-memory tree: path → (showing, children), each node named after its path.
    /// Missing paths are gone, and `/refused`'s children are refused, as at-spi2-atk
    /// refuses more than 65,536. `/late` times out with the request's budget spent, `/hung`
    /// times out on its own call with budget left, as a hung app does, and `/lost` fails
    /// as a lost bus connection does.
    struct Tree {
        nodes: BTreeMap<&'static str, (bool, Vec<&'static str>)>,
        spent: AtomicBool,
    }

    impl Tree {
        fn new<const N: usize>(nodes: [(&'static str, (bool, Vec<&'static str>)); N]) -> Self {
            Self {
                nodes: BTreeMap::from(nodes),
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
                name: format!("name of {path}"),
                states: States::from_words(&[if *showing { SHOWING } else { 0 }]),
                extents: None,
                actions: Vec::new(),
                children: if path == "/refused" {
                    Vec::new()
                } else {
                    kids(children)
                },
                children_refused: path == "/refused",
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

    fn root(children: &[&str]) -> Node {
        Node {
            path: "/root".to_owned(),
            role: 23,
            name: String::new(),
            states: States::from_words(&[SHOWING]),
            extents: None,
            actions: Vec::new(),
            children: kids(children),
            children_refused: false,
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

    /// Wants `wanted` showing nodes whose path starts with `prefix`.
    fn showing_under(prefix: &'static str, wanted: usize) -> Want<impl Fn(&Node) -> bool + Sync> {
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
    async fn finds_matches_under_parents_that_dont_match_and_stops_past_the_limit() {
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
        // /b/1 sits under /a, which doesn't match; /b/hidden is never read.
        assert_eq!(
            paths(&walked),
            ["/a", "/a/1", "/b/1", "/hidden", "/b/2", "/b/3"]
        );
        assert_eq!(walked.capped, None);
        // /b/4 went unread, so no count binds a node outside rows.
        assert_eq!(walked.row(0), Row::Unbound(Unbound::CutShort));
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
        assert_eq!(walked.row(0), Row::Lineage);
    }

    // The bug: a node gone before it was read takes its subtree with it, which may hold a
    // look-alike, yet the count looked finished and bound the node outside rows.
    #[tokio::test]
    async fn a_node_gone_before_it_was_read_leaves_the_count_unfinished() {
        let tree = Tree::new([("/a", (true, vec![]))]);
        let walked = walk(&tree, &root(&["/a", "/gone"]), 2000, everything())
            .await
            .unwrap();
        assert_eq!(paths(&walked), ["/a"]);
        assert_eq!(walked.capped, None);
        assert_eq!(walked.row(0), Row::Unbound(Unbound::CutShort));
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

    /// A tree of `nodes`, each showing, plus a showing `/grid` with `BIG` + 1 leaves,
    /// `/grid/0` on.
    fn with_grid(nodes: &[(&'static str, &[&'static str])]) -> Tree {
        let mut tree = Tree::new([]);
        add_grid(&mut tree, "/grid");
        tree.nodes.extend(
            nodes
                .iter()
                .map(|&(path, children)| (path, (true, children.to_vec()))),
        );
        tree
    }

    /// Adds a showing `grid` with `BIG` + 1 leaves, `<grid>/0` on.
    fn add_grid(tree: &mut Tree, grid: &'static str) {
        let cells: Vec<&'static str> = (0..=BIG)
            .map(|cell| &*Box::leak(format!("{grid}/{cell}").into_boxed_str()))
            .collect();
        tree.nodes
            .extend(cells.iter().map(|&cell| (cell, (true, Vec::new()))));
        tree.nodes.insert(grid, (true, cells));
    }

    // The bug: one grid early in the tree used up the cap depth first, so the window's
    // controls after it were never read, let alone listed.
    #[tokio::test]
    async fn a_big_container_early_in_the_tree_no_longer_hides_the_controls_after_it() {
        let tree = with_grid(&[("/save", &[])]);
        let walked = walk(&tree, &root(&["/grid", "/save"]), 10, everything())
            .await
            .unwrap();
        assert_eq!(&paths(&walked)[..3], ["/grid", "/save", "/grid/0"]);
        assert_eq!(walked.capped, Some(Capped::NodeCap));
        // The grid wasn't read to its end, so `/save` may have a look-alike in it.
        assert_eq!(walked.row(1), Row::Unbound(Unbound::CutShort));
    }

    // A big container's children are read last, in document order, and each still knows
    // its place under the container, which its ref's lineage checks.
    #[tokio::test]
    async fn a_big_containers_children_come_last_and_keep_their_place() {
        let tree = with_grid(&[("/a", &["/a/1"]), ("/a/1", &[]), ("/b", &[])]);
        let walked = walk(&tree, &root(&["/a", "/grid", "/b"]), 2000, everything())
            .await
            .unwrap();
        let read = paths(&walked);
        assert_eq!(
            &read[..6],
            ["/a", "/a/1", "/grid", "/b", "/grid/0", "/grid/1"]
        );
        assert_eq!(read.len(), 4 + BIG + 1);
        assert_eq!(walked.capped, None);
        let at = read.iter().position(|path| *path == "/grid/3").unwrap();
        let link = |path: &str, index| Link {
            path: path.to_owned(),
            index,
            name: NameHash::of(&format!("name of {path}")),
        };
        assert_eq!(walked.lineage(at), [link("/grid", 1), link("/grid/3", 3)]);
        assert_eq!(walked.row(at), Row::Lineage);
    }

    // The server's cap reads a window of 2,500 objects whole. The old cap of 2,000 cut it
    // short, so no control outside a named row bound a ref.
    #[tokio::test]
    async fn a_window_of_2500_objects_is_read_whole_at_the_servers_cap() {
        let cells: Vec<&'static str> = (0..2500)
            .map(|cell| &*Box::leak(format!("/cell/{cell}").into_boxed_str()))
            .collect();
        let mut tree = Tree::new([("/save", (true, vec![]))]);
        tree.nodes
            .extend(cells.iter().map(|&cell| (cell, (true, Vec::new()))));
        let mut children = cells;
        children.push("/save");
        let walked = walk(&tree, &root(&children), crate::a11y::NODE_CAP, everything())
            .await
            .unwrap();
        assert_eq!(walked.nodes.len(), 2501);
        assert_eq!(walked.capped, None);
        assert_eq!(walked.row(2500), Row::Lineage);
    }

    // Two big containers are read in the order they were found: the first grid's cells
    // before the second's, as a table above a list keeps the table's rows first.
    #[tokio::test]
    async fn big_containers_are_read_in_the_order_they_were_found() {
        let mut tree = with_grid(&[]);
        add_grid(&mut tree, "/list");
        let walked = walk(&tree, &root(&["/grid", "/list"]), 2000, everything())
            .await
            .unwrap();
        let read = paths(&walked);
        let first = |prefix: &str| read.iter().position(|path| path.starts_with(prefix));
        assert_eq!(&read[..3], ["/grid", "/list", "/grid/0"]);
        assert!(first("/list/").unwrap() > read.len() - BIG - 2, "{read:?}");
    }

    // The bug: at-spi2-atk refuses `GetChildren` for more than 65,536 children, and that
    // refusal failed the whole listing. The node's children now go unread, the walk reads
    // on, and the walk is cut short, so `/save` is listed but binds nothing.
    #[tokio::test]
    async fn children_the_app_refuses_go_unread_and_leave_the_walk_cut_short() {
        let tree = Tree::new([
            ("/refused", (true, vec!["/refused/1"])),
            ("/refused/1", (true, vec![])),
            ("/save", (true, vec![])),
        ]);
        let walked = walk(&tree, &root(&["/refused", "/save"]), 2000, everything())
            .await
            .unwrap();
        assert_eq!(paths(&walked), ["/refused", "/save"]);
        assert_eq!(walked.capped, Some(Capped::ChildrenRefused));
        assert_eq!(walked.row(1), Row::Unbound(Unbound::CutShort));
    }

    // The bug a walk stopped at its limit with only a big container's children left
    // would have: nothing left to read next, so the count looked finished and bound
    // `/b/1`, whose look-alike may sit in the grid.
    #[tokio::test]
    async fn a_walk_stopped_with_a_big_container_unread_leaves_the_count_unfinished() {
        let tree = with_grid(&[("/b/1", &[]), ("/b/2", &[])]);
        let tree_root = root(&["/grid", "/b/1", "/b/2"]);
        let walked = walk(&tree, &tree_root, 2000, showing_under("/b", 1))
            .await
            .unwrap();
        assert_eq!(paths(&walked), ["/grid", "/b/1", "/b/2"]);
        assert_eq!(walked.row(1), Row::Unbound(Unbound::CutShort));
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
