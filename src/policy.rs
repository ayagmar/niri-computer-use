//! The policy file, `$XDG_CONFIG_HOME/niri-computer-use/policy.toml`, and the decisions it
//! feeds: launch presets, the app deny list, whether a server may take the lease, and
//! which output setups the pointer tools may run on.
//! Everything here is pure; the caller reads the file. The preset rules catch common
//! mistakes. They are a guardrail, not a boundary: a wrapper script or a symlink with
//! another name gets past any list.

use std::collections::BTreeSet;
use std::path::Path;

use niri_ipc::{Output, Transform};
use serde::{Deserialize, Serialize};

use crate::control::LockState;
use crate::error::{ErrorName, ToolError};
use crate::niri::events::StreamState;
use crate::niri::version::Compat;

/// Programs that run whatever command they are given, so no preset may start them.
const COMMAND_RUNNERS: [&str; 35] = [
    "sh",
    "bash",
    "zsh",
    "fish",
    "dash",
    "ksh",
    "mksh",
    "tcsh",
    "csh",
    "nu",
    "elvish",
    "xonsh",
    "env",
    "sudo",
    "doas",
    "pkexec",
    "su",
    "run0",
    "setsid",
    "nohup",
    "systemd-run",
    "timeout",
    "xargs",
    "nice",
    "python",
    "python3",
    "perl",
    "ruby",
    "node",
    "lua",
    "busybox",
    "toybox",
    "uwsm",
    "distrobox",
    "toolbox",
];

/// Terminals run their trailing arguments as a command, so a preset may start one only
/// without arguments.
const TERMINALS: [&str; 17] = [
    "foot",
    "footclient",
    "alacritty",
    "kitty",
    "wezterm",
    "ghostty",
    "gnome-terminal",
    "kgx",
    "konsole",
    "xterm",
    "uxterm",
    "urxvt",
    "st",
    "terminator",
    "tilix",
    "xfce4-terminal",
    "rio",
];

/// The parsed file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Policy {
    /// Input tools refuse while the focused window has one of these `app_id`s.
    #[serde(default)]
    pub(crate) deny_input_app_ids: Vec<String>,
    #[serde(default, rename = "preset")]
    pub(crate) presets: Vec<Preset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Preset {
    /// What `launch` takes.
    pub(crate) name: String,
    /// Fixed; no shells, interpreters or terminals with arguments.
    pub(crate) argv: Vec<String>,
    /// For observing the launched window and for `reuse`.
    pub(crate) app_id: String,
}

/// The file's state, as loaded once at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Loaded {
    /// No file: no presets and an empty deny list, which is valid.
    Missing,
    Valid(Policy),
    /// Action tools refuse with `read_only` until the file is fixed and the server
    /// restarted.
    Invalid(String),
}

/// What `status` reports about the policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct PolicyStatus {
    state: &'static str,
    presets: usize,
    /// What `launch` takes.
    preset_names: Vec<String>,
    denied_app_ids: usize,
    error: Option<String>,
}

