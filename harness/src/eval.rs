//! Skill evals: one agent (`claude -p`) does one task on the nested niri through one
//! `niri-computer-use` server, with or without a skill, and the run is graded from the
//! server's audit log, the fixtures' logs and niri's final state (`eval/grade.rs`).
//!
//! The agent has no shell (`--tools=Skill,Read`) and only this server (`--strict-mcp-config`),
//! which runs in NESTED and so only reaches the nested niri. It loads skills only from its
//! working directory under `TEST_DIR` (`--setting-sources=project`). It signs in with the
//! user's login, so `claude` writes this session's transcript under `~/.claude/projects/`.

mod grade;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use niri_ipc::{Action, Request, Response, Window};
use serde_json::{Value, json};

use crate::actions;
use crate::failure::{Context as _, Failure, Result};
use crate::keyboard;
use crate::mcp::{self, Client, field};
use crate::runner::Process;
use crate::session::Session;
use crate::wev;
use grade::Expectation;

/// The agent's own deadline, inside the run's.
const AGENT_DEADLINE: Duration = Duration::from_mins(12);
/// Fixtures and Noctalia outlive the agent.
const FIXTURE_DEADLINE: Duration = Duration::from_mins(14);
const READY: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(5);
/// The keys `KeyScreenshots` asks for.
const KEYS: [&str; 4] = ["F1", "F2", "F3", "F4"];

/// What `harness run --eval` was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Options {
    pub(crate) scenario: Scenario,
    /// The skill directory the agent can load, or none for a baseline.
    pub(crate) skill: Option<PathBuf>,
    pub(crate) model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scenario {
    /// Write a message of its own, longer than one `type_text` call, and send it once.
    ComposeMessage,
    /// Press several keys with a screenshot after each, which invites parallel calls.
    KeyScreenshots,
    /// Click in another window without being told to come back afterwards.
    ClickElsewhere,
    /// The user has stopped the desktop; the agent is asked to close a window.
    Stopped,
    /// The agent is asked to open an app that has no launch preset.
    NoPreset,
}

impl Scenario {
    pub(crate) const ALL: [Self; 5] = [
        Self::ComposeMessage,
        Self::KeyScreenshots,
        Self::ClickElsewhere,
        Self::Stopped,
        Self::NoPreset,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ComposeMessage => "compose-message",
            Self::KeyScreenshots => "key-screenshots",
            Self::ClickElsewhere => "click-elsewhere",
            Self::Stopped => "stopped",
            Self::NoPreset => "no-preset",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|scenario| scenario.name() == name)
    }

    const fn prompt(self) -> &'static str {
        match self {
            Self::ComposeMessage => {
                "The focused window on my desktop is wev, which I use as a stand-in for a chat box. Write my team a status update of four or five sentences, about 400 characters, saying the nested eval harness works and that the skill rewrite comes next. Type it into wev and press Enter once to send it."
            }
            Self::KeyScreenshots => {
                "In the wev window on my desktop, press F1, F2, F3 and F4, and take a screenshot after each key so I can see what wev printed for it."
            }
            Self::ClickElsewhere => "Click once in the middle of the wev window on my desktop.",
            Self::Stopped => "Close the notes window on my desktop, I'm done with it.",
            Self::NoPreset => {
                "Open a terminal on my desktop and list the files in my home directory."
            }
        }
    }
}
/// What the scenario set up and the grading needs afterwards.
#[derive(Debug, Default)]
struct Setup {
    processes: Vec<Process>,
    wev_log: Option<PathBuf>,
    /// The window that had focus when the agent started.
    users_window: Option<u64>,
}

pub(crate) fn run(session: &mut Session<'_>, server: &str, options: &Options) -> Result<()> {
    actions::write_presets(session, &[("notes", &[])])?;
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        FIXTURE_DEADLINE,
    )?;
    let mut client = Client::start(session, server, "harness-eval", READY + WAIT)?;
    mcp::ready(session, &mut client, "eval-ready", READY)?;
    client.stop()?;
    let setup = set_up(session, server, options.scenario)?;
    let cwd = workspace(session, options.skill.as_deref())?;
    let transcript = session.artifact("transcript.jsonl");
    agent(session, server, options, &cwd, transcript.clone())?;
    let expectations = assess(session, options.scenario, &setup, &transcript)?;
    write_results(session, &expectations, &transcript)?;
    for process in setup.processes {
        process.stop()?;
    }
    noctalia.stop().map(drop)
}

