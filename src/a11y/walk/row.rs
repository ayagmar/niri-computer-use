//! Which record a ref to an element is bound to. A list shows one `Delete` button per
//! row, and the record it deletes is the row's; a list that reuses a row's objects in
//! place for another record, as Qt Quick's does, leaves the button's path and name as
//! they were. The only record boundary the bus states is a row's role: a list item, table
//! row or tree item. A named row binds the record, since the lineage compares its name;
//! an unnamed one binds nothing. Without a row, an element whose role and name other
//! elements read share may be one of a list's rows put on the bus without containers, so
//! nothing binds it either. Nothing here guesses a row from the tree's shape.

use super::Walked;
use crate::a11y::model::{Row, Unbound};

impl Walked {
    /// How a ref to the node at `at` is bound to its record.
    pub(crate) fn row(&self, at: usize) -> Row {
        let row = self.rows.get(at).copied().flatten();
        match row.and_then(|row| self.nodes.get(row).map(|node| (row, node))) {
            Some((_, node)) if node.name.trim().is_empty() => Row::Unbound(Unbound::UnnamedRow),
            Some((row, _)) if row == at => Row::Own,
            None if self.alike.get(at).is_some_and(|&alike| alike > 1) => {
                Row::Unbound(Unbound::Repeated)
            }
            Some(_) | None => Row::Lineage,
        }
    }

