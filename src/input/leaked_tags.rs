//! Tool-call closing tags an agent leaked into the end of `type_text`'s text. Models
//! sometimes carry their own tool-call markup, such as `[/parameter]` or `[/invoke]` (with
//! angle brackets; written with square ones here), past the end of a long argument, and a
//! tag typed into a chat box with `submit` can't be taken back. Only a trailing run counts,
//! only of known tag names, and only closing tags that no opening tag of the same name
//! earlier in the text balances, so a document that opens and closes such a tag goes out
//! as it is.

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
pub(super) struct Leaked {
    /// How many tags the run holds.
    pub(super) tags: usize,
    /// Each tag name once, in the order the run first has it.
    pub(super) names: Vec<&'static str>,
}

/// The trailing run of closing tags with known names that nothing earlier in `text`
/// opens, ignoring whitespace between and after them; none when `text` doesn't end with
/// one.
pub(super) fn leaked(text: &str) -> Option<Leaked> {
    // Each name's first opening tag in `text`, looked up once, which keeps a long run of
    // tags linear in the text's length.
    let mut openers: [Option<Option<usize>>; NAMES.len()] = [None; NAMES.len()];
    let mut head = text.trim_end();
    let mut found = Vec::new();
    while let Some((before, index, name)) = last_closing(head) {
        let opener = openers
            .get_mut(index)
            .and_then(|slot| *slot.get_or_insert_with(|| first_opener(text, name)));
        if opener.is_some_and(|at| at < before.len()) {
            break;
        }
        found.push(name);
        head = before.trim_end();
    }
    let tags = found.len();
    let mut names = Vec::new();
    for name in found.into_iter().rev() {
        if !names.contains(&name) {
            names.push(name);
        }
    }
    (tags > 0).then_some(Leaked { tags, names })
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

/// Where `text` first opens a `name` tag, matched without regard to ASCII case: `<name`
/// followed by `>`, whitespace or the end of the text.
fn first_opener(text: &str, name: &str) -> Option<usize> {
    let name = name.as_bytes();
    text.match_indices('<').map(|(at, _)| at).find(|&at| {
        let rest = text.as_bytes().get(at + 1..).unwrap_or_default();
        rest.get(..name.len())
            .is_some_and(|written| written.eq_ignore_ascii_case(name))
            && rest
                .get(name.len())
                .is_none_or(|next| *next == b'>' || next.is_ascii_whitespace())
    })
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
                "`text` ends with {tags}, a tool-call closing tag that nothing before it opens, which agents leak from their own tool calls by mistake; send the text again without it"
            )
        } else {
            write!(
                f,
                "`text` ends with {} tool-call closing tags that nothing before them opens ({tags}), which agents leak from their own tool calls by mistake; send the text again without them",
                self.tags
            )
        }
    }
}

#[cfg(test)]
mod tests {
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
}