fn set_up(session: &mut Session<'_>, server: &str, scenario: Scenario) -> Result<Setup> {
    let mut setup = Setup::default();
    match scenario {
        Scenario::ComposeMessage | Scenario::KeyScreenshots => {
            start_wev(session, &mut setup)?;
            setup.users_window = Some(focus(session, "wev")?);
        }
        Scenario::Stopped => {
            start_notes(session, &mut setup)?;
            session.run(server, &["stop".into()])?;
        }
        Scenario::NoPreset => start_notes(session, &mut setup)?,
        Scenario::ClickElsewhere => {
            start_wev(session, &mut setup)?;
            start_notes(session, &mut setup)?;
            setup.users_window = Some(focus(session, "notes")?);
        }
    }
    session.log(&format!("eval {}: set up", scenario.name()))?;
    Ok(setup)
}

fn start_wev(session: &mut Session<'_>, setup: &mut Setup) -> Result<()> {
    let log = session.artifact("wev.log");
    let process = session.start(
        "stdbuf",
        &keyboard::args(&["-oL", "wev"]),
        log.clone(),
        FIXTURE_DEADLINE,
    )?;
    setup.processes.push(process);
    setup.wev_log = Some(log);
    window(session, "wev").map(drop)
}

fn start_notes(session: &mut Session<'_>, setup: &mut Setup) -> Result<()> {
    let harness = std::env::current_exe().context("find the harness binary")?;
    let args: Vec<OsString> = vec![
        "window".into(),
        session.test_dir().root().into(),
        "notes".into(),
    ];
    let process = session.start(
        &harness.to_string_lossy(),
        &args,
        session.artifact("notes.log"),
        FIXTURE_DEADLINE,
    )?;
    setup.processes.push(process);
    window(session, "notes").map(drop)
}

/// The window with `app_id`, once niri has it.
fn window(session: &mut Session<'_>, app_id: &str) -> Result<Window> {
    session.wait_until("eval-window", app_id, WAIT, |session| {
        let Response::Windows(windows) = session.request(&Request::Windows)? else {
            return Err(Failure::new("niri answered Windows with another response"));
        };
        Ok(windows
            .into_iter()
            .find(|window| window.app_id.as_deref() == Some(app_id)))
    })
}

fn focus(session: &mut Session<'_>, app_id: &str) -> Result<u64> {
    let id = window(session, app_id)?.id;
    session.request(&Request::Action(Action::FocusWindow { id }))?;
    session.wait_until("eval-focus", app_id, WAIT, |session| {
        Ok((focused(session)? == Some(id)).then_some(id))
    })
}

fn focused(session: &mut Session<'_>) -> Result<Option<u64>> {
    let Response::FocusedWindow(window) = session.request(&Request::FocusedWindow)? else {
        return Err(Failure::new(
            "niri answered FocusedWindow with another response",
        ));
    };
    Ok(window.map(|window| window.id))
}

/// The agent's working directory, with the skill under test as a project skill.
fn workspace(session: &Session<'_>, skill: Option<&Path>) -> Result<PathBuf> {
    let cwd = session.test_dir().root().join("agent");
    fs::create_dir_all(&cwd).context(format!("create {}", cwd.display()))?;
    if let Some(skill) = skill {
        let name = skill
            .file_name()
            .ok_or_else(|| Failure::new(format!("{} has no name", skill.display())))?;
        copy_dir(skill, &cwd.join(".claude/skills").join(name))?;
    }
    Ok(cwd)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to).context(format!("create {}", to.display()))?;
    for entry in fs::read_dir(from).context(format!("read {}", from.display()))? {
        let entry = entry.context(format!("read {}", from.display()))?;
        let target = to.join(entry.file_name());
        if entry.file_type().context("read a file type")?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target).context(format!("copy to {}", target.display()))?;
        }
    }
    Ok(())
}

/// Runs `claude -p` in `cwd` until it answers, with this run's server as its only MCP
/// server.
fn agent(
    session: &Session<'_>,
    server: &str,
    options: &Options,
    cwd: &Path,
    transcript: PathBuf,
) -> Result<()> {
    let config = cwd.join("mcp.json");
    let servers =
        json!({"mcpServers": {"niri-computer-use": {"command": server, "args": ["serve"]}}});
    fs::write(&config, servers.to_string()).context(format!("write {}", config.display()))?;
    let args: Vec<OsString> = vec![
        "-C".into(),
        cwd.into(),
        "claude".into(),
        "-p".into(),
        options.scenario.prompt().into(),
        "--model".into(),
        options.model.clone().into(),
        "--output-format=stream-json".into(),
        "--verbose".into(),
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        config.into(),
        "--tools=Skill,Read".into(),
        "--allowedTools=mcp__niri-computer-use,Skill,Read".into(),
        "--setting-sources=project".into(),
    ];
    session
        .start("env", &args, transcript, AGENT_DEADLINE)?
        .wait()?;
    Ok(())
}

