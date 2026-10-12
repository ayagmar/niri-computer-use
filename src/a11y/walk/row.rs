//! Which row an element sits in, for a ref to it. A list shows one `Delete` button per
//! row, and the row's label says which record it deletes; a list that reuses a row's
//! objects in place for another record, as Qt Quick's does, changes only that label. So
//! a ref to an element with look-alike elements, other elements of its role and name, keeps every
//! other object of its row, and is the same element only while they keep their places
//! and names. The bus doesn't say where a row ends, so the row is found from the tree's
//! shape, and when that shape doesn't settle it, the ref can't be acted on.
//!
//! The row is the subtree, holding the element, of the lowest ancestor whose other
//! children hold the element's look-alike elements, when each of those children is shaped like it
//! and nothing else sits between them: a list of rows, each a container. An ancestor
//! whose look-alike elements sit beside the element itself, as two same-named buttons in one row
//! do, isn't the list, so the search goes on above it. Anything else is `Unbound`: rows
//! that aren't containers, look-alike elements among other objects, a row the walk didn't read
//! whole, or one with more than `KEPT` other objects.

use super::{REACH, Walked};
use crate::a11y::model::{Beside, Link, NameHash, Row, Unbound};

/// The most objects a ref keeps of its row: each is read twice more right before an
/// action.
const KEPT: usize = 16;

impl Walked {
    /// The row of the node at `at`, as its ref keeps it.
    pub(crate) fn row(&self, at: usize) -> Row {
        let alike = self.alike_to(at);
        let own = count(alike, self.subtree(at));
        if alike.len() == own {
            return self.alone(at);
        }
        let mut child = at;
        loop {
            let parent = self.parent(child);
            if child != at && count(alike, self.span(parent)) > count(alike, self.subtree(child)) {
                return self.rows(at, parent, child);
            }
            match parent {
                Some(parent) => child = parent,
                None => return Row::Unbound(Unbound::Flat),
            }
        }
    }

    /// A node with no look-alike among the nodes read. The walk reads at least `REACH`
    /// nodes past it unless it was cut short, so a list's next row would have been read;
    /// cut short closer to it than that, it can't tell.
    fn alone(&self, at: usize) -> Row {
        if self.complete || self.nodes.len().saturating_sub(self.end(at)) >= REACH {
            Row::Alone
        } else {
            Row::Unbound(Unbound::Cut)
        }
    }

    /// The row `child` of `parent`, whose other children hold look-alike elements of the node at
    /// `at`, if `parent` is a list of rows shaped alike and read whole.
    fn rows(&self, at: usize, parent: Option<usize>, child: usize) -> Row {
        let alike = self.alike_to(at);
        let holds = |sibling: usize| count(alike, self.subtree(sibling)) > 0;
        let children = self.children(parent);
        let held: Vec<usize> = children.iter().copied().filter(|&c| holds(c)).collect();
        let (Some(&first), Some(&last)) = (held.first(), held.last()) else {
            return Row::Unbound(Unbound::Flat);
        };
        let between = children.iter().copied().filter(|&c| first < c && c < last);
        if between.into_iter().any(|c| !holds(c)) {
            return Row::Unbound(Unbound::Flat);
        }
        if self.is_open(child) {
            return Row::Unbound(Unbound::Cut);
        }
        let whole: Vec<usize> = held
            .into_iter()
            .filter(|&row| row != child && !self.is_open(row))
            .collect();
        if whole.is_empty() {
            return Row::Unbound(Unbound::Cut);
        }
        let shape = self.shapes.get(child);
        if whole.iter().any(|&row| self.shapes.get(row) != shape) {
            return Row::Unbound(Unbound::Flat);
        }
        let kept: Vec<usize> = self
            .subtree(child)
            .filter(|&index| index != at && !self.ancestor_of(index, at))
            .collect();
        if kept.len() > KEPT {
            return Row::Unbound(Unbound::Large);
        }
        Row::Kept(
            kept.into_iter()
                .filter_map(|index| self.beside(index))
                .collect(),
        )
    }

