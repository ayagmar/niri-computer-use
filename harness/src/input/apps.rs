//! Native text and paste in a Qt Quick text field and in kitty, read back from each: ASCII,
//! the C10 corpus, whose `é`, `ß` and `→` need the extended keymap, `PARTS`, which needs
//! several extended keymaps in turn, with spare keys reused, and a paste of four
//! thousand characters, after which the clipboard owner's types offer the same bytes again.
//! The GTK 4 entry is `text_entry`'s. kitty runs `cat` on a raw terminal, so what reaches
//! the file is what kitty sent the program; a raw terminal reads Return as `\r`.
//!
//! In a headless run it also types `abc` into the Qt field with each backend while Shift
//! is held, then while Caps Lock is on, then while Num Lock is on, on a keyboard niri
//! takes for a physical one, and then presses `a` on that keyboard, after keypad `1` with
//! Num Lock: the call's text must be exact, and the keys show whether the field knows the
//! modifier is still held or the lock still on.

use std::ffi::OsString;
use std::fs;
use std::time::Duration;

use serde_json::{Value, json};

use super::physical::{self, Physical};
use super::text_entry::focused_app;
use super::{SERVER_DEADLINE, WAIT, paste};
use crate::failure::{Context as _, Failure, Result};
use crate::keyboard::{CORPUS, PARTS};
use crate::mcp::{Client, field, structured};
use crate::runner::Process;
use crate::session::Session;

const ASCII: &str = "Hello, world! 0123456789 ~`{}|<>?";
/// Four times `type_text`'s limit.
const REPEATS: usize = 40;
/// Not `kitty`, so it can't be taken for the kitty another check starts.
const KITTY_CLASS: &str = "org.ncu.KittyInput";
const APP_DEADLINE: Duration = Duration::from_secs(60);
/// kitty takes a few seconds to start without a session bus's portal.
const STARTUP: Duration = Duration::from_secs(15);

/// One application under test: the start of its `app_id`, the keys that paste in it, and
/// where its text is read back.
struct Target {
    name: &'static str,
    /// `qml6` is `org.qt-project.qml6` since Qt 6.12 and `org.qt-project.qml` before.
    app_id: &'static str,
    paste_keys: &'static str,
    /// The file the text is read back from, in `TEST_DIR`.
    file: &'static str,
    /// Whether the file grows with each text, as kitty's does, rather than holding the
    /// field's whole text.
    appends: bool,
    /// Whether the app reads the clipboard after the paste key. A Qt Quick text field
    /// reads the text whenever the clipboard changes, to know whether it can paste
    /// (`QQuickTextInput::q_canPasteChanged`), and Qt keeps what it read for the
    /// selection (`QWaylandMimeData`), so its paste reads nothing.
    reads_after_key: bool,
}

const QT: Target = Target {
    name: "Qt Quick text field",
    app_id: "org.qt-project.qml",
    paste_keys: "ctrl+v",
    file: "qt-input-text",
    appends: false,
    reads_after_key: false,
};

const KITTY: Target = Target {
    name: "kitty",
    app_id: KITTY_CLASS,
    paste_keys: "ctrl+shift+v",
    file: "kitty-input",
    appends: true,
    reads_after_key: true,
};

pub(super) fn run(session: &mut Session<'_>, wtype: &mut Client, server: &str) -> Result<()> {
    let mut native = Client::start_keyboard(
        session,
        server,
        "native",
        "harness-m7-apps",
        SERVER_DEADLINE,
    )?;
    let qt = start_qt(session)?;
    let app_id = check(session, &mut native, &QT)?;
    held(
        session,
        [("wtype", wtype), ("native", &mut native)],
        &app_id,
    )?;
    qt.stop()?;
    let kitty = start_kitty(session)?;
    check(session, &mut native, &KITTY)?;
    kitty.stop()?;
    native.stop()
}

fn start_qt(session: &Session<'_>) -> Result<Process> {
    let root = session.test_dir().root();
    let file = root.join("qt-input.qml");
    fs::write(&file, include_str!("../../fixtures/qt-input.qml"))
        .context(format!("write {}", file.display()))?;
    let args: Vec<OsString> = vec![
        "QML_XHR_ALLOW_FILE_WRITE=1".into(),
        "QT_FORCE_STDERR_LOGGING=1".into(),
        "qml6".into(),
        file.into(),
        "--".into(),
        root.into(),
    ];
    session.start("env", &args, session.artifact("qt-input.log"), APP_DEADLINE)
}