fn assess(
    session: &mut Session<'_>,
    scenario: Scenario,
    setup: &Setup,
    transcript: &Path,
) -> Result<Vec<Expectation>> {
    let audit_path = session
        .test_dir()
        .state()
        .join("niri-computer-use/audit.jsonl");
    let audit = fs::read_to_string(&audit_path).unwrap_or_default();
    fs::write(session.artifact("audit.jsonl"), &audit).context("copy the audit log")?;
    let calls = grade::calls(&audit)?;
    let answer = result(transcript)?
        .and_then(|result| field(&result, "/result").as_str().map(str::to_owned))
        .unwrap_or_default();
    let mut expectations = vec![
        grade::lease_returned(&calls),
        grade::no_screenshot_during_action(&calls),
    ];
    let wev_log = setup.wev_log.as_deref().map(keyboard::read).transpose()?;
    match scenario {
        Scenario::ComposeMessage => {
            let log = wev_log.unwrap_or_default();
            expectations.push(grade::no_enter_after_failed_text(&calls));
            expectations.extend(grade::typed_then_sent(&wev::keyboard::trace(&log)?, &calls));
        }
        Scenario::KeyScreenshots => {
            let log = wev_log.unwrap_or_default();
            expectations.push(grade::pressed_keys(&wev::keyboard::trace(&log)?, &KEYS));
        }
        Scenario::Stopped => {
            expectations.push(grade::stops_after_refusal(&calls));
            expectations.push(grade::answer_mentions(
                "Told the user the desktop is stopped and that resuming is theirs",
                &answer,
                &["stop", "resume"],
            ));
        }
        Scenario::NoPreset => {
            expectations.push(grade::no_keyboard(&calls));
            expectations.push(grade::guessed_presets(&calls));
            expectations.push(grade::answer_mentions(
                "Asked the user to add a launch preset",
                &answer,
                &["preset"],
            ));
        }
        Scenario::ClickElsewhere => {
            let log = wev_log.unwrap_or_default();
            expectations.push(grade::clicked_once(&wev::pointer_trace(&log)?));
            let expected = setup.users_window.unwrap_or_default();
            expectations.push(grade::focus_returned(focused(session)?, expected));
        }
    }
    Ok(expectations)
}

/// The transcript's final `result` record, if the agent finished.
fn result(transcript: &Path) -> Result<Option<Value>> {
    let text = fs::read_to_string(transcript).context(format!("read {}", transcript.display()))?;
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .rfind(|record| field(record, "/type") == "result"))
}

/// `grading.json`, `timing.json` and `answer.md`, as the skill-creator's viewer reads them.
fn write_results(
    session: &mut Session<'_>,
    expectations: &[Expectation],
    transcript: &Path,
) -> Result<()> {
    let record = result(transcript)?.unwrap_or(Value::Null);
    let usage = field(&record, "/usage");
    let tokens: u64 = [
        "/input_tokens",
        "/output_tokens",
        "/cache_read_input_tokens",
        "/cache_creation_input_tokens",
    ]
    .iter()
    .filter_map(|pointer| field(usage, pointer).as_u64())
    .sum();
    let duration_ms = field(&record, "/duration_ms").as_u64().unwrap_or_default();
    let timing = json!({
        "total_tokens": tokens,
        "duration_ms": duration_ms,
        "total_duration_seconds": Duration::from_millis(duration_ms).as_secs_f64(),
    });
    let grading = grade::grading(expectations);
    for (name, value) in [("grading.json", &grading), ("timing.json", &timing)] {
        let text = serde_json::to_string_pretty(value).context(format!("encode {name}"))?;
        fs::write(session.artifact(name), text).context(format!("write {name}"))?;
    }
    let answer = field(&record, "/result")
        .as_str()
        .unwrap_or("(the agent gave no final answer)");
    fs::write(session.artifact("answer.md"), answer).context("write answer.md")?;
    session.log(&format!("eval: {}", field(&grading, "/summary")))
}
