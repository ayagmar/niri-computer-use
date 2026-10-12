//! What tells an element apart from the elements like it, of its role and name, around it.
//! A list shows one `Delete` button per row, and the row's label says which record it
//! deletes; a list that reuses a row's objects in place for another record, as Qt Quick's
//! does, changes only that label. So a ref to an element with others like it nearby keeps
//! the objects beside it that its role and name don't cover, and the element is the same
//! one only while they keep their places and names.

use super::Walked;
use crate::a11y::model::{Beside, Link, NameHash};

/// The most objects a ref keeps beside its element, the nearest in document order: each
/// is read twice more right before an action.
const KEPT: usize = 16;

impl Walked {
    /// The objects beside the node at `at` that tell it apart from its look-alike nodes,
    /// those of its role and name. From the node up, the first ancestor with another child
    /// holding a look-alike, or whose children weren't all read, decides: its children
    /// between the nearest such ones on either side, which hold the node, are what tells it
    /// apart, apart from the node's own subtree and its ancestors, which its ref checks
    /// already. None when no ancestor up to the root has a look-alike.
    pub(crate) fn beside(&self, at: usize) -> Vec<Beside> {
        let Some(node) = self.nodes.get(at) else {
            return Vec::new();
        };
        let alike = |index: usize| {
            self.nodes
                .get(index)
                .is_some_and(|other| other.role == node.role && other.name == node.name)
        };
        let mut child = at;
        loop {
            let parent = self.parent(child);
            let holds = |sibling: &usize| self.subtree(*sibling).any(alike);
            let siblings = self.children(parent);
            let before: Vec<usize> = siblings.iter().copied().filter(|&s| s < child).collect();
            let after: Vec<usize> = siblings.iter().copied().filter(|&s| s > child).collect();
            let near_before: Vec<usize> = before
                .iter()
                .rev()
                .copied()
                .take_while(|s| !holds(s))
                .collect();
            let near_after: Vec<usize> = after.iter().copied().take_while(|s| !holds(s)).collect();
            if near_before.len() < before.len()
                || near_after.len() < after.len()
                || self.unfinished(parent)
            {
                let segment = near_before.into_iter().chain([child]).chain(near_after);
                return self.kept(at, segment);
            }
            match parent {
                Some(parent) => child = parent,
                None => return Vec::new(),
            }
        }
    }

    /// The objects in the subtrees of `segment`, except `at`'s own and its ancestors, the
    /// `KEPT` nearest to it, in document order.
    fn kept(&self, at: usize, segment: impl Iterator<Item = usize>) -> Vec<Beside> {
        let own = self.subtree(at);
        let mut found: Vec<usize> = segment
            .flat_map(|sibling| self.subtree(sibling))
            .filter(|index| !own.contains(index) && !self.ancestor_of(*index, at))
            .collect();
        found.sort_by_key(|&index| index.abs_diff(at));
        found.truncate(KEPT);
        found.sort_unstable();
        found
            .into_iter()
            .filter_map(|index| {
                let node = self.nodes.get(index)?;
                let (_, place) = self.places.get(index)?;
                let parent = self.parent(index).and_then(|parent| self.nodes.get(parent));
                Some(Beside {
                    parent: parent.map(|parent| parent.path.clone()),
                    link: Link {
                        path: node.path.clone(),
                        index: *place,
                        name: NameHash::of(&node.name),
                    },
                })
            })
            .collect()
    }

    fn parent(&self, index: usize) -> Option<usize> {
        self.places.get(index).and_then(|(parent, _)| *parent)
    }

