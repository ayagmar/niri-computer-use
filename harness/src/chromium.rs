//! `--chromium`: what text typed and pasted through `niri-computer-use` arrives as in a
//! Chromium text area. A Chromium-based browser, found by `find`, opens
//! `fixtures/chromium.html` in app mode on the nested niri, with a profile, a home and
//! caches under `TEST_DIR`. The page puts the text area's exact value in its title, which
//! niri reports, so nothing here needs accessibility. Text the project says arrives
//! exactly must arrive exactly; text it says is lost is reported as received, so a run
//! shows both the loss and the day a fix lands.

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use niri_ipc::{Request, Response, Window};
use serde_json::{Value, json};

use crate::failure::{Context as _, Failure, Result};
use crate::keyboard::{CORPUS, PARTS};
use crate::mcp::{self, Client, field, structured};
use crate::runner::Process;
use crate::session::Session;

/// A browser to use instead of the first one on `PATH`.
pub(crate) const OVERRIDE: &str = "NCU_CHROMIUM";
/// Looked up on `PATH` in this order.
const NAMES: [&str; 6] = [
    "chromium",
    "chromium-browser",
    "google-chrome-stable",
    "google-chrome",
    "brave",
    "helium-browser",
];
const TITLE: &str = "ncu-chromium:";
/// Within what is left of the run's deadline.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(220);
const SERVER_DEADLINE: Duration = Duration::from_secs(210);
const BROWSER_DEADLINE: Duration = Duration::from_secs(200);
const READY: Duration = Duration::from_secs(20);
/// A fresh profile's first start, with the page loaded.
const STARTUP: Duration = Duration::from_secs(30);
const WAIT: Duration = Duration::from_secs(5);
/// How long the title must stay the same, after the call, to count as what arrived.
const SETTLED: Duration = Duration::from_secs(1);
/// SIGTERM lets the browser end its own helpers before the rest are killed.
const TERM_GRACE: Duration = Duration::from_secs(5);