fn start_kitty(session: &Session<'_>) -> Result<Process> {
    let out = session.test_dir().root().join(KITTY.file);
    fs::write(&out, "").context(format!("create {}", out.display()))?;
    let script = format!("stty raw -echo && exec cat > '{}'", out.display());
    let args: Vec<OsString> = [
        "--config",
        "NONE",
        "--class",
        KITTY_CLASS,
        "sh",
        "-c",
        &script,
    ]
    .iter()
    .map(OsString::from)
    .collect();
    session.start(
        "kitty",
        &args,
        session.artifact("kitty-input.log"),
        APP_DEADLINE,
    )
}

/// Waits for `app`'s focus, then types and pastes into it and reads each text back, under
/// the lease. Returns the app's `app_id`.
fn check(session: &mut Session<'_>, client: &mut Client, app: &Target) -> Result<String> {
    structured(&client.call(session, "acquire_desktop", json!({}))?)?;
    let app_id = session.wait_until("m7-apps-focus", "the app's focus", STARTUP, |session| {
        let desktop = structured(&client.call(session, "desktop_state", json!({}))?)?;
        Ok(focused_app(&desktop).filter(|id| id.starts_with(app.app_id)))
    })?;
    let mut expected = String::new();
    for text in [ASCII, CORPUS, PARTS] {
        let result = call(session, client, &app_id, json!({"text": text}))?;
        sent(app, &result)?;
        shown(session, app, &mut expected, text)?;
        clear(session, client, app, &app_id, &mut expected)?;
    }
    let (owner, copied) = paste::own(session, app.app_id)?;
    let text = CORPUS.repeat(REPEATS);
    let args = json!({"text": text, "via": "paste", "paste_keys": app.paste_keys});
    let pasted = call(session, client, &app_id, args)?;
    sent(app, &pasted)?;
    shown(session, app, &mut expected, &text)?;
    let outcome = json!({"read": app.reads_after_key, "clipboard": "restored"});
    if field(&pasted, "/paste") != &outcome {
        return Err(Failure::new(format!("M7 paste in {}: {pasted}", app.name)));
    }
    owner.wait()?;
    let after = paste::offered(session)?;
    if after != copied {
        return Err(Failure::new(format!(
            "M7 {}: the clipboard after the paste is {after:?}, not {copied:?}",
            app.name
        )));
    }
    clear(session, client, app, &app_id, &mut expected)?;
    structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    session.log(&format!(
        "M7 native in {}: ASCII, the C10 corpus and a text needing several keymaps typed, and {} characters pasted with {}, read back exactly, with paste {}; the owner's {} types offered again",
        app.name,
        text.chars().count(),
        app.paste_keys,
        outcome,
        copied.len()
    ))?;
    Ok(app_id)
}

#[derive(Debug, Clone, Copy)]
enum Hold {
    Shift,
    CapsLock,
    NumLock,
}

impl Hold {
    const ALL: [Self; 3] = [Self::Shift, Self::CapsLock, Self::NumLock];

    fn start(self, keyboard: &mut Physical) -> Result<()> {
        match self {
            Self::Shift => keyboard.key(physical::LEFT_SHIFT, true),
            Self::CapsLock => keyboard.tap(physical::CAPS_LOCK),
            Self::NumLock => keyboard.tap(physical::NUM_LOCK),
        }
    }

    fn end(self, keyboard: &mut Physical) -> Result<()> {
        match self {
            Self::Shift => keyboard.key(physical::LEFT_SHIFT, false),
            Self::CapsLock => keyboard.tap(physical::CAPS_LOCK),
            Self::NumLock => keyboard.tap(physical::NUM_LOCK),
        }
    }

    /// The keys pressed by hand after the call: keypad `1` with Num Lock, then `a`, whose
    /// arrival says the keys before it were handled.
    const fn by_hand(self) -> &'static [u32] {
        match self {
            Self::Shift | Self::CapsLock => &[physical::A],
            Self::NumLock => &[physical::KEYPAD_1, physical::A],
        }
    }

    /// What the keys pressed by hand type after a call with `backend`. wtype's map isn't
    /// the compositor's, so niri sends the compositor's map with the modifiers and locks
    /// it has along with the next physical key, and the field knows: `A`, and `1` before
    /// the `a`. Native leaves the compositor's map active, so niri sends no map, and the
    /// field keeps the call's zero modifiers and locks until a modifier or lock changes:
    /// `a`, and keypad `1` as End, which types nothing.
    fn typed(self, backend: &str) -> &'static str {
        match (self, backend == "native") {
            (Self::Shift | Self::CapsLock, false) => "A",
            (Self::NumLock, false) => "1a",
            (_, true) => "a",
        }
    }
}

