//! `status --check`: the readiness report as one line per prerequisite, ✓ or ✗, with the
//! fix for each ✗. It reads the report's own JSON, so the text can't say anything the JSON
//! doesn't.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use super::Status;
use crate::stop_key;

/// The text, and how many blockers it names.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Verdict {
    pub(crate) text: String,
    pub(crate) blockers: usize,
}

/// The verdict on `status`, with the stop key's bind naming `program`.
pub(crate) fn verdict(status: &Status, program: &Path) -> Verdict {
    match serde_json::to_value(status).and_then(serde_json::from_value::<View>) {
        Ok(view) => render(&lines(&view, program)),
        Err(error) => render(&[Line::blocker(
            format!("status: unreadable ({error})"),
            "report this as a bug, with the output of `niri-computer-use status`",
        )]),
    }
}

/// The parts of the report the check reads.
#[derive(Debug, Deserialize)]
struct View {
    niri: NiriView,
    display_error: Option<ErrorView>,
    stop: bool,
    input_dirty: Option<serde_json::Value>,
    stop_key: String,
    stop_key_detail: String,
    lock: LockView,
    outputs: OutputsView,
    accessibility: AccessibilityView,
    policy: PolicyView,
    unrestricted: SettingView,
    confirm_lease: SettingView,
    tools: SettingView,
    shared: SettingView,
    engine: Option<EngineView>,
    binaries: BTreeMap<String, bool>,
}

#[derive(Debug, Deserialize)]
struct NiriView {
    version: Option<String>,
    ipc_crate: String,
    compat: Option<String>,
    event_stream: Option<String>,
    error: Option<ErrorView>,
}

#[derive(Debug, Deserialize)]
struct ErrorView {
    error: String,
    detail: String,
}

