//! The part of the XDG Desktop Entry Specification 1.5 that `launch` needs: the
//! `[Desktop Entry]` group of one file, whether the entry is an application this desktop
//! shows, and its `Exec` line as an argv, with no shell and no files passed. Pure: the
//! caller reads the file and checks `TryExec`.

use std::collections::BTreeMap;

/// The keys of an entry that `launch` reads, decoded. Localized keys, such as
/// `Name[fr]`, are skipped: the unlocalized value is the one used.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each is one of the spec's boolean keys, read as the file writes it"
)]
pub(crate) struct Entry {
    /// `Type`.
    pub(crate) kind: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) icon: Option<String>,
    /// `Exec`, with the string escapes decoded and the quoting still in it.
    pub(crate) exec: Option<String>,
    pub(crate) try_exec: Option<String>,
    pub(crate) hidden: bool,
    pub(crate) no_display: bool,
    pub(crate) terminal: bool,
    pub(crate) dbus_activatable: bool,
    /// `OnlyShowIn`; none when the key is absent, which differs from an empty list.
    pub(crate) only_show_in: Option<Vec<String>>,
    pub(crate) not_show_in: Vec<String>,
    pub(crate) startup_wm_class: Option<String>,
}

/// An entry `launch` may start: its command, and `TryExec`, which the caller looks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Startable {
    pub(crate) argv: Vec<String>,
    pub(crate) try_exec: Option<String>,
    /// `StartupWMClass`, the other `app_id` its windows may have besides the entry's id.
    pub(crate) wm_class: Option<String>,
}

/// Reads the `[Desktop Entry]` group of a file's text. Anything before it but comments
/// and blank lines, a line that is neither a key, a group nor a comment, a key given twice,
/// or a boolean other than `true` or `false` makes the file invalid, as the spec says.
pub(crate) fn parse(text: &str) -> Result<Entry, String> {
    let keys = group(text)?;
    let string = |key: &str| keys.get(key).map(|value| unescape(value));
    let boolean = |key: &str| match keys.get(key).copied() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(other) => Err(format!("{key} is {other:?}, not true or false")),
    };
    Ok(Entry {
        kind: string("Type"),
        name: string("Name"),
        icon: string("Icon"),
        exec: string("Exec"),
        try_exec: string("TryExec"),
        hidden: boolean("Hidden")?,
        no_display: boolean("NoDisplay")?,
        terminal: boolean("Terminal")?,
        dbus_activatable: boolean("DBusActivatable")?,
        only_show_in: keys.get("OnlyShowIn").map(|value| list(value)),
        not_show_in: keys
            .get("NotShowIn")
            .map_or_else(Vec::new, |value| list(value)),
        startup_wm_class: string("StartupWMClass"),
    })
}

/// The unlocalized keys of the `[Desktop Entry]` group, which must come first. The groups
/// after it, such as `[Desktop Action new-window]`, aren't read.
fn group(text: &str) -> Result<BTreeMap<&str, &str>, String> {
    let mut keys = BTreeMap::new();
    let mut started = false;
    for (number, line) in text.lines().enumerate() {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            if started {
                break;
            }
            if line.trim_end() != "[Desktop Entry]" {
                return Err("the first group isn't [Desktop Entry]".to_owned());
            }
            started = true;
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| started) else {
            return Err(format!(
                "line {} isn't a key in [Desktop Entry], a group or a comment",
                number + 1
            ));
        };
        let key = key.trim_end();
        if key.contains('[') {
            continue;
        }
        if keys.insert(key, value.trim_start()).is_some() {
            return Err(format!("{key} is given twice"));
        }
    }
    if started {
        Ok(keys)
    } else {
        Err("it has no [Desktop Entry] group".to_owned())
    }
}

/// A string value with the spec's escapes `\s`, `\n`, `\t`, `\r` and `\\` decoded. Any
/// other backslash is kept, for `Exec`'s own quoting rules to read.
fn unescape(value: &str) -> String {
    let mut decoded = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            decoded.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => decoded.push(' '),
            Some('n') => decoded.push('\n'),
            Some('t') => decoded.push('\t'),
            Some('r') => decoded.push('\r'),
            Some('\\') | None => decoded.push('\\'),
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
        }
    }
    decoded
}

/// A list value: strings separated by `;`, where `\;` is a literal `;`. The last `;` is
/// optional.
fn list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut item = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            ';' => items.push(unescape(&std::mem::take(&mut item))),
            '\\' if chars.as_str().starts_with(';') => {
                item.push(';');
                chars.next();
            }
            '\\' => {
                item.push('\\');
                if let Some(next) = chars.next() {
                    item.push(next);
                }
            }
            other => item.push(other),
        }
    }
    if !item.is_empty() {
        items.push(unescape(&item));
    }
    items
}

