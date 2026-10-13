//! Which `DoAction` index runs an action `elements` listed. `DoAction` takes an action's
//! own index, which needn't be its place in `GetActions`' list: GTK 4 leaves disabled
//! actions out of the list and still counts them. So the index is the one whose action
//! has the listed name, asked by the method whose names the list holds, never by the
//! other: GTK 4 and at-spi2-atk list what `GetLocalizedName` gives and Qt, which has no
//! `GetLocalizedName`, what `GetName` gives (docs/decisions.md). Anything less than one
//! such index, known to be the only one, maps to none.

use crate::a11y::ActionNames;

/// Where the name `elements` gave an action comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// `GetActions`' list. Its names are `GetLocalizedName`'s, or `GetName`'s in an app that
    /// refuses `GetLocalizedName`.
    List,
    /// `GetName` at the action's place, for one the list leaves unnamed, as Chromium does.
    Own,
}

/// An action's name and the method that gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Named {
    Localized(String),
    Own(String),
}

impl Named {
    fn name(&self) -> &str {
        match self {
            Self::Localized(name) | Self::Own(name) => name,
        }
    }
}

/// The name an action has where `source` takes its names: for `List`, `GetLocalizedName`'s,
/// or `GetName`'s where the app refuses `GetLocalizedName`; for `Own`, `GetName`'s. `None`
/// when the app refuses the method that would name it.
pub(crate) fn named(names: &ActionNames, source: Source) -> Option<Named> {
    match (source, &names.localized, &names.own) {
        (Source::List, Some(localized), _) => Some(Named::Localized(localized.clone())),
        (_, _, Some(own)) => Some(Named::Own(own.clone())),
        _ => None,
    }
}

/// Whether `names`, at `index`, are past an element's last action, which lists `listed`
/// actions: at or after that many, neither method names one there. GTK 4 and Qt answer
/// both with an error past the end; at-spi2-atk answers both with an empty name. One
/// method's error alone, or both's inside the list, is not the end.
pub(crate) fn past_end(index: usize, listed: usize, names: &ActionNames) -> bool {
    let nameless = |name: &Option<String>| name.as_deref().is_none_or(str::is_empty);
    index >= listed && nameless(&names.own) && nameless(&names.localized)
}

/// The one index in `table`, the names of each of an element's actions in index order,
/// whose action is named `name` where `source` takes its names, and that name. `None`
/// when no index or several are, when an action's name there is unknown, or when the
/// list's names would come from both methods: some by `GetLocalizedName` and others by
/// `GetName` where the app refused it.
pub(crate) fn find(table: &[ActionNames], source: Source, name: &str) -> Option<(usize, Named)> {
    let named = table
        .iter()
        .map(|names| named(names, source))
        .collect::<Option<Vec<_>>>()?;
    let localized = named.iter().any(|n| matches!(n, Named::Localized(_)));
    let own = named.iter().any(|n| matches!(n, Named::Own(_)));
    if localized && own {
        return None;
    }
    let mut matching = named
        .into_iter()
        .enumerate()
        .filter(|(_, named)| named.name() == name);
    let found = matching.next()?;
    matching.next().is_none().then_some(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(own: Option<&str>, localized: Option<&str>) -> ActionNames {
        ActionNames {
            own: own.map(str::to_owned),
            localized: localized.map(str::to_owned),
        }
    }

    #[test]
    fn an_elements_actions_end_where_neither_method_names_one_past_the_list() {
        let refused = names(None, None);
        let empty = names(Some(""), Some(""));
        // GTK 4 and Qt refuse both, at-spi2-atk names neither.
        assert!(past_end(1, 1, &refused));
        assert!(past_end(2, 1, &empty));
        // Inside the list an action is there, whatever the app answers.
        assert!(!past_end(0, 1, &refused));
        assert!(!past_end(0, 1, &empty));
        // One method naming an action is one more action.
        assert!(!past_end(1, 1, &names(None, Some("Klicken"))));
        assert!(!past_end(1, 1, &names(Some("click"), Some(""))));
    }

    #[test]
    fn no_action_is_found_while_another_ones_name_is_unknown() {
        // Inside the list, an action the app names by neither method may have the name too.
        let refused = [names(Some("click"), Some("Klicken")), names(None, None)];
        assert_eq!(find(&refused, Source::List, "Klicken"), None);
        // A name filled from `GetName` is compared only with `GetName`, which the second
        // action's is unknown for.
        let unnamed = [names(Some("click"), Some("")), names(None, Some("Drücken"))];
        assert_eq!(find(&unnamed, Source::Own, "click"), None);
    }
}