    /// Whether `row` binds the node at `at` only because no other node read has its role
    /// and name: it sits in no row, so a walk that stopped early may have missed one.
    pub(crate) fn bound_by_count(&self, at: usize) -> bool {
        self.rows.get(at).copied().flatten().is_none() && self.row(at) == Row::Lineage
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::super::{Node, Walked, rows};
    use crate::a11y::model::{Row, States, Unbound};

    const FILLER: u32 = 20;
    const IMAGE: u32 = 27;
    const LABEL: u32 = 29;
    const LIST: u32 = 31;
    const LIST_ITEM: u32 = 32;
    const PANEL: u32 = 39;
    const BUTTON: u32 = 43;
    const TABLE_ROW: u32 = 90;
    const TREE_ITEM: u32 = 91;

    /// A walked tree from `(depth, role, name)` in document order, each node's path its
    /// position.
    fn walked(tree: &[(usize, u32, &str)]) -> Walked {
        let mut walked = Walked::default();
        let mut above: Vec<usize> = Vec::new();
        for (at, &(depth, role, name)) in tree.iter().enumerate() {
            above.truncate(depth);
            walked.nodes.push(Node {
                path: format!("/{at}"),
                role,
                name: name.to_owned(),
                states: States::from_words(&[0]),
                extents: None,
                actions: Vec::new(),
                children: Vec::new(),
            });
            walked.places.push((above.last().copied(), 0));
            above.push(at);
        }
        walked.finish();
        walked
    }

    /// The row of every node whose name is `name`.
    fn rows_named(walked: &Walked, name: &str) -> Vec<Row> {
        (0..walked.nodes.len())
            .filter(|&at| walked.nodes.get(at).is_some_and(|node| node.name == name))
            .map(|at| walked.row(at))
            .collect()
    }

    /// A list of `records`, each a list item named `item(record)` holding its label and a
    /// `Delete` button.
    fn list(item: impl Fn(&str) -> String, records: &[&str]) -> Walked {
        let mut tree = vec![(0, LIST, String::new())];
        for record in records {
            tree.push((1, LIST_ITEM, item(record)));
            tree.push((2, LABEL, (*record).to_owned()));
            tree.push((2, BUTTON, "Delete".to_owned()));
        }
        let borrowed: Vec<(usize, u32, &str)> = tree
            .iter()
            .map(|(depth, role, name)| (*depth, *role, name.as_str()))
            .collect();
        walked(&borrowed)
    }

    #[test]
    fn a_control_in_a_named_row_is_bound_by_the_rows_name() {
        let walked = list(str::to_owned, &["Record 1", "Record 2"]);
        assert_eq!(rows_named(&walked, "Delete"), [Row::Lineage, Row::Lineage]);
    }

    #[test]
    fn a_control_in_an_unnamed_row_is_unbound_even_when_its_row_is_the_only_one() {
        for records in [&["Record 1", "Record 2"][..], &["Record 1"][..]] {
            let walked = list(|_| String::new(), records);
            let unbound = rows_named(&walked, "Delete");
            assert!(
                unbound
                    .iter()
                    .all(|row| *row == Row::Unbound(Unbound::UnnamedRow)),
                "{records:?}: {unbound:?}"
            );
        }
    }

    #[test]
    fn named_rows_of_different_shapes_each_bind_their_own_controls() {
        let walked = walked(&[
            (0, LIST, ""),
            (1, LIST_ITEM, "Record 1"),
            (2, LABEL, "Record 1"),
            (2, BUTTON, "Delete"),
            (1, LIST_ITEM, "Record 2"),
            (2, LABEL, "Record 2"),
            (2, IMAGE, "Warning"),
            (2, PANEL, ""),
            (3, BUTTON, "Delete"),
            (3, LABEL, "0:42"),
        ]);
        assert_eq!(rows_named(&walked, "Delete"), [Row::Lineage, Row::Lineage]);
    }

    #[test]
    fn every_row_role_binds_by_its_name_and_nothing_without_one() {
        for role in [LIST_ITEM, TABLE_ROW, TREE_ITEM] {
            for (name, bound) in [
                ("Record 1", Row::Lineage),
                ("", Row::Unbound(Unbound::UnnamedRow)),
            ] {
                let walked = walked(&[(0, role, name), (1, BUTTON, "Open")]);
                assert_eq!(rows_named(&walked, "Open"), [bound], "role {role}");
            }
        }
    }

    // Each record's row is a panel, not a row, holding its label and two wrapped `Delete`
    // buttons, one beside a static label: no wrapper may pass for the record.
    #[test]
    fn look_alike_controls_outside_rows_are_unbound_however_they_are_wrapped() {
        let mut tree = vec![(0, FILLER, "")];
        for record in ["Record 1", "Record 2"] {
            tree.extend([
                (1, PANEL, ""),
                (2, LABEL, record),
                (2, PANEL, ""),
                (3, FILLER, ""),
                (4, LABEL, "Static icon"),
                (4, BUTTON, "Delete"),
                (3, FILLER, ""),
                (4, BUTTON, "Delete"),
            ]);
        }
        let walked = walked(&tree);
        let unbound = rows_named(&walked, "Delete");
        assert_eq!(unbound.len(), 4);
        assert!(
            unbound
                .iter()
                .all(|row| *row == Row::Unbound(Unbound::Repeated))
        );
    }

    #[test]
    fn a_unique_control_outside_rows_keeps_the_lineage_check_and_a_named_row_its_own_name() {
        let walked = walked(&[
            (0, PANEL, ""),
            (1, BUTTON, "Save"),
            (1, LIST, ""),
            (2, LIST_ITEM, "Record 1"),
        ]);
        assert_eq!(rows_named(&walked, "Save"), [Row::Lineage]);
        assert_eq!(rows_named(&walked, "Record 1"), [Row::Own]);
    }

    // Only a ref bound for want of a look-alike depends on how far the walk read.
    #[test]
    fn only_a_unique_control_outside_rows_is_bound_by_the_count() {
        let walked = walked(&[
            (0, PANEL, ""),
            (1, BUTTON, "Save"),
            (1, BUTTON, "Delete"),
            (1, BUTTON, "Delete"),
            (1, LIST, ""),
            (2, LIST_ITEM, "Record 1"),
            (3, BUTTON, "Open"),
        ]);
        let counted: Vec<bool> = (0..walked.nodes.len())
            .map(|at| walked.bound_by_count(at))
            .collect();
        assert_eq!(counted, [true, true, false, false, true, false, false]);
    }

    // The 11.8 s regression: finding every node's row asks once per node whether it is a
    // row, however deep the tree, where walking up from each node of this chain would ask
    // about two million times.
    #[test]
    fn every_nodes_row_is_found_in_one_pass_over_a_deep_chain() {
        let places: Vec<(Option<usize>, i32)> = (0..2000_usize)
            .map(|index| (index.checked_sub(1), 0))
            .collect();
        let asked = Cell::new(0);
        let found = rows(&places, |index| {
            asked.set(asked.get() + 1);
            index % 500 == 0
        });
        assert_eq!(asked.get(), 2000);
        assert_eq!(found.get(1999), Some(&Some(1500)));
        assert_eq!(found.get(499), Some(&Some(0)));
    }
}
