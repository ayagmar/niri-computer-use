//! Tool-call closing tags an agent leaked into the end of `type_text`'s or
//! `set_element_text`'s text. Models sometimes carry their own tool-call markup, such as
//! `[/parameter]` or `[/invoke]` (with angle brackets; written with square ones here), past
//! the end of a long argument, and a tag typed into a chat box with `submit` can't be
//! taken back. Only a trailing run counts, only of known tag names, and only the closing
//! tags after the last one that an opening tag of the same name earlier in the text
//! balances, each opening tag balancing one closing tag, so a document that opens and
//! closes such tags goes out as it is.

use std::fmt;

/// The tag names of agents' tool-call formats.
const NAMES: [&str; 10] = [
    "text",
    "parameter",
    "invoke",
    "function_calls",
    "function_call",
    "tool_use",
    "tool_call",
    "antml:parameter",
    "antml:invoke",
    "antml:function_calls",
];

/// A trailing run of leaked closing tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Leaked {
    /// How many tags the run holds.
    pub(super) tags: usize,
    /// Each tag name once, in the order the run first has it.
    pub(super) names: Vec<&'static str>,
}

/// The closing tags with known names at the end of `text`, ignoring whitespace between and
/// after them, after the last one an opening tag earlier in `text` balances; none when the
/// last is balanced or `text` doesn't end with one.
pub(crate) fn leaked(text: &str) -> Option<Leaked> {
    let (before, run) = trailing_run(text);
    let mut open = unclosed(before);
    // Each opening tag still open before the run balances one of its closing tags, in
    // order. A leak is at the very end, so a balanced tag ends any leak before it.
    let mut last_balanced = None;
    for (at, &(index, _)) in run.iter().enumerate() {
        if let Some(depth) = open.get_mut(index).filter(|depth| **depth > 0) {
            *depth -= 1;
            last_balanced = Some(at);
        }
    }
    let leaked = run
        .get(last_balanced.map_or(0, |at| at + 1)..)
        .filter(|leaked| !leaked.is_empty())?;
    let mut names = Vec::new();
    for &(_, name) in leaked {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    Some(Leaked {
        tags: leaked.len(),
        names,
    })
}

/// The text before the run of known closing tags `text` ends with, and the run in order:
/// each tag's index in `NAMES` and its name as listed there.
fn trailing_run(text: &str) -> (&str, Vec<(usize, &'static str)>) {
    let mut head = text.trim_end();
    let mut run = Vec::new();
    while let Some((before, index, name)) = last_closing(head) {
        run.push((index, name));
        head = before.trim_end();
    }
    run.reverse();
    (head, run)
}

/// What comes before the closing tag `head` ends with, and the tag's name, when it is a
/// known one: its index in `NAMES` and the name as listed there.
fn last_closing(head: &str) -> Option<(&str, usize, &'static str)> {
    let (before, tag) = head.rsplit_once("</")?;
    let written = tag.strip_suffix('>')?;
    let (index, name) = NAMES
        .iter()
        .enumerate()
        .find(|(_, name)| written.eq_ignore_ascii_case(name))?;
    Some((before, index, name))
}

/// How many tags of each name `text` leaves open, matched without regard to ASCII case,
/// in one pass: an opening tag, `<name` followed by `>`, whitespace or the end, opens
/// one, and a closing tag closes the latest still open, if any.
fn unclosed(text: &str) -> [usize; NAMES.len()] {
    let mut open = [0_usize; NAMES.len()];
    for (at, _) in text.match_indices('<') {
        let rest = text.as_bytes().get(at + 1..).unwrap_or_default();
        let (closing, rest) = rest
            .strip_prefix(b"/")
            .map_or((false, rest), |rest| (true, rest));
        for (depth, name) in open.iter_mut().zip(NAMES) {
            let named = rest
                .get(..name.len())
                .is_some_and(|written| written.eq_ignore_ascii_case(name.as_bytes()));
            let next = rest.get(name.len()).copied();
            if !named {
                continue;
            }
            if closing && next == Some(b'>') {
                *depth = depth.saturating_sub(1);
            } else if !closing && next.is_none_or(|next| next == b'>' || next.is_ascii_whitespace())
            {
                *depth += 1;
            }
        }
    }
    open
}

impl fmt::Display for Leaked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tags = self
            .names
            .iter()
            .map(|name| format!("`</{name}>`"))
            .collect::<Vec<_>>()
            .join(", ");
        if self.tags == 1 {
            write!(
                f,
                "`text` ends with {tags}, a tool-call closing tag that no opening tag before it balances, which agents leak from their own tool calls by mistake; send the text again without it"
            )
        } else {
            write!(
                f,
                "`text` ends with {} tool-call closing tags that no opening tags before them balance ({tags}), which agents leak from their own tool calls by mistake; send the text again without them",
                self.tags
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// The closing tag of `name`, built so this source never holds a tool-call tag.
    fn close(name: &str) -> String {
        format!("<{}{name}>", '/')
    }

    fn names(text: &str) -> Vec<&'static str> {
        leaked(text).map(|leak| leak.names).unwrap_or_default()
    }

    #[test]
    fn a_trailing_run_of_tool_call_tags_is_found_across_whitespace() {
        let text = format!(
            "End of the note.\n{}\n{}\n",
            close("parameter"),
            close("invoke")
        );
        let leak = leaked(&text).unwrap();
        assert_eq!(leak.tags, 2);
        assert_eq!(leak.names, ["parameter", "invoke"]);
        let namespaced = ["antml:parameter", "antml:invoke", "antml:function_calls"];
        let body = format!("Body {}", namespaced.map(close).join("\n"));
        assert_eq!(names(&body), namespaced);
        assert_eq!(names(&format!("ok{}", close("TOOL_CALL"))), ["tool_call"]);
    }

    #[test]
    fn repeated_tags_are_counted_and_named_once() {
        let text = format!("a{}{}{}", close("text"), close("invoke"), close("text"));
        let leak = leaked(&text).unwrap();
        assert_eq!(leak.tags, 3);
        assert_eq!(leak.names, ["text", "invoke"]);
    }

    #[test]
    fn text_that_doesnt_end_with_a_known_closing_tag_is_left_alone() {
        let invoke = close("invoke");
        for text in [
            String::new(),
            "   ".to_owned(),
            "Lorem ipsum.".to_owned(),
            "Hello </div>".to_owned(),
            "</other>".to_owned(),
            "<invoke/>".to_owned(),
            format!("a {invoke} b"),
            invoke.replace('>', " >"),
            invoke.replace('/', "/ "),
            invoke.replace('>', ""),
        ] {
            assert_eq!(leaked(&text), None, "{text:?}");
        }
    }

    #[test]
    fn a_tag_opened_earlier_is_document_content() {
        let (text, invoke) = (close("text"), close("invoke"));
        assert_eq!(leaked(&format!("<text>quoted{text}")), None);
        let parameter = format!(r#"<parameter name="x">v{}"#, close("parameter"));
        assert_eq!(leaked(&parameter), None);
        assert_eq!(leaked(&format!("<INVOKE>\nv\n{invoke}")), None);
        // Only the run after the balanced tag leaked.
        assert_eq!(names(&format!("<text>quoted{text}\n{invoke}")), ["invoke"]);
        // A longer name starting the same isn't an opener.
        assert_eq!(names(&format!("<texts>a{text}")), ["text"]);
        // An opener after the closing tag doesn't balance it.
        assert_eq!(names(&format!("a{text} <text> {invoke}")), ["invoke"]);
    }

    /// Found in review: one opening tag let every later closing tag of its name through.
    #[test]
    fn each_opening_tag_balances_only_one_closing_tag() {
        let (text, parameter, invoke) = (close("text"), close("parameter"), close("invoke"));
        let doubled = leaked(&format!("<text>a{text}{text}")).unwrap();
        assert_eq!((doubled.tags, doubled.names), (1, vec!["text"]));
        // A quoted tool call, then the agent's own leaked tags.
        let quoted = format!(r#"<invoke name="x"><parameter name="y">v{parameter}{invoke}"#);
        let leak = leaked(&format!("{quoted}\n{parameter}\n{invoke}")).unwrap();
        assert_eq!((leak.tags, leak.names), (2, vec!["parameter", "invoke"]));
        assert_eq!(leaked(&quoted), None);
    }

    /// Found in review: the run was reported from its first unbalanced tag, so the
    /// document's own last closing tag was named with the stray one before it.
    #[test]
    fn a_balanced_last_tag_ends_the_leak() {
        let (text, invoke) = (close("text"), close("invoke"));
        assert_eq!(leaked(&format!("<text>a{invoke}{text}")), None);
        assert_eq!(names(&format!("<text>a{invoke}{text}{invoke}")), ["invoke"]);
    }

    #[test]
    fn a_closing_tag_before_an_opening_tag_leaves_it_open() {
        let text = close("text");
        assert_eq!(leaked(&format!("{text}<text>a{text}")), None);
        assert_eq!(
            names(&format!("<text>{text}{text}<text>a{text}")),
            Vec::<&str>::new()
        );
        assert_eq!(names(&format!("<text>{text}a{text}")), ["text"]);
    }

    #[test]
    fn inner_markup_stays_and_only_the_tail_is_named() {
        let text = format!("Hello <b>world</b>\n{}", close("text"));
        assert_eq!(names(&text), ["text"]);
    }

    #[test]
    fn the_detail_names_the_tags_and_says_to_resend_without_them() {
        let invoke = close("invoke");
        let one = leaked(&format!("x{invoke}")).unwrap().to_string();
        assert!(
            one.starts_with(&format!("`text` ends with `{invoke}`, a")),
            "{one}"
        );
        assert!(one.ends_with("without it"), "{one}");
        let run = format!("x{}{invoke}{invoke}", close("parameter"));
        let three = leaked(&run).unwrap().to_string();
        let named = format!("(`{}`, `{invoke}`)", close("parameter"));
        assert!(three.contains(&named), "{three}");
        assert!(three.starts_with("`text` ends with 3 "), "{three}");
    }

    /// A paste of 1 MiB of tags stays linear: each name's opener is looked up once.
    #[test]
    fn a_long_run_is_found_in_one_pass() {
        let text = close("text").repeat(150_000);
        let leak = leaked(&text).unwrap();
        assert_eq!(leak.tags, 150_000);
        assert_eq!(leak.names, ["text"]);
    }

    /// `name` with its ASCII letters' case picked by the bits of `cases`.
    fn recased(name: &str, cases: u64) -> String {
        name.chars()
            .enumerate()
            .map(|(at, c)| {
                if (cases >> (at % 64)) & 1 == 1 {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect()
    }

    /// A piece of text, with the name it closes when it is a known closing tag: free words,
    /// whitespace, a known closing or opening tag in any case, or an unknown closing tag.
    fn piece() -> impl Strategy<Value = (String, Option<&'static str>)> {
        let name = || prop::sample::select(NAMES.to_vec());
        prop_oneof![
            "[^<]{0,6}".prop_map(|words| (words, None)),
            "[ \\t\\n]{1,2}".prop_map(|space| (space, None)),
            (name(), any::<u64>())
                .prop_map(|(name, cases)| (close(&recased(name, cases)), Some(name))),
            (
                name(),
                any::<u64>(),
                prop::sample::select(vec![">", " a=\"b\">"])
            )
                .prop_map(|(name, cases, end)| (format!("<{}{end}", recased(name, cases)), None)),
            Just((close("div"), None)),
        ]
    }

    proptest! {
        #![proptest_config(crate::test_support::properties(512))]

        /// A known closing tag after any text that holds no tag, in any case and with any
        /// whitespace after it, is caught and named.
        #[test]
        fn a_trailing_tag_nothing_opens_is_caught(
            text in "[^<]{0,40}",
            name in prop::sample::select(NAMES.to_vec()),
            cases in any::<u64>(),
            after in "[ \t\n]{0,3}",
        ) {
            let sent = format!("{text}{}{after}", close(&recased(name, cases)));
            let leak = leaked(&sent);
            prop_assert_eq!(leak.map(|leak| leak.names), Some(vec![name]));
        }

        /// The same tag after an opening tag of its name, in any case and with or without
        /// attributes, is document content.
        #[test]
        fn a_tag_opened_before_it_is_left_alone(
            (before, inside) in ("[^<]{0,20}", "[^<]{0,20}"),
            name in prop::sample::select(NAMES.to_vec()),
            cases in any::<u64>(),
            attributes in prop::sample::select(vec![">", " name=\"x\">", "\n>"]),
        ) {
            let opened = recased(name, cases);
            let sent = format!("{before}<{opened}{attributes}{inside}{}", close(name));
            prop_assert_eq!(leaked(&sent), None);
        }

        /// Text of free words, whitespace, and known and unknown tags: only the closing tags
        /// after its last other character are ever named, and with no opening tag in the
        /// text every one of them is.
        #[test]
        fn only_the_tags_at_the_end_are_named(pieces in prop::collection::vec(piece(), 0..8)) {
            let text: String = pieces.iter().map(|(piece, _)| piece.as_str()).collect();
            let mut tail = Vec::new();
            for (piece, closes) in pieces.iter().rev() {
                match closes {
                    Some(name) => tail.push(*name),
                    None if piece.trim().is_empty() => {}
                    None => break,
                }
            }
            tail.reverse();
            let leak = leaked(&text);
            if let Some(leak) = &leak {
                prop_assert!(leak.tags <= tail.len(), "{text:?}");
                prop_assert!(leak.names.iter().all(|name| tail.contains(name)), "{text:?}");
            }
            let opens = pieces.iter().any(|(piece, _)| piece.starts_with('<') && !piece.starts_with("</"));
            if !opens {
                let mut names = Vec::new();
                for name in &tail {
                    if !names.contains(name) {
                        names.push(*name);
                    }
                }
                let wanted = (!tail.is_empty()).then_some(Leaked { tags: tail.len(), names });
                prop_assert_eq!(leak, wanted, "{:?}", text);
            }
        }
    }
}
