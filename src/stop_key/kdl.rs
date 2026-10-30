//! A tolerant reader of the KDL 1 that niri's config is written in, enough to find its
//! `include`s and `binds`: nodes with their arguments, properties and children. Comments
//! (`//`, nested `/* */`, and `/-` on a node, an entry or a children block), quoted and raw
//! strings, type annotations and line continuations are understood; values are kept as
//! their text, since only strings matter here. Anything it can't read, such as an
//! unterminated string or an unbalanced brace, is an error, as niri would refuse the file.

/// One node: `name arg… key=value… { children }`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Node {
    pub(super) name: String,
    pub(super) args: Vec<String>,
    pub(super) props: Vec<(String, String)>,
    pub(super) children: Vec<Self>,
}

impl Node {
    /// The last value of property `key`, as KDL takes it.
    pub(super) fn prop(&self, key: &str) -> Option<&str> {
        self.props
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A bare word: an identifier, a number, `true`, `false` or `null`.
    Word(String),
    Str(String),
    Equals,
    Open,
    Close,
    /// `;` or a line break.
    End,
    SlashDash,
}

/// The nodes of `text`, or what keeps it from being read.
pub(super) fn parse(text: &str) -> Result<Vec<Node>, String> {
    let tokens = tokenize(text)?;
    let mut parser = Parser {
        tokens: &tokens,
        at: 0,
    };
    let nodes = parser.nodes()?;
    match parser.next() {
        None => Ok(nodes),
        Some(_) => Err("a } closes no block".to_owned()),
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn next(&mut self) -> Option<&Token> {
        let token = self.tokens.get(self.at);
        self.at += 1;
        token
    }

    /// Nodes up to a `}` or the end, which is left unread.
    fn nodes(&mut self) -> Result<Vec<Node>, String> {
        let mut nodes = Vec::new();
        loop {
            match self.peek() {
                None | Some(Token::Close) => return Ok(nodes),
                Some(Token::End) => {
                    self.at += 1;
                }
                Some(Token::SlashDash) => {
                    self.at += 1;
                    self.skip_ends();
                    self.node()?;
                }
                Some(_) => nodes.push(self.node()?),
            }
        }
    }

    fn skip_ends(&mut self) {
        while self.peek() == Some(&Token::End) {
            self.at += 1;
        }
    }

    fn node(&mut self) -> Result<Node, String> {
        let mut node = Node {
            name: self.value("a node name")?,
            ..Node::default()
        };
        loop {
            match self.peek() {
                None | Some(Token::Close) => return Ok(node),
                Some(Token::End) => {
                    self.at += 1;
                    return Ok(node);
                }
                Some(Token::Open) => {
                    node.children = self.children()?;
                }
                Some(Token::SlashDash) => self.skip_entry()?,
                Some(_) => self.entry(&mut node)?,
            }
        }
    }

    /// The entry or children block after a `/-`, read and dropped.
    fn skip_entry(&mut self) -> Result<(), String> {
        self.at += 1;
        if self.peek() == Some(&Token::Open) {
            self.children()?;
        } else {
            self.entry(&mut Node::default())?;
        }
        Ok(())
    }

    /// A `{ … }` block, braces included.
    fn children(&mut self) -> Result<Vec<Node>, String> {
        self.at += 1;
        let children = self.nodes()?;
        match self.next() {
            Some(Token::Close) => Ok(children),
            _ => Err("a { is never closed".to_owned()),
        }
    }

    /// An argument or a `key=value` property, added to `node`.
    fn entry(&mut self, node: &mut Node) -> Result<(), String> {
        let first = self.value("an argument")?;
        if self.peek() == Some(&Token::Equals) {
            self.at += 1;
            let value = self.value("a property's value")?;
            node.props.push((first, value));
        } else {
            node.args.push(first);
        }
        Ok(())
    }

    fn value(&mut self, what: &str) -> Result<String, String> {
        match self.next() {
            Some(Token::Word(text) | Token::Str(text)) => Ok(text.clone()),
            other => Err(format!("expected {what}, found {other:?}")),
        }
    }
}

fn tokenize(text: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\n' | '\r' | '\u{85}' | '\u{c}' | '\u{2028}' | '\u{2029}' | ';' => {
                tokens.push(Token::End);
            }
            '{' => tokens.push(Token::Open),
            '}' => tokens.push(Token::Close),
            '=' => tokens.push(Token::Equals),
            '"' => tokens.push(Token::Str(quoted(&mut chars)?)),
            '(' => annotation(&mut chars)?,
            '\\' => continuation(&mut chars)?,
            '/' => match chars.next() {
                Some('/') => line_comment(&mut chars),
                Some('*') => block_comment(&mut chars)?,
                Some('-') => tokens.push(Token::SlashDash),
                _ => return Err("a / starts no comment".to_owned()),
            },
            'r' if matches!(chars.peek(), Some('"' | '#')) => {
                tokens.push(Token::Str(raw(&mut chars)?));
            }
            c if c.is_whitespace() || c == '\u{feff}' => {}
            c => tokens.push(Token::Word(word(c, &mut chars))),
        }
    }
    Ok(tokens)
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// Whether `c` ends a bare word.
fn ends_word(c: char) -> bool {
    c.is_whitespace() || "\\/(){}<>;[]=,\"".contains(c)
}

fn word(first: char, chars: &mut Chars<'_>) -> String {
    let mut word = String::from(first);
    while let Some(&c) = chars.peek() {
        if ends_word(c) {
            break;
        }
        word.push(c);
        chars.next();
    }
    word
}