impl Loaded {
    /// Checks what reading the file at `path` gave; `None` when there is no config
    /// directory to look in, which counts as invalid because a file might exist.
    pub(crate) fn from_read(read: Option<(&Path, std::io::Result<String>)>) -> Self {
        let Some((path, read)) = read else {
            return Self::Invalid("neither XDG_CONFIG_HOME nor HOME is set".to_owned());
        };
        match read {
            Ok(text) => parse(&text).map_or_else(
                |error| Self::Invalid(format!("{}: {error}", path.display())),
                Self::Valid,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::Missing,
            Err(error) => Self::Invalid(format!("read {}: {error}", path.display())),
        }
    }

    /// The preset `launch` names, or `unknown_preset`.
    pub(crate) fn preset(&self, name: &str) -> Result<&Preset, ToolError> {
        let presets = match self {
            Self::Valid(policy) => policy.presets.as_slice(),
            Self::Missing | Self::Invalid(_) => &[],
        };
        presets
            .iter()
            .find(|preset| preset.name == name)
            .ok_or_else(|| {
                let names: Vec<&str> = presets.iter().map(|preset| preset.name.as_str()).collect();
                ToolError::new(
                    ErrorName::UnknownPreset,
                    format!("no preset named {name:?}; the policy file has {names:?}"),
                )
            })
    }

    pub(crate) fn status(&self) -> PolicyStatus {
        let (state, policy, error) = match self {
            Self::Missing => ("missing", None, None),
            Self::Valid(policy) => ("loaded", Some(policy), None),
            Self::Invalid(error) => ("invalid", None, Some(error.clone())),
        };
        PolicyStatus {
            state,
            presets: policy.map_or(0, |policy| policy.presets.len()),
            preset_names: policy.map_or_else(Vec::new, |policy| {
                policy
                    .presets
                    .iter()
                    .map(|preset| preset.name.clone())
                    .collect()
            }),
            denied_app_ids: policy.map_or(0, |policy| policy.deny_input_app_ids.len()),
            error,
        }
    }
}

/// Parses the file and checks every preset.
pub(crate) fn parse(text: &str) -> Result<Policy, String> {
    let policy: Policy = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut names = BTreeSet::new();
    for preset in &policy.presets {
        check(preset)?;
        if !names.insert(preset.name.as_str()) {
            return Err(format!("two presets are named {:?}", preset.name));
        }
    }
    Ok(policy)
}

fn check(preset: &Preset) -> Result<(), String> {
    let name = &preset.name;
    if name.is_empty() || preset.app_id.is_empty() {
        return Err("a preset needs a non-empty name and app_id".to_owned());
    }
    let Some(program) = preset.argv.first().filter(|program| !program.is_empty()) else {
        return Err(format!("preset {name:?} has an empty argv"));
    };
    let base = Path::new(program)
        .file_name()
        .map_or_else(String::new, |base| base.to_string_lossy().into_owned());
    // `python3.13` is `python3`, and `perl5.40` is `perl`.
    let unversioned = base.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    if COMMAND_RUNNERS.contains(&base.as_str()) || COMMAND_RUNNERS.contains(&unversioned) {
        return Err(format!(
            "preset {name:?} starts {base}, which runs any command it is given"
        ));
    }
    if base == "flatpak" && preset.argv.iter().any(|arg| arg.starts_with("--command")) {
        return Err(format!(
            "preset {name:?} starts flatpak with --command, which runs any command"
        ));
    }
    if TERMINALS.contains(&base.as_str()) && preset.argv.len() > 1 {
        return Err(format!(
            "preset {name:?} starts the terminal {base} with arguments, which it can run as a command"
        ));
    }
    Ok(())
}

/// What the control decision looks at, gathered when `acquire_desktop` or an action tool
/// is called.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Facts<'a> {
    /// The version rule's answer; none when niri's version couldn't be read.
    pub(crate) compat: Option<Compat>,
    /// Why niri's version couldn't be read.
    pub(crate) niri_error: Option<&'a ToolError>,
    pub(crate) event_stream: Option<StreamState>,
    pub(crate) policy: &'a Loaded,
    pub(crate) lock: LockState,
}

/// Why a server may not take the lease or act now, checked after the stop flag, the
/// input-dirty marker and, for actions, the lease: niri unreachable, then read-only (niri's version, its event schema,
/// or the policy file), then a screen that is locked or whose lock state is unknown. Input
/// only ever goes to a screen known to be unlocked.
pub(crate) fn refuse_control(facts: Facts<'_>) -> Option<ToolError> {
    let read_only = |reason: String| Some(ToolError::new(ErrorName::ReadOnly, reason));
    if let Some(error) = facts.niri_error {
        return Some(error.clone());
    }
    match facts.compat {
        None => {
            return Some(ToolError::new(
                ErrorName::NiriUnavailable,
                "niri's version is unknown",
            ));
        }
        Some(Compat::ReadOnly) => {
            return read_only("this build doesn't support the running niri version".to_owned());
        }
        Some(Compat::Ok | Compat::PatchWarning) => {}
    }
    if facts.event_stream == Some(StreamState::SchemaIncompatible) {
        return read_only("niri sent events this build can't parse".to_owned());
    }
    if let Loaded::Invalid(error) = facts.policy {
        return read_only(format!("the policy file is invalid: {error}"));
    }
    match facts.lock {
        LockState::Unlocked => None,
        LockState::Locked => Some(ToolError::new(
            ErrorName::ScreenLocked,
            "the screen is locked",
        )),
        LockState::Unknown => Some(ToolError::new(
            ErrorName::ScreenLocked,
            "the lock state is unknown: neither logind nor Noctalia answered (see status.lock)",
        )),
    }
}