/// What the project says typed text arrives as in Chromium. Paste always delivers it.
#[derive(Debug, Clone, Copy)]
enum Typed {
    Exact,
    /// Known to arrive otherwise, for this reason.
    Lost(&'static str),
}

#[derive(Debug, Clone, Copy)]
struct Case {
    name: &'static str,
    text: &'static str,
    typed: Typed,
}

const CASES: [Case; 12] = [
    Case {
        name: "ASCII",
        text: "Hello, world! 0123456789 ~`{}|<>?",
        typed: Typed::Exact,
    },
    Case {
        name: "the C10 corpus",
        text: CORPUS,
        typed: Typed::Exact,
    },
    Case {
        name: "a period first, wtype's Escape slot",
        text: ". a. b, c",
        typed: Typed::Exact,
    },
    Case {
        name: "a space first, wtype's Escape slot",
        text: " leading space, then words",
        typed: Typed::Exact,
    },
    Case {
        name: "a period fourteenth, wtype's Backspace slot",
        text: "abcdefghijklm.n",
        typed: Typed::Exact,
    },
    Case {
        name: "a space fourteenth, wtype's Backspace slot",
        text: "abcdefghijklm n",
        typed: Typed::Exact,
    },
    Case {
        name: "three symbols the layout lacks",
        text: "ab é ab ß ab → ab",
        typed: Typed::Exact,
    },
    Case {
        name: "fourteen symbols the layout lacks",
        text: "a αβγδεζηθικλμνξ ok",
        typed: Typed::Exact,
    },
    Case {
        name: "thirty symbols the layout lacks, in parts",
        text: PARTS,
        typed: Typed::Exact,
    },
    Case {
        name: "accented Latin",
        text: "café naïve façade Ærø",
        typed: Typed::Exact,
    },
    Case {
        name: "Cyrillic",
        text: "Привет, мир",
        typed: Typed::Exact,
    },
    Case {
        name: "emoji",
        text: "ok 🤖 🌙 done",
        typed: Typed::Lost(
            "Chromium cuts characters outside the Basic Multilingual Plane to 16 bits",
        ),
    },
];

/// The browser: `NCU_CHROMIUM` when set, which must then be an executable file, or else
/// the first of `NAMES` on `PATH`. `None` when there is neither.
pub(crate) fn find(chosen: Option<OsString>, path: Option<OsString>) -> Result<Option<PathBuf>> {
    if let Some(chosen) = chosen {
        let chosen = PathBuf::from(chosen);
        if !executable(&chosen) {
            return Err(Failure::new(format!(
                "{OVERRIDE}={} is not an executable file",
                chosen.display()
            )));
        }
        return absolute(&chosen).map(Some);
    }
    let dirs: Vec<PathBuf> =
        path.map_or_else(Vec::new, |path| std::env::split_paths(&path).collect());
    NAMES
        .iter()
        .flat_map(|name| dirs.iter().map(move |dir| dir.join(name)))
        .find(|candidate| executable(candidate))
        .map(|found| absolute(&found))
        .transpose()
}

fn executable(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn absolute(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).context(format!("resolve {}", path.display()))
}

pub(crate) fn run(session: &mut Session<'_>, server: &str, browser: &str) -> Result<()> {
    let version = session.run(browser, &["--version".into()])?;
    session.log(&format!(
        "Chromium fixture: {browser}, {}",
        String::from_utf8_lossy(&version.stdout).trim()
    ))?;
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let mut client = Client::start(session, server, "harness-chromium", SERVER_DEADLINE)?;
    mcp::ready(session, &mut client, "chromium-ready", READY)?;
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let process = start(session, browser)?;
    let page = Page::open(session)?;
    let differed = cases(session, &mut client, &page)?;
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    process.terminate(TERM_GRACE)?;
    client.stop()?;
    noctalia.stop()?;
    if differed.is_empty() {
        return session.log("Chromium fixture: every promised text arrived exactly");
    }
    Err(Failure::new(format!(
        "Chromium fixture: promised text arrived otherwise: {}",
        differed.join("; ")
    )))
}

/// The browser in app mode, with everything it keeps under `TEST_DIR` and its log, the
/// page's keydowns among it, in `chromium.log`.
fn start(session: &Session<'_>, browser: &str) -> Result<Process> {
    let root = session.test_dir().root();
    let page = root.join("chromium.html");
    fs::write(&page, include_str!("../fixtures/chromium.html"))
        .context(format!("write {}", page.display()))?;
    let home = root.join("chromium-home");
    fs::create_dir_all(&home).context(format!("create {}", home.display()))?;
    let mut args: Vec<OsString> = vec![prefixed("HOME=", &home), browser.into()];
    args.extend(
        [
            "--ozone-platform=wayland",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-sync",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-extensions",
            "--disable-crash-reporter",
            "--password-store=basic",
            "--enable-logging=stderr",
        ]
        .map(OsString::from),
    );
    args.push(prefixed("--user-data-dir=", &root.join("chromium-profile")));
    args.push(prefixed("--app=file://", &page));
    session.start(
        "env",
        &args,
        session.artifact("chromium.log"),
        BROWSER_DEADLINE,
    )
}

fn prefixed(prefix: &str, path: &Path) -> OsString {
    let mut arg = OsString::from(prefix);
    arg.push(path);
    arg
}

/// The fixture's window in the nested niri.
#[derive(Debug)]
struct Page {
    id: u64,
    app_id: String,
    log: PathBuf,
}

impl Page {
    /// Waits for the page's title, then for niri's focus on its window.
    fn open(session: &mut Session<'_>) -> Result<Self> {
        let window = session.wait_until(
            "chromium-window",
            "the fixture page's window",
            STARTUP,
            |session| {
                Ok(windows(session)?
                    .into_iter()
                    .find(|window| units(window).is_some()))
            },
        )?;
        let app_id = window.app_id.clone().unwrap_or_default();
        session.log(&format!(
            "Chromium fixture: window {} in the nested niri, app_id {app_id:?}, pid {:?}",
            window.id, window.pid
        ))?;
        session.wait_until("chromium-focus", "focus on the page", WAIT, |session| {
            let Response::FocusedWindow(focused) = session.request(&Request::FocusedWindow)? else {
                return Err(Failure::new(
                    "niri answered FocusedWindow with another response",
                ));
            };
            Ok(focused.filter(|focused| focused.id == window.id).map(drop))
        })?;
        Ok(Self {
            id: window.id,
            app_id,
            log: session.artifact("chromium.log"),
        })
    }