/// A quoted string after its opening `"`, with its escapes resolved.
fn quoted(chars: &mut Chars<'_>) -> Result<String, String> {
    let mut text = String::new();
    loop {
        match chars.next() {
            None => return Err("a string is never closed".to_owned()),
            Some('"') => return Ok(text),
            Some('\\') => text.push(escape(chars)?),
            Some(c) => text.push(c),
        }
    }
}

fn escape(chars: &mut Chars<'_>) -> Result<char, String> {
    Ok(match chars.next() {
        Some('n') => '\n',
        Some('r') => '\r',
        Some('t') => '\t',
        Some('b') => '\u{8}',
        Some('f') => '\u{c}',
        Some(c @ ('\\' | '/' | '"')) => c,
        Some('u') => {
            if chars.next() != Some('{') {
                return Err("a \\u escape lacks its {".to_owned());
            }
            let hex: String = chars.by_ref().take_while(|&c| c != '}').collect();
            u32::from_str_radix(&hex, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| format!("\\u{{{hex}}} is no character"))?
        }
        other => return Err(format!("unknown escape \\{other:?}")),
    })
}

/// A raw string after its `r`: `"…"`, or `#"…"#` with as many `#` on each side.
fn raw(chars: &mut Chars<'_>) -> Result<String, String> {
    let mut hashes = 0;
    while chars.next_if_eq(&'#').is_some() {
        hashes += 1;
    }
    if chars.next() != Some('"') {
        return Err("a raw string lacks its opening quote".to_owned());
    }
    let mut text = String::new();
    while let Some(c) = chars.next() {
        if c == '"' {
            let mut closing = 0;
            while closing < hashes && chars.next_if_eq(&'#').is_some() {
                closing += 1;
            }
            if closing == hashes {
                return Ok(text);
            }
            text.push('"');
            text.extend(std::iter::repeat_n('#', closing));
        } else {
            text.push(c);
        }
    }
    Err("a raw string is never closed".to_owned())
}

/// A `(type)` annotation after its `(`, which nothing here reads.
fn annotation(chars: &mut Chars<'_>) -> Result<(), String> {
    match chars.peek() {
        Some('"') => {
            chars.next();
            quoted(chars)?;
        }
        _ => while chars.next_if(|&c| !ends_word(c)).is_some() {},
    }
    match chars.next() {
        Some(')') => Ok(()),
        _ => Err("a type annotation is never closed".to_owned()),
    }
}

/// A `\` line continuation: spaces and a comment may follow it before the line break.
fn continuation(chars: &mut Chars<'_>) -> Result<(), String> {
    while chars.next_if(|&c| c.is_whitespace() && c != '\n').is_some() {}
    if chars.next_if_eq(&'/').is_some() {
        if chars.next() != Some('/') {
            return Err("a \\ is followed by something other than a line break".to_owned());
        }
        line_comment(chars);
        return Ok(());
    }
    match chars.next() {
        Some('\n') | None => Ok(()),
        Some(_) => Err("a \\ is followed by something other than a line break".to_owned()),
    }
}

fn line_comment(chars: &mut Chars<'_>) {
    while chars.next_if(|&c| c != '\n').is_some() {}
    chars.next();
}

/// A `/* … */` comment after its opening, which may nest.
fn block_comment(chars: &mut Chars<'_>) -> Result<(), String> {
    let mut depth = 1_u32;
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('*', Some('/')) => {
                chars.next();
                depth -= 1;
                if depth == 0 {
                    return Ok(());
                }
            }
            ('/', Some('*')) => {
                chars.next();
                depth += 1;
            }
            _ => {}
        }
    }
    Err("a /* comment is never closed".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, args: &[&str]) -> Node {
        Node {
            name: name.to_owned(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            ..Node::default()
        }
    }

    #[test]
    fn reads_nodes_with_arguments_properties_and_children() {
        let text = r##"
binds {
    Mod+Shift+Escape allow-when-locked=true hotkey-overlay-title="Stop \"it\"" { spawn "/bin/x" "stop"; }
    Mod+T { spawn r#"foot"#; }
}
include optional=true "a.kdl"
"##;
        let nodes = parse(text).unwrap();
        assert_eq!(nodes.len(), 2);
        let stop = &nodes[0].children[0];
        assert_eq!(stop.name, "Mod+Shift+Escape");
        assert_eq!(stop.prop("hotkey-overlay-title"), Some("Stop \"it\""));
        assert_eq!(stop.prop("allow-when-locked"), Some("true"));
        assert_eq!(stop.children, [node("spawn", &["/bin/x", "stop"])]);
        assert_eq!(nodes[0].children[1].children, [node("spawn", &["foot"])]);
        assert_eq!(nodes[1].args, ["a.kdl"]);
        assert_eq!(nodes[1].prop("optional"), Some("true"));
    }

    #[test]
    fn leaves_out_what_comments_and_slashdash_remove() {
        let text = r#"
// binds { Mod+X { spawn "gone" "stop"; } }
/* one /* nested */ still a comment */
/-binds {
    Mod+Y { spawn "gone"; }
}
binds {
    /- Mod+Z { spawn "gone"; }
    Mod+A /-{ spawn "gone"; } { spawn "kept" /-"gone"; }
    Mod+B (t)"typed" \
        { spawn "joined"; }
}
"#;
        let nodes = parse(text).unwrap();
        assert_eq!(nodes.len(), 1);
        let binds = &nodes[0].children;
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].children, [node("spawn", &["kept"])]);
        assert_eq!(binds[1].args, ["typed"]);
        assert_eq!(binds[1].children, [node("spawn", &["joined"])]);
    }

    #[test]
    fn refuses_what_niri_could_not_read() {
        for text in [
            "binds {",
            "binds { }\n}",
            "node \"open",
            "/* open",
            "node r#\"open\"",
            "node =",
            "node / x",
            "node \"\\q\"",
            "node (open",
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
    }
}