/// `app_denied` when the window with keyboard focus belongs to an app the policy file
/// denies input to (plan §9). Its `app_id` is the client's own claim, and a click can land
/// on another window, so this is a guardrail, not a boundary.
pub(crate) fn refuse_input(policy: &Loaded, focused_app_id: Option<&str>) -> Option<ToolError> {
    let Loaded::Valid(policy) = policy else {
        return None;
    };
    let app_id = focused_app_id?;
    policy
        .deny_input_app_ids
        .iter()
        .any(|denied| denied == app_id)
        .then(|| {
            ToolError::new(
                ErrorName::AppDenied,
                format!("the focused window's app_id {app_id:?} is on the policy's deny list"),
            )
        })
}

/// Whether the pointer tools may run on these outputs (plan §8): exactly one enabled
/// output, either a monitor with transform `Normal` or nested niri's `winit` window, which
/// niri always shows `Flipped180`. Those are the setups live tests cover; anything else,
/// including a monitor really rotated to `Flipped180`, is `untested_output_config`.
pub(crate) fn pointer_support<'a>(
    outputs: impl IntoIterator<Item = &'a Output>,
) -> Result<(), ToolError> {
    let enabled: Vec<(&str, Transform)> = outputs
        .into_iter()
        .filter_map(|output| Some((output.name.as_str(), output.logical?.transform)))
        .collect();
    match enabled.as_slice() {
        [("winit", Transform::Flipped180)] => Ok(()),
        [(name, Transform::Normal)] if *name != "winit" => Ok(()),
        _ => Err(ToolError::new(
            ErrorName::UntestedOutputConfig,
            format!(
                "the pointer runs only with one enabled output, a monitor at transform Normal or nested niri's winit window; enabled outputs and transforms: {enabled:?}"
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
deny_input_app_ids = ["org.keepassxc.KeePassXC"]

[[preset]]
name = "firefox"
argv = ["firefox"]
app_id = "firefox"

[[preset]]
name = "terminal"
argv = ["/usr/bin/foot"]
app_id = "foot"
"#;

    #[test]
    fn reads_the_plan_example() {
        let policy = parse(EXAMPLE).unwrap();
        assert_eq!(policy.deny_input_app_ids, ["org.keepassxc.KeePassXC"]);
        assert_eq!(policy.presets.len(), 2);
        assert_eq!(policy.presets[1].argv, ["/usr/bin/foot"]);
        assert_eq!(parse("").unwrap(), Policy::default());
        // An app that only looks versioned, and a flatpak app, are fine.
        let fine = "[[preset]]\nname = \"a\"\nargv = [\"gimp-2.10\"]\napp_id = \"gimp\"\n\n[[preset]]\nname = \"b\"\nargv = [\"flatpak\", \"run\", \"org.mozilla.firefox\"]\napp_id = \"firefox\"\n";
        assert_eq!(parse(fine).unwrap().presets.len(), 2);
    }

    #[test]
    fn refuses_presets_that_could_run_any_command() {
        let preset = |argv: &str| {
            parse(&format!(
                "[[preset]]\nname = \"x\"\nargv = {argv}\napp_id = \"x\""
            ))
            .unwrap_err()
        };
        assert!(preset(r#"["bash", "-c", "rm -rf ~"]"#).contains("runs any command"));
        assert!(preset(r#"["/usr/bin/env", "firefox"]"#).contains("starts env"));
        assert!(preset(r#"["python3", "-m", "http.server"]"#).contains("python3"));
        assert!(preset(r#"["foot", "-e", "sh"]"#).contains("terminal foot"));
        assert!(preset(r#"["kitty", "sh"]"#).contains("terminal kitty"));
        assert!(preset(r#"["python3.13", "x.py"]"#).contains("python3.13"));
        assert!(preset(r#"["/usr/bin/perl5.40"]"#).contains("perl5.40"));
        assert!(preset(r#"["busybox", "sh"]"#).contains("busybox"));
        assert!(preset(r#"["uwsm", "app", "--", "firefox"]"#).contains("uwsm"));
        assert!(preset(r#"["flatpak", "run", "--command=sh", "org.x.Y"]"#).contains("--command"));
        assert!(preset("[]").contains("empty argv"));
        assert!(preset(r#"[""]"#).contains("empty argv"));
    }

    #[test]
    fn refuses_unknown_keys_duplicates_and_blank_names() {
        assert!(parse("deny_apps = []").is_err());
        assert!(
            parse("[[preset]]\nname = \"a\"\nargv = [\"a\"]\napp_id = \"a\"\nshell = true")
                .is_err()
        );
        let twice = "[[preset]]\nname = \"a\"\nargv = [\"a\"]\napp_id = \"a\"\n".repeat(2);
        assert_eq!(parse(&twice).unwrap_err(), "two presets are named \"a\"");
        assert!(parse("[[preset]]\nname = \"\"\nargv = [\"a\"]\napp_id = \"a\"").is_err());
    }

    #[test]
    fn a_missing_file_is_valid_and_others_are_reported() {
        use std::io::{Error, ErrorKind};
        let path = Path::new("/c/policy.toml");
        let loaded = Loaded::from_read(Some((path, Err(Error::from(ErrorKind::NotFound)))));
        assert_eq!(loaded, Loaded::Missing);
        assert_eq!(loaded.status().state, "missing");
        let status = Loaded::from_read(Some((path, Ok(EXAMPLE.to_owned())))).status();
        assert_eq!(
            (status.state, status.presets, status.denied_app_ids),
            ("loaded", 2, 1)
        );
        assert_eq!(status.preset_names, ["firefox", "terminal"]);
        let invalid = Loaded::from_read(Some((path, Ok("nonsense".to_owned())))).status();
        assert_eq!(invalid.state, "invalid");
        assert!(invalid.error.unwrap().starts_with("/c/policy.toml: "));
        let unreadable =
            Loaded::from_read(Some((path, Err(Error::from(ErrorKind::PermissionDenied)))));
        assert!(
            matches!(unreadable, Loaded::Invalid(error) if error.starts_with("read /c/policy.toml"))
        );
        assert_eq!(Loaded::from_read(None).status().state, "invalid");
    }

    #[test]
    fn launch_names_a_preset_from_the_file() {
        let loaded = Loaded::Valid(parse(EXAMPLE).unwrap());
        assert_eq!(loaded.preset("terminal").unwrap().app_id, "foot");
        let unknown = loaded.preset("Firefox").unwrap_err();
        assert_eq!(unknown.name, ErrorName::UnknownPreset);
        assert_eq!(
            unknown.detail,
            "no preset named \"Firefox\"; the policy file has [\"firefox\", \"terminal\"]"
        );
        assert_eq!(
            Loaded::Missing.preset("firefox").unwrap_err().detail,
            "no preset named \"firefox\"; the policy file has []"
        );
    }

    #[test]
    fn the_lease_is_refused_in_order() {
        let unreachable = ToolError::new(ErrorName::NiriUnavailable, "gone");
        let invalid = Loaded::Invalid("bad".to_owned());
        let ok = Facts {
            compat: Some(Compat::Ok),
            niri_error: None,
            event_stream: Some(StreamState::Connected),
            policy: &Loaded::Missing,
            lock: LockState::Unlocked,
        };
        let name = |facts| refuse_control(facts).map(|error| error.name);
        assert_eq!(name(ok), None);
        assert_eq!(
            name(Facts {
                compat: Some(Compat::PatchWarning),
                ..ok
            }),
            None
        );
        assert_eq!(
            name(Facts {
                lock: LockState::Unknown,
                ..ok
            }),
            Some(ErrorName::ScreenLocked)
        );
        assert_eq!(
            name(Facts {
                event_stream: Some(StreamState::Disconnected),
                ..ok
            }),
            None
        );
        assert_eq!(
            name(Facts {
                compat: None,
                niri_error: Some(&unreachable),
                lock: LockState::Locked,
                ..ok
            }),
            Some(ErrorName::NiriUnavailable)
        );
        assert_eq!(
            name(Facts {
                compat: Some(Compat::ReadOnly),
                ..ok
            }),
            Some(ErrorName::ReadOnly)
        );
        assert_eq!(
            name(Facts {
                event_stream: Some(StreamState::SchemaIncompatible),
                ..ok
            }),
            Some(ErrorName::ReadOnly)
        );
        assert_eq!(
            name(Facts {
                policy: &invalid,
                lock: LockState::Locked,
                ..ok
            }),
            Some(ErrorName::ReadOnly)
        );
        assert_eq!(
            name(Facts {
                lock: LockState::Locked,
                ..ok
            }),
            Some(ErrorName::ScreenLocked)
        );
    }

    #[test]
    fn input_to_a_denied_app_is_refused() {
        let policy = Loaded::Valid(parse(EXAMPLE).unwrap());
        let refused = refuse_input(&policy, Some("org.keepassxc.KeePassXC")).unwrap();
        assert_eq!(refused.name, ErrorName::AppDenied);
        assert!(
            refused.detail.contains("org.keepassxc.KeePassXC"),
            "{}",
            refused.detail
        );
        assert_eq!(refuse_input(&policy, Some("firefox")), None);
        assert_eq!(refuse_input(&policy, None), None);
        assert_eq!(
            refuse_input(&Loaded::Missing, Some("org.keepassxc.KeePassXC")),
            None
        );
    }

    fn output(name: &str, transform: Option<Transform>) -> Output {
        serde_json::from_value(serde_json::json!({
            "name": name, "make": "", "model": "", "serial": null, "physical_size": null,
            "modes": [], "current_mode": null, "is_custom_mode": false,
            "vrr_supported": false, "vrr_enabled": false,
            "logical": transform.map(|transform| serde_json::json!({
                "x": 0, "y": 0, "width": 960, "height": 720, "scale": 1.0,
                "transform": transform
            }))
        }))
        .unwrap()
    }

    #[test]
    fn the_pointer_runs_on_one_tested_output() {
        let supported = |outputs: &[Output]| pointer_support(outputs).map_err(|error| error.name);
        let monitor = output("DP-1", Some(Transform::Normal));
        let nested = output("winit", Some(Transform::Flipped180));
        let off = output("HDMI-A-1", None);
        assert_eq!(supported(std::slice::from_ref(&monitor)), Ok(()));
        assert_eq!(supported(std::slice::from_ref(&nested)), Ok(()));
        // A disabled output doesn't count.
        assert_eq!(supported(&[monitor.clone(), off]), Ok(()));
        let untested = Err(ErrorName::UntestedOutputConfig);
        assert_eq!(supported(&[]), untested);
        assert_eq!(supported(&[monitor, nested]), untested);
        assert_eq!(
            supported(&[output("DP-1", Some(Transform::Flipped180))]),
            untested
        );
        assert_eq!(supported(&[output("DP-1", Some(Transform::_90))]), untested);
        assert_eq!(
            supported(&[output("winit", Some(Transform::Normal))]),
            untested
        );
        let error = pointer_support(&[output("eDP-1", Some(Transform::_270))]).unwrap_err();
        assert!(
            error.detail.ends_with(r#"[("eDP-1", _270)]"#),
            "{}",
            error.detail
        );
    }
}