/// Whether `entry` is one `launch` may start on a desktop that `desktops` names, as
/// `XDG_CURRENT_DESKTOP` lists them, and its command, with `location`, the file's path,
/// for `%k`; otherwise why not. `TryExec` is left to the caller.
pub(crate) fn startable(
    entry: &Entry,
    desktops: &[String],
    location: &str,
) -> Result<Startable, String> {
    match entry.kind.as_deref() {
        Some("Application") => {}
        Some(other) => return Err(format!("is of Type {other}, not Application")),
        None => return Err("has no Type".to_owned()),
    }
    if entry.hidden {
        return Err("is hidden (Hidden=true), as a deleted entry is".to_owned());
    }
    if entry.no_display {
        return Err("is NoDisplay=true, so menus don't show it".to_owned());
    }
    if !shown_in(entry, desktops) {
        return Err(format!(
            "isn't shown on this desktop ({}) by its OnlyShowIn or NotShowIn",
            desktops.join(":")
        ));
    }
    if entry.terminal {
        return Err(
            "runs in a terminal (Terminal=true), and launch starts no terminal for it".to_owned(),
        );
    }
    let Some(exec) = &entry.exec else {
        return Err(if entry.dbus_activatable {
            "is started over D-Bus (DBusActivatable=true) and has no Exec line, which launch needs"
                .to_owned()
        } else {
            "has no Exec line".to_owned()
        });
    };
    let fields = Fields {
        name: entry.name.as_deref().unwrap_or_default(),
        icon: entry.icon.as_deref().unwrap_or_default(),
        location,
    };
    let argv = expand(split(exec)?, &fields)?;
    if argv.first().is_none_or(String::is_empty) {
        return Err("has an Exec line with no program".to_owned());
    }
    Ok(Startable {
        argv,
        try_exec: entry.try_exec.clone().filter(|program| !program.is_empty()),
        wm_class: entry
            .startup_wm_class
            .clone()
            .filter(|class| !class.is_empty()),
    })
}

/// The first desktop in `desktops` that `OnlyShowIn` or `NotShowIn` names decides, as `GLib`
/// does; when none is named, an entry with an `OnlyShowIn` isn't shown.
fn shown_in(entry: &Entry, desktops: &[String]) -> bool {
    for desktop in desktops {
        if entry
            .only_show_in
            .as_ref()
            .is_some_and(|only| only.contains(desktop))
        {
            return true;
        }
        if entry.not_show_in.contains(desktop) {
            return false;
        }
    }
    entry.only_show_in.is_none()
}

/// A piece of an `Exec` argument: text, or a field code such as `%U`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Text(String),
    Code(char),
}

/// Characters a shell would read unquoted. The spec says an argument with one must be
/// quoted, so an unquoted one means the line was written for a shell.
const RESERVED: &[char] = &[
    '\'', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')', '`',
];

/// Splits `Exec`, its string escapes already decoded, into arguments at unquoted spaces
/// and tabs. Inside double quotes, a backslash escapes `"`, `` ` ``, `$` and `\`. Outside
/// them, a backslash escapes any character, as `GLib` reads it.
fn split(exec: &str) -> Result<Vec<Vec<Part>>, String> {
    let mut args = Vec::new();
    let mut arg: Option<Vec<Part>> = None;
    let mut quoted = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        let text = match (quoted, c) {
            (false, ' ' | '\t' | '\n') => {
                args.extend(arg.take());
                continue;
            }
            (_, '"') => {
                quoted = !quoted;
                arg.get_or_insert_with(Vec::new);
                continue;
            }
            (_, '%') => {
                let part = field(chars.next())?;
                arg.get_or_insert_with(Vec::new).push(part);
                continue;
            }
            (true, '\\') => match chars.as_str().chars().next() {
                Some(next @ ('"' | '`' | '$' | '\\')) => {
                    chars.next();
                    next
                }
                _ => '\\',
            },
            (false, '\\') => chars
                .next()
                .ok_or_else(|| "Exec ends with a backslash".to_owned())?,
            (true, '$' | '`') => {
                return Err(format!(
                    "Exec has an unescaped {c} in quotes, which only a shell expands"
                ));
            }
            (false, _) if RESERVED.contains(&c) => {
                return Err(format!(
                    "Exec has an unquoted {c}, which only a shell reads"
                ));
            }
            _ => c,
        };
        push_text(arg.get_or_insert_with(Vec::new), text);
    }
    if quoted {
        return Err("Exec has an unclosed quote".to_owned());
    }
    args.extend(arg);
    Ok(args)
}

/// The part `%` and the character after it make: `%%` is a literal `%`.
fn field(code: Option<char>) -> Result<Part, String> {
    match code {
        Some('%') => Ok(Part::Text("%".to_owned())),
        Some(code) => Ok(Part::Code(code)),
        None => Err("Exec ends with %".to_owned()),
    }
}