#[derive(Debug, Deserialize)]
struct LockView {
    state: String,
    source: String,
    logind_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OutputsView {
    pointer_supported: bool,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AccessibilityView {
    available: bool,
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PolicyView {
    state: String,
    presets: usize,
    error: Option<String>,
}

/// A setting a variable can change, and why the variable was refused.
#[derive(Debug, Deserialize)]
struct SettingView {
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EngineView {
    fallback: Option<String>,
}

/// One prerequisite.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    met: bool,
    /// Whether the server can't do its job until it is met; otherwise one feature is off.
    blocks: bool,
    what: String,
    fix: Option<String>,
}

impl Line {
    fn met(what: impl Into<String>) -> Self {
        Self {
            met: true,
            blocks: false,
            what: what.into(),
            fix: None,
        }
    }

    fn blocker(what: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            met: false,
            blocks: true,
            what: what.into(),
            fix: Some(fix.into()),
        }
    }

    fn limit(what: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            blocks: false,
            ..Self::blocker(what, fix)
        }
    }
}

fn lines(view: &View, program: &Path) -> Vec<Line> {
    let mut lines = vec![niri(&view.niri)];
    let reachable = view.niri.error.is_none();
    if reachable {
        lines.extend(display(view.display_error.as_ref()));
        lines.push(lock(&view.lock));
        lines.push(pointer(&view.outputs));
    }
    lines.push(grim(&view.binaries));
    lines.push(accessibility(&view.accessibility));
    if reachable {
        lines.push(stop_key(view, program));
    }
    lines.extend(flags(view));
    lines.push(policy(&view.policy));
    lines.extend(settings(view));
    if !reachable {
        lines.push(Line::limit(
            "the display, lock state, pointer and stop key weren't checked without niri",
            "fix niri first, then run this again",
        ));
    }
    lines
}

fn niri(niri: &NiriView) -> Line {
    if let Some(error) = &niri.error {
        return Line::blocker(
            format!("niri: not reachable ({}: {})", error.error, error.detail),
            "run this from a shell inside your niri session, or set NIRI_SOCKET to niri's socket",
        );
    }
    let version = niri.version.as_deref().unwrap_or("of an unknown version");
    let supported = release(&niri.ipc_crate);
    if niri.event_stream.as_deref() == Some("schema_incompatible") {
        return Line::blocker(
            format!(
                "niri {version}: sent events this build can't read, so every action is refused"
            ),
            format!("run the niri this build is made for, {supported}"),
        );
    }
    match niri.compat.as_deref() {
        Some("ok") => Line::met(format!("niri {version}: supported")),
        Some("patch_warning") => Line::met(format!(
            "niri {version}: supported, though this build was made for {}",
            niri.ipc_crate
        )),
        _ => Line::blocker(
            format!("niri {version}: not supported by this build, so the tools only look"),
            format!(
                "run niri {supported}, which this build is made for, or a niri-computer-use release made for your niri"
            ),
        ),
    }
}

/// The niri release a `niri-ipc` version is made for: `26.4.0` is niri `26.04`.
fn release(ipc_crate: &str) -> String {
    let mut parts = ipc_crate.split('.');
    match (parts.next(), parts.next()) {
        (Some(major), Some(minor)) => format!("{major}.{minor:0>2}"),
        _ => ipc_crate.to_owned(),
    }
}

fn display(error: Option<&ErrorView>) -> Option<Line> {
    let error = error?;
    Some(Line::blocker(
        format!(
            "Wayland display: {} ({}), so input and screenshots are refused",
            error.detail, error.error
        ),
        "run this from a shell inside your niri session, where WAYLAND_DISPLAY is niri's",
    ))
}

fn lock(lock: &LockView) -> Line {
    match lock.state.as_str() {
        "unknown" => {
            let why = lock
                .logind_error
                .as_deref()
                .map(|error| format!(" ({error})"))
                .unwrap_or_default();
            Line::blocker(
                format!("lock state: unknown{why}, so every action is refused"),
                "start niri with `niri --session` (as `niri-session` does), or run Noctalia",
            )
        }
        state => Line::met(format!("lock state: {state}, read from {}", lock.source)),
    }
}

fn pointer(outputs: &OutputsView) -> Line {
    if outputs.pointer_supported {
        return Line::met("pointer: supported on these outputs");
    }
    Line::limit(
        format!(
            "pointer: {}; the pointer tools refuse",
            outputs.reason.as_deref().unwrap_or("not supported")
        ),
        "the keyboard, element and window tools still work",
    )
}

fn grim(binaries: &BTreeMap<String, bool>) -> Line {
    if binaries.get("grim").copied().unwrap_or(false) {
        return Line::met("grim: found");
    }
    Line::blocker(
        "grim: not found on PATH, so screenshots fail",
        "install grim",
    )
}

fn accessibility(accessibility: &AccessibilityView) -> Line {
    if accessibility.available {
        return Line::met("accessibility bus: found");
    }
    Line::limit(
        format!(
            "accessibility bus: none ({}), so the element tools aren't listed",
            accessibility.reason.as_deref().unwrap_or("no reason given")
        ),
        "install at-spi2-core, then restart your agents",
    )
}

fn stop_key(view: &View, program: &Path) -> Line {
    let detail = &view.stop_key_detail;
    match view.stop_key.as_str() {
        "bound" => Line::met(format!("stop key: {detail}")),
        "not_found" => Line::blocker(
            format!("stop key: not bound ({detail})"),
            format!(
                "add this line inside `binds` in your niri config; the Noctalia plugin's Stop also stops an agent:\n{}",
                stop_key::bind_line(program)
            ),
        ),
        _ => Line::limit(
            format!("stop key: can't tell ({detail})"),
            "make sure a key in your niri config runs `niri-computer-use stop`",
        ),
    }
}

/// The stop flag and the input-dirty marker, while set.
fn flags(view: &View) -> Vec<Line> {
    let mut lines = Vec::new();
    if view.stop {
        lines.push(Line::blocker(
            "stop flag: set, so every action is refused",
            "run `niri-computer-use resume` when agents may act again",
        ));
    }
    if view.input_dirty.is_some() {
        lines.push(Line::blocker(
            "input-dirty marker: set, so every action is refused",
            "run `niri-computer-use recover`",
        ));
    }
    lines
}

fn policy(policy: &PolicyView) -> Line {
    match (policy.state.as_str(), &policy.error) {
        ("invalid", error) => Line::blocker(
            format!(
                "policy file: invalid ({}), so every action is refused",
                error.as_deref().unwrap_or("no reason given")
            ),
            "correct the file, then restart your agents",
        ),
        ("missing", _) => Line::met("policy file: none, so `launch` has no presets"),
        _ => Line::met(format!("policy file: loaded, {} presets", policy.presets)),
    }
}

/// The variables refused, and why a client in shared mode is served standalone.
fn settings(view: &View) -> Vec<Line> {
    let refused = [
        ("unrestricted", &view.unrestricted),
        ("confirm_lease", &view.confirm_lease),
        ("tools", &view.tools),
        ("shared", &view.shared),
    ];
    let mut lines: Vec<Line> = refused
        .into_iter()
        .filter_map(|(name, setting)| {
            let error = setting.error.as_ref()?;
            Some(Line::limit(
                format!("{name}: {error}"),
                "correct the variable in the agent's server settings",
            ))
        })
        .collect();
    if let Some(fallback) = view
        .engine
        .as_ref()
        .and_then(|engine| engine.fallback.as_ref())
    {
        lines.push(Line::limit(
            format!("shared engine: not used ({fallback})"),
            "see `engine.fallback` in `niri-computer-use status`",
        ));
    }
    lines
}

fn render(lines: &[Line]) -> Verdict {
    let mut text = Vec::new();
    for line in lines {
        let mark = if line.met { '✓' } else { '✗' };
        let optional = if line.met || line.blocks {
            ""
        } else {
            " (optional)"
        };
        // An upstream error, such as TOML's, may take several lines.
        let what = line.what.replace('\n', "\n    ");
        text.push(format!("{mark} {what}{optional}"));
        if let Some(fix) = &line.fix {
            text.push(format!("    fix: {}", fix.replace('\n', "\n      ")));
        }
    }
    let blockers = lines.iter().filter(|line| line.blocks).count();
    text.push(String::new());
    text.push(match blockers {
        0 => "Ready.".to_owned(),
        1 => "Not ready: 1 blocker.".to_owned(),
        n => format!("Not ready: {n} blockers."),
    });
    Verdict {
        text: text.join("\n"),
        blockers,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    const PROGRAM: &str = "/home/u/.local/bin/niri-computer-use";

    /// JSON pointers into the report, and the values they get.
    type Changes<'a> = &'a [(&'a str, Value)];
    /// Changes, a line the verdict must have, and text its fix must have.
    type Case<'a> = (Changes<'a>, &'a str, &'a str);

    /// The report of a desktop with everything in place, as the `status` subcommand gives it.
    fn ready() -> Value {
        json!({
            "instance": "niri.wayland-1.42.sock",
            "engine": null,
            "display_error": null,
            "niri": {
                "version": "26.04 (8ed0da4)", "ipc_crate": "26.4.0", "compat": "ok",
                "event_stream": null, "error": null
            },
            "stop": false,
            "stop_key": "bound",
            "stop_key_detail": "/home/u/.config/niri/config.kdl binds a key to `niri-computer-use stop`",
            "input_dirty": null,
            "lock": {"state": "unlocked", "source": "logind", "session": "2", "logind_error": null},
            "outputs": {"pointer_supported": true, "reason": null},
            "accessibility": {"available": true, "address": "unix:path=/run/user/1000/at-spi/bus", "reason": null},
            "policy": {"state": "loaded", "presets": 2, "error": null},
            "unrestricted": {"enabled": false, "source": null, "error": null},
            "confirm_lease": {"enabled": false, "source": null, "error": null},
            "tools": {"profile": "full", "source": null, "error": null},
            "shared": {"setting": "on", "error": null},
            "binaries": {"grim": true, "notify-send": true}
        })
    }

    /// The fixture with each pointer's value replaced.
    fn with(changes: &[(&str, Value)]) -> Verdict {
        let mut status = ready();
        for (pointer, value) in changes {
            *status.pointer_mut(pointer).unwrap() = value.clone();
        }
        let view: View = serde_json::from_value(status).unwrap();
        render(&lines(&view, Path::new(PROGRAM)))
    }

    #[test]
    fn a_ready_desktop_has_every_line_met() {
        let verdict = with(&[]);
        assert_eq!(
            verdict.text,
            "\
✓ niri 26.04 (8ed0da4): supported
✓ lock state: unlocked, read from logind
✓ pointer: supported on these outputs
✓ grim: found
✓ accessibility bus: found
✓ stop key: /home/u/.config/niri/config.kdl binds a key to `niri-computer-use stop`
✓ policy file: loaded, 2 presets

Ready."
        );
        assert_eq!(verdict.blockers, 0);
    }

    #[test]
    fn without_niri_it_says_so_and_skips_what_needs_niri() {
        let verdict = with(&[(
            "/niri/error",
            json!({"error": "niri_unavailable", "detail": "connect to /run/user/1000/niri.sock: Connection refused"}),
        )]);
        assert_eq!(
            verdict.text,
            "\
✗ niri: not reachable (niri_unavailable: connect to /run/user/1000/niri.sock: Connection refused)
    fix: run this from a shell inside your niri session, or set NIRI_SOCKET to niri's socket
✓ grim: found
✓ accessibility bus: found
✓ policy file: loaded, 2 presets
✗ the display, lock state, pointer and stop key weren't checked without niri (optional)
    fix: fix niri first, then run this again

Not ready: 1 blocker."
        );
    }

    #[test]
    fn each_blocker_has_its_line_and_fix() {
        let cases: [Case<'_>; 8] = [
            (
                &[
                    ("/niri/version", json!("25.11")),
                    ("/niri/compat", json!("read_only")),
                ],
                "✗ niri 25.11: not supported by this build, so the tools only look",
                "fix: run niri 26.04, which",
            ),
            (
                &[
                    ("/policy/state", json!("invalid")),
                    (
                        "/policy/error",
                        json!(
                            "policy.toml: TOML parse error at line 1\n  |\nunknown field `preset`"
                        ),
                    ),
                ],
                "✗ policy file: invalid (policy.toml: TOML parse error at line 1\n      |\n    unknown field `preset`), so every action is refused",
                "fix: correct the file, then restart your agents",
            ),
            (
                &[
                    ("/lock/state", json!("unknown")),
                    ("/lock/source", json!("none")),
                    ("/lock/logind_error", json!("niri runs without --session")),
                ],
                "✗ lock state: unknown (niri runs without --session), so every action is refused",
                "`niri --session`",
            ),
            (
                &[
                    ("/stop_key", json!("not_found")),
                    (
                        "/stop_key_detail",
                        json!("no bind runs `niri-computer-use stop` in /c.kdl"),
                    ),
                ],
                "✗ stop key: not bound (no bind runs `niri-computer-use stop` in /c.kdl)",
                "{ spawn \"/home/u/.local/bin/niri-computer-use\" \"stop\"; }",
            ),
            (
                &[("/binaries/grim", json!(false))],
                "✗ grim: not found on PATH, so screenshots fail",
                "fix: install grim",
            ),
            (
                &[("/stop", json!(true))],
                "✗ stop flag: set, so every action is refused",
                "niri-computer-use resume",
            ),
            (
                &[("/input_dirty", json!({"operation": "click"}))],
                "✗ input-dirty marker: set, so every action is refused",
                "niri-computer-use recover",
            ),
            (
                &[(
                    "/display_error",
                    json!({"error": "session_mismatch", "detail": "another process serves the display"}),
                )],
                "✗ Wayland display: another process serves the display (session_mismatch), so input and screenshots are refused",
                "WAYLAND_DISPLAY is niri's",
            ),
        ];
        for (changes, line, fix) in cases {
            let verdict = with(changes);
            assert!(
                verdict.text.contains(&format!("{line}\n")),
                "{line}:\n{}",
                verdict.text
            );
            assert!(verdict.text.contains(fix), "{fix}:\n{}", verdict.text);
            assert_eq!(verdict.blockers, 1, "{}", verdict.text);
            assert!(verdict.text.ends_with("Not ready: 1 blocker."));
        }
    }

    #[test]
    fn a_missing_feature_is_marked_optional_and_blocks_nothing() {
        let cases: [(Changes<'_>, &str); 5] = [
            (
                &[
                    ("/outputs/pointer_supported", json!(false)),
                    ("/outputs/reason", json!("two enabled outputs")),
                ],
                "✗ pointer: two enabled outputs; the pointer tools refuse (optional)",
            ),
            (
                &[
                    ("/accessibility/available", json!(false)),
                    ("/accessibility/reason", json!("no org.a11y.Bus")),
                ],
                "✗ accessibility bus: none (no org.a11y.Bus), so the element tools aren't listed (optional)",
            ),
            (
                &[
                    ("/stop_key", json!("unknown")),
                    ("/stop_key_detail", json!("read /c.kdl: Permission denied")),
                ],
                "✗ stop key: can't tell (read /c.kdl: Permission denied) (optional)",
            ),
            (
                &[(
                    "/unrestricted/error",
                    json!("NIRI_COMPUTER_USE_UNRESTRICTED must be 1; it stays off"),
                )],
                "✗ unrestricted: NIRI_COMPUTER_USE_UNRESTRICTED must be 1; it stays off (optional)",
            ),
            (
                &[(
                    "/engine",
                    json!({"fallback": "the engine refused this build"}),
                )],
                "✗ shared engine: not used (the engine refused this build) (optional)",
            ),
        ];
        for (changes, line) in cases {
            let verdict = with(changes);
            assert!(
                verdict.text.contains(&format!("{line}\n")),
                "{line}:\n{}",
                verdict.text
            );
            assert_eq!(verdict.blockers, 0, "{}", verdict.text);
            assert!(verdict.text.ends_with("Ready."));
        }
    }

    // The check reads the report through its JSON, so a renamed field would fail here
    // rather than read as missing.
    #[tokio::test]
    async fn reads_the_report_the_subcommand_collects() {
        let audit = crate::audit::Audit::new(None);
        let session = crate::test_support::session(1);
        let settings = session.settings();
        let sources = super::super::Sources {
            engine: None,
            event_stream: None,
            config_failed: None,
            audit: &audit,
            lease: crate::control::desk::status_without_desk(&crate::Env::default()),
            policy: &settings.policy,
            unrestricted: &settings.unrestricted,
            confirm_lease: &settings.confirm_lease,
            tools: &settings.tools,
            shared: &settings.shared,
            place: crate::test_support::no_programs(),
            accessibility: &crate::a11y::detect(None).await,
            accessibility_switch: None,
        };
        let status = super::super::collect(&crate::Env::default(), sources).await;
        let verdict = verdict(&status, Path::new(PROGRAM));
        assert!(
            verdict
                .text
                .starts_with("✗ niri: not reachable (niri_unavailable: NIRI_SOCKET is not set)"),
            "{}",
            verdict.text
        );
        assert!(
            verdict.text.contains("✗ grim: not found on PATH"),
            "{}",
            verdict.text
        );
        assert_eq!(verdict.blockers, 2, "{}", verdict.text);
    }
}