    /// What a ref keeps of the node at `index` beside its element.
    fn beside(&self, index: usize) -> Option<Beside> {
        let node = self.nodes.get(index)?;
        let (parent, place) = self.places.get(index)?;
        Some(Beside {
            parent: parent
                .and_then(|parent| self.nodes.get(parent))
                .map(|parent| parent.path.clone()),
            link: Link {
                path: node.path.clone(),
                index: *place,
                name: NameHash::of(&node.name),
            },
        })
    }

    /// The nodes of the same role and name as the node at `at`, it among them.
    fn alike_to(&self, at: usize) -> &[usize] {
        self.alike_of
            .get(at)
            .and_then(|group| self.alike.get(*group))
            .map_or(&[], Vec::as_slice)
    }

    fn parent(&self, index: usize) -> Option<usize> {
        self.places.get(index).and_then(|(parent, _)| *parent)
    }

    fn is_open(&self, index: usize) -> bool {
        self.open.get(index).copied().unwrap_or(true)
    }

    /// The nodes read under `parent`, the root when none, without it.
    fn span(&self, parent: Option<usize>) -> std::ops::Range<usize> {
        parent.map_or(0..self.nodes.len(), |parent| parent + 1..self.end(parent))
    }

    /// The nodes read under `parent`, none for the root, in document order: each
    /// sibling's subtree ends where the next sibling begins.
    fn children(&self, parent: Option<usize>) -> Vec<usize> {
        let span = self.span(parent);
        std::iter::successors(Some(span.start), |&child| Some(self.end(child)))
            .take_while(|&child| child < span.end)
            .collect()
    }

    /// The node at `index` and the nodes read under it, which follow it in document order.
    fn subtree(&self, index: usize) -> std::ops::Range<usize> {
        index..self.end(index)
    }

    /// Where the subtree of the node at `index` ends in document order.
    fn end(&self, index: usize) -> usize {
        self.ends.get(index).copied().unwrap_or(index + 1)
    }

    /// Whether the node at `ancestor` is above the node at `index`.
    fn ancestor_of(&self, ancestor: usize, index: usize) -> bool {
        ancestor < index && index < self.end(ancestor)
    }
}