    /// The nodes read under `parent`, none for the root, in document order: each sibling's
    /// subtree ends where the next sibling begins.
    fn children(&self, parent: Option<usize>) -> Vec<usize> {
        let (first, end) = parent.map_or((0, self.nodes.len()), |parent| {
            (parent + 1, self.end(parent))
        });
        std::iter::successors(Some(first), |&child| Some(self.end(child)))
            .take_while(|&child| child < end)
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

    /// Whether the walk stopped with children of `parent`, or of a node under it, unread,
    /// so a look-alike may be among them.
    fn unfinished(&self, parent: Option<usize>) -> bool {
        self.unread.iter().any(|&unread| match (parent, unread) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(parent), Some(unread)) => unread == parent || self.ancestor_of(parent, unread),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{Tree, everything, root, showing_under};
    use super::super::{Walked, walk};
    use crate::a11y::model::Beside;

    /// The paths of the objects kept beside the node at `path`.
    fn beside(walked: &Walked, path: &str) -> Vec<String> {
        let at = walked
            .nodes
            .iter()
            .position(|node| node.path == path)
            .unwrap();
        walked
            .beside(at)
            .into_iter()
            .map(|Beside { link, .. }| link.path)
            .collect()
    }

    /// Two rows of a list, each with its record's label and a `Delete` button, and a
    /// `Play` button beside the elapsed time.
    fn rows() -> Tree {
        Tree::new([
            ("/list", (true, vec!["/row1", "/row2"])),
            ("/row1", (true, vec!["/row1/label", "/row1/delete#Delete"])),
            ("/row1/label", (true, vec![])),
            ("/row1/delete#Delete", (true, vec!["/row1/delete/text"])),
            ("/row1/delete/text", (true, vec![])),
            ("/row2", (true, vec!["/row2/label", "/row2/delete#Delete"])),
            ("/row2/label", (true, vec![])),
            ("/row2/delete#Delete", (true, vec![])),
            ("/player", (true, vec!["/player/time", "/player/play"])),
            ("/player/time", (true, vec![])),
            ("/player/play", (true, vec![])),
        ])
    }

    #[tokio::test]
    async fn a_button_like_the_next_rows_keeps_its_rows_label_and_a_unique_one_nothing() {
        let tree = rows();
        let walked = walk(&tree, &root(&["/list", "/player"]), 2000, everything())
            .await
            .unwrap();
        assert_eq!(beside(&walked, "/row1/delete#Delete"), ["/row1/label"]);
        assert_eq!(beside(&walked, "/row2/delete#Delete"), ["/row2/label"]);
        assert_eq!(beside(&walked, "/player/play"), Vec::<String>::new());
    }

    #[tokio::test]
    async fn a_button_side_by_side_with_buttons_like_it_keeps_the_objects_up_to_them() {
        let tree = Tree::new([
            (
                "/list",
                (
                    true,
                    vec!["/a", "/delete1#Delete", "/b", "/c", "/delete2#Delete", "/d"],
                ),
            ),
            ("/a", (true, vec![])),
            ("/delete1#Delete", (true, vec![])),
            ("/b", (true, vec![])),
            ("/c", (true, vec![])),
            ("/delete2#Delete", (true, vec![])),
            ("/d", (true, vec![])),
        ]);
        let walked = walk(&tree, &root(&["/list"]), 2000, everything())
            .await
            .unwrap();
        assert_eq!(beside(&walked, "/delete1#Delete"), ["/a", "/b", "/c"]);
        assert_eq!(beside(&walked, "/delete2#Delete"), ["/b", "/c", "/d"]);
    }

    // The walk stopped past the limit before the second row: a look-alike may be there.
    #[tokio::test]
    async fn a_button_whose_rows_neighbours_were_left_unread_keeps_what_was_read_of_them() {
        let tree = rows();
        let walked = walk(
            &tree,
            &root(&["/list", "/player"]),
            2000,
            showing_under("/row1/delete", 0),
        )
        .await
        .unwrap();
        assert_eq!(walked.nodes.last().unwrap().path, "/row1/delete#Delete");
        assert_eq!(beside(&walked, "/row1/delete#Delete"), ["/row1/label"]);
    }

    #[tokio::test]
    async fn only_the_nearest_objects_are_kept() {
        let mut children = vec!["/delete1#Delete"];
        let labels: Vec<&'static str> = (0..40)
            .map(|n| &*Box::leak(format!("/label{n:02}").into_boxed_str()))
            .collect();
        children.extend(&labels);
        children.push("/delete2#Delete");
        let mut nodes: Vec<(&'static str, (bool, Vec<&'static str>))> =
            vec![("/list", (true, children.clone()))];
        nodes.extend(children.iter().map(|&path| (path, (true, vec![]))));
        let tree = Tree::of(nodes);
        let walked = walk(&tree, &root(&["/list"]), 2000, everything())
            .await
            .unwrap();
        let kept = beside(&walked, "/delete1#Delete");
        assert_eq!(kept, labels[..super::KEPT]);
    }
}