/// Each backend's call with Shift held, with Caps Lock on and with Num Lock on, by hand,
/// in the Qt field `app_id`; see `Hold::typed`.
fn held(session: &mut Session<'_>, clients: [(&str, &mut Client); 2], app_id: &str) -> Result<()> {
    let Some(mut keyboard) = Physical::connect(session)? else {
        return session
            .log("M7 held modifiers: skipped (a visible run has no keyboard to hold them on)");
    };
    for (backend, client) in clients {
        structured(&client.call(session, "acquire_desktop", json!({}))?)?;
        for hold in Hold::ALL {
            let expected = hold.typed(backend);
            let typed = after_call(session, client, &mut keyboard, app_id, hold)?;
            if typed != expected {
                return Err(Failure::new(format!(
                    "M7 {backend} with {hold:?}: the keys by hand after the call typed {typed:?}, not {expected:?}"
                )));
            }
        }
        structured(&client.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    }
    session.log(&format!(
        "M7 held modifiers in the Qt field: `abc` typed exactly with Shift held, with Caps Lock on and with Num Lock on by both backends; `a` by hand afterwards typed `{}` after wtype and `{}` after native; with Num Lock on, keypad `1` then `a` typed `{}` after wtype and `{}` after native",
        Hold::Shift.typed("wtype"),
        Hold::Shift.typed("native"),
        Hold::NumLock.typed("wtype"),
        Hold::NumLock.typed("native"),
    ))
}

/// Types `abc` with `hold`, checks the field shows exactly that, then presses the keys
/// by hand and returns what they typed, once an `a` or `A` ends it, and empties the field.
fn after_call(
    session: &mut Session<'_>,
    client: &mut Client,
    keyboard: &mut Physical,
    app_id: &str,
    hold: Hold,
) -> Result<String> {
    hold.start(keyboard)?;
    let result = call(session, client, app_id, json!({"text": "abc"}))?;
    sent(&QT, &result)?;
    let mut expected = String::new();
    shown(session, &QT, &mut expected, "abc")?;
    for &key in hold.by_hand() {
        keyboard.tap(key)?;
    }
    let path = session.test_dir().root().join(QT.file);
    let typed = session.wait_until("m7-held-key", "the keys pressed by hand", WAIT, |_| {
        let seen = fs::read_to_string(&path).context(format!("read {}", path.display()))?;
        let added = seen.strip_prefix("abc").unwrap_or_default();
        Ok(added.ends_with(['a', 'A']).then(|| added.to_owned()))
    })?;
    hold.end(keyboard)?;
    expected.push_str(&typed);
    clear(session, client, &QT, app_id, &mut expected)?;
    Ok(typed)
}

fn call(
    session: &mut Session<'_>,
    client: &mut Client,
    app_id: &str,
    mut args: Value,
) -> Result<Value> {
    if let Some(args) = args.as_object_mut() {
        args.insert("expect".into(), json!({"app_id": app_id}));
    }
    structured(&client.call(session, "type_text", args)?)
}

fn sent(app: &Target, result: &Value) -> Result<()> {
    if field(result, "/observed") != "sent" || field(result, "/focus") != "matched" {
        return Err(Failure::new(format!("M7 {}: {result}", app.name)));
    }
    Ok(())
}

/// Waits until the app shows `text`, after what it showed before for kitty, whose file
/// keeps everything.
fn shown(session: &mut Session<'_>, app: &Target, expected: &mut String, text: &str) -> Result<()> {
    if app.appends {
        expected.push_str(text);
    } else {
        text.clone_into(expected);
    }
    let path = session.test_dir().root().join(app.file);
    session.wait_until("m7-apps-text", "the app's text", WAIT, |_| {
        let seen = fs::read_to_string(&path).context(format!("read {}", path.display()))?;
        Ok((seen == *expected).then_some(()))
    })
}

/// Empties the Qt field with `ctrl+a`, then `BackSpace`; kitty's file only grows.
fn clear(
    session: &mut Session<'_>,
    client: &mut Client,
    app: &Target,
    app_id: &str,
    expected: &mut String,
) -> Result<()> {
    if app.appends {
        return Ok(());
    }
    let args = json!({"keys": ["ctrl+a", "BackSpace"], "expect": {"app_id": app_id}});
    let result = structured(&client.call(session, "key", args)?)?;
    sent(app, &result)?;
    shown(session, app, expected, "")
}