/// How many of `sorted` fall in `range`.
fn count(sorted: &[usize], range: std::ops::Range<usize>) -> usize {
    let below = |bound: usize| sorted.partition_point(|&index| index < bound);
    below(range.end) - below(range.start)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Tree, everything, root, showing_under};
    use super::super::{REACH, Walked, walk};
    use crate::a11y::model::{Beside, Row, Unbound};

    /// What the ref to the node at `path` keeps: the paths of its row's objects.
    fn row(walked: &Walked, path: &str) -> Result<Vec<String>, Option<Unbound>> {
        let at = walked
            .nodes
            .iter()
            .position(|node| node.path == path)
            .unwrap();
        match walked.row(at) {
            Row::Alone => Err(None),
            Row::Unbound(why) => Err(Some(why)),
            Row::Kept(kept) => Ok(kept
                .into_iter()
                .map(|Beside { link, .. }| link.path)
                .collect()),
        }
    }

    /// A list of two rows, each a container of a label with its record and the given
    /// buttons, the `Delete` ones named alike, and a `Play` button beside the elapsed
    /// time.
    fn list(buttons: &[&'static str]) -> Tree {
        let mut nodes = vec![("/list", (true, vec!["/row1", "/row2"]))];
        for row in ["/row1", "/row2"] {
            let label = leak(format!("{row}/label"));
            let mut children = vec![label];
            children.extend(buttons.iter().map(|button| leak(format!("{row}{button}"))));
            nodes.push((row, (true, children.clone())));
            nodes.extend(children.into_iter().map(|child| (child, (true, vec![]))));
        }
        nodes.push(("/player", (true, vec!["/player/time", "/player/play"])));
        nodes.push(("/player/time", (true, vec![])));
        nodes.push(("/player/play", (true, vec![])));
        Tree::of(nodes)
    }

    fn leak(path: String) -> &'static str {
        Box::leak(path.into_boxed_str())
    }

    async fn walked(tree: &Tree) -> Walked {
        walk(tree, &root(&["/list", "/player"]), 2000, everything())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_button_like_the_next_rows_keeps_its_rows_label_and_a_unique_one_nothing() {
        let walked = walked(&list(&["/delete#Delete"])).await;
        assert_eq!(
            row(&walked, "/row1/delete#Delete"),
            Ok(vec!["/row1/label".to_owned()])
        );
        assert_eq!(
            row(&walked, "/row2/delete#Delete"),
            Ok(vec!["/row2/label".to_owned()])
        );
        assert_eq!(row(&walked, "/player/play"), Err(None));
    }

    // Two buttons of one name in each row: the row, not the button beside it, is what
    // tells the second apart.
    #[tokio::test]
    async fn a_second_button_of_one_name_in_a_row_keeps_the_whole_row() {
        let walked = walked(&list(&["/a#Delete", "/b#Delete"])).await;
        assert_eq!(
            row(&walked, "/row1/b#Delete"),
            Ok(vec!["/row1/label".to_owned(), "/row1/a#Delete".to_owned()])
        );
    }

    // A row with more objects than a ref keeps can't be checked whole, so the ref can't
    // be acted on rather than keep a part that may leave the record's label out.
    #[tokio::test]
    async fn a_row_with_more_objects_than_a_ref_keeps_is_unbound() {
        let statics: Vec<&'static str> = (0..super::KEPT)
            .map(|n| leak(format!("/s{n:02}")))
            .collect();
        let mut buttons = statics.clone();
        buttons.push("/delete#Delete");
        let walked = walked(&list(&buttons)).await;
        assert_eq!(
            row(&walked, "/row1/delete#Delete"),
            Err(Some(Unbound::Large))
        );
        let fits = walked_with(&statics[1..]).await;
        assert_eq!(
            row(&fits, "/row1/delete#Delete").map(|kept| kept.len()),
            Ok(super::KEPT)
        );
    }

    async fn walked_with(statics: &[&'static str]) -> Walked {
        let mut buttons = statics.to_vec();
        buttons.push("/delete#Delete");
        walked(&list(&buttons)).await
    }

    // Rows side by side in the list, with no container of their own: nothing shows
    // whether a label belongs to the button before it or after it.
    #[tokio::test]
    async fn buttons_side_by_side_with_their_look_alike_elements_are_unbound() {
        let tree = Tree::new([
            (
                "/list",
                (
                    true,
                    vec!["/label1", "/delete1#Delete", "/label2", "/delete2#Delete"],
                ),
            ),
            ("/label1", (true, vec![])),
            ("/delete1#Delete", (true, vec![])),
            ("/label2", (true, vec![])),
            ("/delete2#Delete", (true, vec![])),
        ]);
        let walked = walk(&tree, &root(&["/list"]), 2000, everything())
            .await
            .unwrap();
        assert_eq!(row(&walked, "/delete1#Delete"), Err(Some(Unbound::Flat)));
        assert_eq!(row(&walked, "/delete2#Delete"), Err(Some(Unbound::Flat)));
    }

    // Rows that aren't shaped alike, or something else between them, don't show which
    // container is a row.
    #[tokio::test]
    async fn rows_of_different_shapes_or_with_something_between_are_unbound() {
        let tree = Tree::new([
            ("/list", (true, vec!["/row1", "/gap", "/row2", "/row3"])),
            ("/row1", (true, vec!["/row1/label", "/row1/delete#Delete"])),
            ("/row1/label", (true, vec![])),
            ("/row1/delete#Delete", (true, vec![])),
            ("/gap", (true, vec![])),
            ("/row2", (true, vec!["/row2/label", "/row2/delete#Delete"])),
            ("/row2/label", (true, vec![])),
            ("/row2/delete#Delete", (true, vec![])),
            ("/row3", (true, vec!["/row3/delete#Delete"])),
            ("/row3/delete#Delete", (true, vec![])),
        ]);
        let walked = walk(&tree, &root(&["/list"]), 2000, everything())
            .await
            .unwrap();
        assert_eq!(
            row(&walked, "/row1/delete#Delete"),
            Err(Some(Unbound::Flat))
        );
        assert_eq!(
            row(&walked, "/row3/delete#Delete"),
            Err(Some(Unbound::Flat))
        );
    }

    /// Two rows, each `Delete`, `More` and then the label with its record, so a walk cut
    /// right after the first `More` leaves the first row's record unread.
    fn label_last() -> Tree {
        let mut nodes = vec![("/list", (true, vec!["/row1", "/row2"]))];
        for row in ["/row1", "/row2"] {
            let children = ["/delete#Delete", "/more#More", "/label"]
                .map(|child| leak(format!("{row}{child}")));
            nodes.push((row, (true, children.to_vec())));
            nodes.extend(children.into_iter().map(|child| (child, (true, vec![]))));
        }
        Tree::of(nodes)
    }

    // The walk read `REACH` nodes past the match that stopped it, so the first row's
    // record and the next row were read after all.
    #[tokio::test]
    async fn a_walk_stopped_at_its_limit_reads_on_to_the_rows_record() {
        let tree = label_last();
        let walked = walk(&tree, &root(&["/list"]), 2000, showing_under("/row1/", 1))
            .await
            .unwrap();
        assert_eq!(walked.capped, None);
        assert_eq!(
            row(&walked, "/row1/delete#Delete"),
            Ok(vec!["/row1/more#More".to_owned(), "/row1/label".to_owned()])
        );
    }

    // The object cap or the request's budget stopped the walk before the first row's
    // record: the ref can't be acted on.
    #[tokio::test]
    async fn a_walk_cut_before_the_rows_record_leaves_its_ref_unbound() {
        let tree = label_last();
        let capped = walk(&tree, &root(&["/list"]), 4, everything())
            .await
            .unwrap();
        assert_eq!(row(&capped, "/row1/delete#Delete"), Err(Some(Unbound::Cut)));
        let spent = Tree::new([
            ("/list", (true, vec!["/row1", "/row2"])),
            (
                "/row1",
                (
                    true,
                    vec!["/row1/delete#Delete", "/row1/more#More", "/late"],
                ),
            ),
            ("/row1/delete#Delete", (true, vec![])),
            ("/row1/more#More", (true, vec![])),
            ("/row2", (true, vec![])),
        ]);
        let late = walk(&spent, &root(&["/list"]), 2000, everything())
            .await
            .unwrap();
        assert_eq!(row(&late, "/row1/delete#Delete"), Err(Some(Unbound::Cut)));
    }

    // A unique node read well before the cap keeps no row: the walk read past it far
    // enough to see a next row.
    #[tokio::test]
    async fn a_unique_node_far_enough_before_the_cap_is_alone() {
        let mut children = vec!["/play"];
        let fillers: Vec<&'static str> =
            (0..REACH + 10).map(|n| leak(format!("/f{n:03}"))).collect();
        children.extend(&fillers);
        let mut nodes = vec![("/panel", (true, children.clone()))];
        nodes.extend(children.iter().map(|&path| (path, (true, vec![]))));
        let tree = Tree::of(nodes);
        let walked = walk(&tree, &root(&["/panel"]), REACH + 5, everything())
            .await
            .unwrap();
        assert_eq!(row(&walked, "/play"), Err(None));
        assert_eq!(row(&walked, fillers[REACH - 1]), Err(Some(Unbound::Cut)));
    }

    // Many root children left unread after a deep chain: each node's row is found in
    // one pass over what was read, not one per unread child.
    #[tokio::test]
    async fn a_wide_frame_cut_after_a_deep_chain_settles_every_row_quickly() {
        let depth = 1500;
        let chain: Vec<&'static str> = (0..depth).map(|n| leak(format!("/c{n:04}"))).collect();
        let mut nodes: Vec<(&'static str, (bool, Vec<&'static str>))> = chain
            .windows(2)
            .map(|pair| (pair[0], (true, vec![pair[1]])))
            .collect();
        nodes.push((chain[depth - 1], (true, vec![])));
        let tree = Tree::of(nodes);
        let mut top = vec![chain[0]];
        let wide: Vec<&'static str> = (0..100_000).map(|n| leak(format!("/w{n:06}"))).collect();
        top.extend(&wide);
        let walked = walk(&tree, &root(&top), depth, everything()).await.unwrap();
        let started = std::time::Instant::now();
        let rows: Vec<Row> = (0..walked.nodes.len()).map(|at| walked.row(at)).collect();
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        // The cap fell inside every node's subtree, so none can tell its row.
        assert!(rows.iter().all(|row| *row == Row::Unbound(Unbound::Cut)));
    }
}