    /// The text area's value as UTF-16 code units, from the window's title.
    fn value(&self, session: &mut Session<'_>) -> Result<Vec<u16>> {
        let window = windows(session)?
            .into_iter()
            .find(|window| window.id == self.id)
            .ok_or_else(|| Failure::new("the fixture page's window closed"))?;
        units(&window).ok_or_else(|| {
            Failure::new(format!(
                "the fixture page's title is {:?}",
                window.title.unwrap_or_default()
            ))
        })
    }

    fn log_len(&self) -> Result<usize> {
        Ok(fs::metadata(&self.log)
            .context(format!("read {}", self.log.display()))?
            .len()
            .try_into()
            .unwrap_or(usize::MAX))
    }

    /// The page's keydowns logged since `offset`.
    fn keydowns(&self, offset: usize) -> Result<Vec<(String, String)>> {
        let log = fs::read(&self.log).context(format!("read {}", self.log.display()))?;
        Ok(keydowns(&String::from_utf8_lossy(
            log.get(offset..).unwrap_or_default(),
        )))
    }
}

fn windows(session: &mut Session<'_>) -> Result<Vec<Window>> {
    let Response::Windows(windows) = session.request(&Request::Windows)? else {
        return Err(Failure::new("niri answered Windows with another response"));
    };
    Ok(windows)
}

/// The UTF-16 code units the page's title encodes, four hex digits each.
fn units(window: &Window) -> Option<Vec<u16>> {
    let hex = window.title.as_deref()?.strip_prefix(TITLE)?;
    if hex.len() % 4 != 0 {
        return None;
    }
    (0..hex.len())
        .step_by(4)
        .map(|start| u16::from_str_radix(hex.get(start..start + 4)?, 16).ok())
        .collect()
}

/// Each `ncu-key` console line's DOM `code` and `key`.
fn keydowns(log: &str) -> Vec<(String, String)> {
    log.lines()
        .filter_map(|line| {
            let json = line.split_once("\"ncu-key ")?.1;
            let value = serde_json::Deserializer::from_str(json)
                .into_iter::<Value>()
                .next()?
                .ok()?;
            let text = |name| field(&value, name).as_str().map(str::to_owned);
            Some((text("/code")?, text("/key")?))
        })
        .collect()
}

/// Types and pastes every case, clearing the field after each. Returns the promised
/// cases that arrived otherwise.
fn cases(session: &mut Session<'_>, client: &mut Client, page: &Page) -> Result<Vec<String>> {
    let mut differed = Vec::new();
    for case in CASES {
        let offset = page.log_len()?;
        let typed = arrived(session, client, page, json!({"text": case.text}), case.text)?;
        let typed_line = typed_report(&case, &typed, &page.keydowns(offset)?);
        clear(session, client, page)?;
        let args = json!({"text": case.text, "via": "paste", "paste_keys": "ctrl+v"});
        let pasted = arrived(session, client, page, args, case.text)?;
        let pasted_line = pasted_report(&case, &pasted);
        clear(session, client, page)?;
        for (line, failed) in [typed_line, pasted_line] {
            session.log(&line)?;
            differed.extend(failed);
        }
    }
    Ok(differed)
}

/// Sends `type_text` with `args` and returns what the field holds once it shows `text`,
/// or once it stopped changing for `SETTLED` after the call.
fn arrived(
    session: &mut Session<'_>,
    client: &mut Client,
    page: &Page,
    args: Value,
    text: &str,
) -> Result<Vec<u16>> {
    send(session, client, page, "type_text", args)?;
    let expected: Vec<u16> = text.encode_utf16().collect();
    let mut last: Option<(Vec<u16>, Instant)> = None;
    session.wait_until("chromium-text", "the field settling", WAIT, |session| {
        let value = page.value(session)?;
        if value == expected {
            return Ok(Some(value));
        }
        match &last {
            Some((seen, since)) if *seen == value => {
                Ok((since.elapsed() >= SETTLED).then_some(value))
            }
            _ => {
                last = Some((value, Instant::now()));
                Ok(None)
            }
        }
    })
}

/// `ctrl+a`, then `BackSpace`, empties the field.
fn clear(session: &mut Session<'_>, client: &mut Client, page: &Page) -> Result<()> {
    send(
        session,
        client,
        page,
        "key",
        json!({"keys": ["ctrl+a", "BackSpace"]}),
    )?;
    session.wait_until("chromium-clear", "an empty field", WAIT, |session| {
        Ok(page.value(session)?.is_empty().then_some(()))
    })
}

/// Calls `tool` with focus expected on the page; it must have sent everything.
fn send(
    session: &mut Session<'_>,
    client: &mut Client,
    page: &Page,
    tool: &str,
    mut args: Value,
) -> Result<()> {
    if let Some(args) = args.as_object_mut() {
        args.insert("expect".into(), json!({"app_id": page.app_id}));
    }
    let result = structured(&client.call(session, tool, args)?)?;
    if field(&result, "/observed") != "sent" || field(&result, "/focus") != "matched" {
        return Err(Failure::new(format!("Chromium fixture {tool}: {result}")));
    }
    Ok(())
}

/// The log line for a typed case, with the keydowns whose `key` isn't one ASCII
/// character, and the failure when a promised case arrived otherwise.
fn typed_report(
    case: &Case,
    received: &[u16],
    keys: &[(String, String)],
) -> (String, Option<String>) {
    let exact = received.iter().copied().eq(case.text.encode_utf16());
    let codes: Vec<String> = keys
        .iter()
        .filter(|(_, key)| !(key.len() == 1 && key.is_ascii()))
        .map(|(code, key)| format!("{key:?} {code}"))
        .collect();
    let codes = if codes.is_empty() {
        String::new()
    } else {
        format!("; keydowns {}", codes.join(", "))
    };
    let (outcome, failed) = match (exact, case.typed) {
        (true, Typed::Exact) => ("exact".to_owned(), None),
        (true, Typed::Lost(_)) => (
            "exact, though known to be lost: a fix landed".to_owned(),
            None,
        ),
        (false, Typed::Lost(why)) => (
            format!("known loss ({why}), received {}", shown(received)),
            None,
        ),
        (false, Typed::Exact) => (
            format!("DIFFERS, received {}", shown(received)),
            Some(format!("{} typed as {}", case.name, shown(received))),
        ),
    };
    (
        format!("Chromium type_text, {}: {outcome}{codes}", case.name),
        failed,
    )
}

/// The log line for a pasted case, and the failure when it arrived otherwise.
fn pasted_report(case: &Case, received: &[u16]) -> (String, Option<String>) {
    if received.iter().copied().eq(case.text.encode_utf16()) {
        return (format!("Chromium paste, {}: exact", case.name), None);
    }
    (
        format!(
            "Chromium paste, {}: DIFFERS, received {}",
            case.name,
            shown(received)
        ),
        Some(format!("{} pasted as {}", case.name, shown(received))),
    )
}

/// The text quoted and escaped, then the code point of each character that isn't ASCII,
/// since a character cut to 16 bits can look like a plausible one. A lone surrogate is
/// shown by its code unit.
fn shown(units: &[u16]) -> String {
    let mut text = String::new();
    let mut points = Vec::new();
    for unit in char::decode_utf16(units.iter().copied()) {
        let point = match unit {
            Ok(ch) => {
                text.extend(ch.escape_debug());
                u32::from(ch)
            }
            Err(lone) => {
                text.push(char::REPLACEMENT_CHARACTER);
                u32::from(lone.unpaired_surrogate())
            }
        };
        if point > 0x7f {
            points.push(format!("U+{point:04X}"));
        }
    }
    if points.is_empty() {
        format!("\"{text}\"")
    } else {
        format!("\"{text}\" ({})", points.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titled(title: &str) -> Window {
        serde_json::from_value(json!({
            "id": 1, "title": title, "app_id": "x", "pid": null, "workspace_id": null,
            "is_focused": false, "is_floating": false, "is_urgent": false,
            "layout": {"pos_in_scrolling_layout": null, "tile_size": [1.0, 1.0],
                "window_size": [1, 1], "tile_pos_in_workspace_view": null,
                "window_offset_in_tile": [0.0, 0.0]},
            "focus_timestamp": null
        }))
        .unwrap()
    }

    #[test]
    fn the_title_gives_back_the_exact_value() {
        // Chromium collapses whitespace in titles, so the encoding must keep it.
        let value = "  a\u{a0}b  é🤖";
        let hex: Vec<String> = value
            .encode_utf16()
            .map(|unit| format!("{unit:04x}"))
            .collect();
        let title = format!("{TITLE}{}", hex.concat());
        assert_eq!(
            units(&titled(&title)).unwrap(),
            value.encode_utf16().collect::<Vec<_>>()
        );
        assert_eq!(units(&titled(TITLE)).unwrap(), Vec::<u16>::new());
        for other in ["chromium.html", "ncu-chromium:006", "ncu-chromium:zz00"] {
            assert_eq!(units(&titled(other)), None, "{other}");
        }
    }

    #[test]
    fn a_lone_surrogate_is_shown_not_dropped() {
        assert_eq!(
            shown(&[0x61, 0xd83e, 0xf916]),
            "\"a\u{fffd}\u{f916}\" (U+D83E U+F916)"
        );
        assert_eq!(
            shown(&"a\"b\n".encode_utf16().collect::<Vec<_>>()),
            "\"a\\\"b\\n\""
        );
    }

    #[test]
    fn keydowns_are_read_from_console_lines() {
        let log = concat!(
            "[1:1:1014/120000.000000:INFO:CONSOLE:31] \"ncu-key {\"code\":\"BracketRight\",\"key\":\"}\"}\", source: file:///t/chromium.html (31)\n",
            "[1:1:1014/120000.000000:ERROR:gpu] unrelated\n",
            "[1:1:1014/120000.000000:INFO:CONSOLE(31)] \"ncu-key {\"code\":\"F13\",\"key\":\"γ\"}\", source: file:///t/chromium.html (31)\n",
        );
        assert_eq!(
            keydowns(log),
            [
                ("BracketRight".to_owned(), "}".to_owned()),
                ("F13".to_owned(), "γ".to_owned())
            ]
        );
    }

    #[test]
    fn only_a_promised_case_that_differs_fails() {
        let case = |typed| Case {
            name: "c",
            text: "é",
            typed,
        };
        let lost = Typed::Lost("why");
        let keys = [
            ("IntlRo".to_owned(), "é".to_owned()),
            ("KeyA".to_owned(), "a".to_owned()),
        ];
        let (line, failed) = typed_report(&case(Typed::Exact), &[0xe9], &keys);
        assert_eq!(
            (line.as_str(), failed),
            ("Chromium type_text, c: exact; keydowns \"é\" IntlRo", None)
        );
        assert!(typed_report(&case(Typed::Exact), &[], &[]).1.is_some());
        let (lost_line, lost_failed) = typed_report(&case(lost), &[0x61], &[]);
        assert!(
            lost_line.contains("known loss (why), received \"a\""),
            "{lost_line}"
        );
        assert_eq!(lost_failed, None);
        assert!(
            typed_report(&case(lost), &[0xe9], &[])
                .0
                .contains("a fix landed")
        );
        assert!(pasted_report(&case(lost), &[0x61]).1.is_some());
        assert_eq!(pasted_report(&case(lost), &[0xe9]).1, None);
    }

    #[test]
    fn the_override_wins_and_must_be_executable() {
        let dir = std::env::temp_dir().join(format!("harness-chromium-{}", std::process::id()));
        let (first, second) = (dir.join("first"), dir.join("second"));
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        let script = |path: &Path, mode| {
            fs::write(path, "").unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
        };
        script(&first.join("helium-browser"), 0o755);
        script(&first.join("chromium"), 0o644);
        script(&second.join("chromium"), 0o755);
        let path = std::env::join_paths([&first, &second]).unwrap();
        // An earlier name wins over an earlier directory; a file that isn't executable is skipped.
        assert_eq!(
            find(None, Some(path.clone())).unwrap(),
            Some(second.join("chromium"))
        );
        let chosen = first.join("helium-browser");
        assert_eq!(
            find(Some(chosen.clone().into()), Some(path.clone())).unwrap(),
            Some(chosen)
        );
        assert!(find(Some(first.join("chromium").into()), Some(path)).is_err());
        assert_eq!(find(None, Some(dir.join("none").into())).unwrap(), None);
        assert_eq!(find(None, None).unwrap(), None);
        fs::remove_dir_all(dir).unwrap();
    }
}