fn push_text(arg: &mut Vec<Part>, c: char) {
    if let Some(Part::Text(text)) = arg.last_mut() {
        text.push(c);
    } else {
        arg.push(Part::Text(c.to_string()));
    }
}

/// What the field codes `%c`, `%i` and `%k` expand to.
#[derive(Debug, Clone, Copy)]
struct Fields<'a> {
    name: &'a str,
    icon: &'a str,
    location: &'a str,
}

/// The file codes `%f`, `%F`, `%u` and `%U`, which expand to nothing since `launch` passes
/// no files, and the deprecated codes, which the spec says to remove.
const REMOVED: &[char] = &['f', 'F', 'u', 'U', 'd', 'D', 'n', 'N', 'v', 'm'];

/// The arguments with their field codes expanded. An argument that is only a removed code
/// goes; `%i` alone is `--icon <Icon>`, or nothing without an icon; `%c` is the name and
/// `%k` the file's location. `%F`, `%U` and `%i` may only stand alone, and a code the spec
/// doesn't define makes the line invalid.
fn expand(args: Vec<Vec<Part>>, fields: &Fields<'_>) -> Result<Vec<String>, String> {
    let mut expanded = Vec::with_capacity(args.len());
    for arg in args {
        match arg.as_slice() {
            [Part::Code(code)] if REMOVED.contains(code) => continue,
            [Part::Code('i')] => {
                if !fields.icon.is_empty() {
                    expanded.extend(["--icon".to_owned(), fields.icon.to_owned()]);
                }
                continue;
            }
            _ => {}
        }
        let mut text = String::new();
        for part in arg {
            match part {
                Part::Text(piece) => text.push_str(&piece),
                Part::Code(code @ ('F' | 'U' | 'i')) => {
                    return Err(format!(
                        "Exec has %{code} inside an argument, where it may only stand alone"
                    ));
                }
                Part::Code(code) if REMOVED.contains(&code) => {}
                Part::Code('c') => text.push_str(fields.name),
                Part::Code('k') => text.push_str(fields.location),
                Part::Code(code) => {
                    return Err(format!(
                        "Exec has %{code}, which isn't a field code the spec defines"
                    ));
                }
            }
        }
        expanded.push(text);
    }
    Ok(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOPS: &[&str] = &["niri"];

    fn niri() -> Vec<String> {
        DESKTOPS.iter().map(|&desktop| desktop.to_owned()).collect()
    }

    fn app(exec: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName=App\nIcon=app-icon\nExec={exec}\n")
    }

    /// The argv `exec` gives, or why it gives none.
    fn argv(exec: &str) -> Result<Vec<String>, String> {
        let entry = parse(&app(exec))?;
        startable(&entry, &niri(), "/apps/app.desktop").map(|start| start.argv)
    }

    fn strings(argv: &[&str]) -> Vec<String> {
        argv.iter().map(|&arg| arg.to_owned()).collect()
    }

    #[test]
    fn exec_quoting_and_escapes_give_the_argv_a_shell_would() {
        let cases: &[(&str, &[&str])] = &[
            ("firefox %u", &["firefox"]),
            ("nautilus --new-window %U", &["nautilus", "--new-window"]),
            (
                r#""/opt/My App/app" --flag"#,
                &["/opt/My App/app", "--flag"],
            ),
            // `\\` is the string escape for one backslash, which then escapes the space.
            (r"/opt/My\\ App/app", &["/opt/My App/app"]),
            // `\s` is a plain space, which separates arguments unless quoted.
            (r"app\sone", &["app", "one"]),
            (r#"app "\sone""#, &["app", " one"]),
            (r#"app "say \\"hi\\"""#, &["app", r#"say "hi""#]),
            // Four backslashes in the file are one literal backslash in a quoted argument.
            (r#"app "a\\\\b""#, &["app", r"a\b"]),
            (r#"app "\\$HOME""#, &["app", "$HOME"]),
            ("app 100%% --file=%f", &["app", "100%", "--file="]),
            (
                "app %i --title=%c %k",
                &[
                    "app",
                    "--icon",
                    "app-icon",
                    "--title=App",
                    "/apps/app.desktop",
                ],
            ),
            ("app %d %D %n %N %v %m", &["app"]),
            (r#"app """#, &["app", ""]),
            ("  app\t  --a  ", &["app", "--a"]),
            (
                "/usr/bin/flatpak run --branch=stable --command=firefox --file-forwarding org.mozilla.firefox @@u %u @@",
                &[
                    "/usr/bin/flatpak",
                    "run",
                    "--branch=stable",
                    "--command=firefox",
                    "--file-forwarding",
                    "org.mozilla.firefox",
                    "@@u",
                    "@@",
                ],
            ),
        ];
        for (exec, expected) in cases {
            assert_eq!(argv(exec), Ok(strings(expected)), "{exec}");
        }
    }

    #[test]
    fn an_exec_line_written_for_a_shell_or_against_the_spec_is_refused() {
        for exec in [
            "app | tee log",
            "app > log",
            "app; rm x",
            "~/bin/app",
            "app $HOME",
            "app 'quoted'",
            r#"app "$HOME""#,
            r#"app "unclosed"#,
            r"app \\",
            "app %",
            "app %x",
            "app --files=%F",
            "app --icon=%i",
            "%U",
            "",
        ] {
            assert!(argv(exec).is_err(), "{exec:?} gave {:?}", argv(exec));
        }
    }

    #[test]
    fn only_the_desktop_entry_group_counts_and_localized_keys_are_skipped() {
        let text = "# a comment\n\n[Desktop Entry]\nName=Files\nName[fr]=Fichiers\nType = Application\nExec=nautilus\n\n[Desktop Action new]\nExec=nautilus --new-window\nName=New\n";
        let entry = parse(text).unwrap();
        assert_eq!(entry.name.as_deref(), Some("Files"));
        assert_eq!(entry.exec.as_deref(), Some("nautilus"));
        assert_eq!(entry.kind.as_deref(), Some("Application"));
        for malformed in [
            "Name=x\n[Desktop Entry]\nType=Application\n",
            "[Other]\nType=Application\n",
            "[Desktop Entry]\nType=Application\nnot a key\n",
            "[Desktop Entry]\nExec=a\nExec=b\n",
            "[Desktop Entry]\nHidden=yes\n",
            "no group at all",
            "",
        ] {
            assert!(parse(malformed).is_err(), "{malformed:?}");
        }
    }

    fn refusal(extra: &str) -> String {
        let entry = parse(&format!("{}{extra}", app("app"))).unwrap();
        startable(&entry, &niri(), "/apps/app.desktop").unwrap_err()
    }

    #[test]
    fn hidden_undisplayed_terminal_and_non_application_entries_are_refused() {
        assert!(refusal("Hidden=true\n").contains("Hidden=true"));
        assert!(refusal("NoDisplay=true\n").contains("NoDisplay=true"));
        assert!(refusal("Terminal=true\n").contains("Terminal=true"));
        let link = parse("[Desktop Entry]\nType=Link\nURL=https://example.org\n").unwrap();
        assert!(startable(&link, &niri(), "").unwrap_err().contains("Link"));
        let dbus = parse("[Desktop Entry]\nType=Application\nDBusActivatable=true\n").unwrap();
        assert!(
            startable(&dbus, &niri(), "")
                .unwrap_err()
                .contains("DBusActivatable")
        );
        // With an Exec line, a D-Bus activatable entry starts it, as other launchers do.
        let both = parse(&format!("{}DBusActivatable=true\n", app("app"))).unwrap();
        assert_eq!(startable(&both, &niri(), "").unwrap().argv, ["app"]);
    }

    #[test]
    fn the_first_current_desktop_either_list_names_decides() {
        let shown = |extra: &str, desktops: &[&str]| {
            let entry = parse(&format!("{}{extra}", app("app"))).unwrap();
            startable(&entry, &strings(desktops), "").is_ok()
        };
        assert!(shown("", &["niri"]));
        assert!(shown("OnlyShowIn=GNOME;niri;\n", &["niri"]));
        assert!(!shown("OnlyShowIn=GNOME;KDE;\n", &["niri"]));
        assert!(!shown("NotShowIn=niri;\n", &["niri"]));
        assert!(shown("NotShowIn=GNOME;\n", &["niri"]));
        assert!(shown(
            "OnlyShowIn=niri;\nNotShowIn=GNOME;\n",
            &["niri", "GNOME"]
        ));
        assert!(!shown(
            "OnlyShowIn=niri;\nNotShowIn=GNOME;\n",
            &["GNOME", "niri"]
        ));
        assert!(!shown("OnlyShowIn=\n", &["niri"]));
    }

    #[test]
    fn try_exec_and_the_window_class_come_through() {
        let entry = parse(&format!(
            "{}TryExec=/usr/bin/app\nStartupWMClass=App\n",
            app("app")
        ))
        .unwrap();
        let start = startable(&entry, &niri(), "").unwrap();
        assert_eq!(start.try_exec.as_deref(), Some("/usr/bin/app"));
        assert_eq!(start.wm_class.as_deref(), Some("App"));
    }

    #[test]
    fn lists_split_at_unescaped_semicolons() {
        assert_eq!(list(r"a;b\;c;d"), ["a", "b;c", "d"]);
        assert_eq!(list("a;"), ["a"]);
        assert_eq!(list(""), Vec::<String>::new());
    }
}
