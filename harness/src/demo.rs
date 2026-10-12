//! Demo recordings for the README and the site (`harness run --demo <scene>`). The nested
//! niri runs the demo config (`config::demo_niri`), with Noctalia, a wallpaper, kitty
//! showing a source file in Neovim as the user's window, and the chat fixture. One server
//! is driven through a scripted scene (`demo/scenes.rs`): the calls come from the script,
//! and every result is the server's. A `grim` loop records the output meanwhile, one JPEG
//! per frame named by its capture time, and `timeline.jsonl` records each call with its
//! start and end on the same clock. `scripts/demo-encode.py` turns both into the videos.

mod scenes;

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Write as _;
use std::time::{Duration, SystemTime};

use niri_ipc::{Action, Request, Response, Window};
use serde_json::{Value, json};

use crate::a11y::Bus;
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field};
use crate::runner::Process;
use crate::session::{self, Session};

/// Every process the demo starts outlives the scene by this much at most.
const FIXTURE_DEADLINE: Duration = Duration::from_secs(170);
const READY: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(10);
/// How long the desktop sits still in the recording before the first call and after the
/// last.
const STILL: Duration = Duration::from_millis(1500);
/// The source file the user's window shows.
const SOURCE: (&str, &str) = ("settle.rs", include_str!("../../src/settle.rs"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scene {
    /// The README and hero story: the agent switches to the chat, writes a reply through
    /// the chat's accessible elements, and puts focus back on the user's window.
    Errand,
    /// Window layout through `niri_action`: widths, a maximized column, fullscreen.
    Layout,
    /// `type_text` refused with `focus_mismatch`, then sent with `expect` and `submit`.
    Keyboard,
    /// The user's stop between two calls, and the next call refused with `stopped`.
    Stop,
}

impl Scene {
    pub(crate) const ALL: [Self; 4] = [Self::Errand, Self::Layout, Self::Keyboard, Self::Stop];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Errand => "errand",
            Self::Layout => "layout",
            Self::Keyboard => "keyboard",
            Self::Stop => "stop",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scene| scene.name() == name)
    }
}

pub(crate) fn run(session: &mut Session<'_>, server: &str, scene: Scene) -> Result<()> {
    let bus = Bus::start(session)?;
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        FIXTURE_DEADLINE,
    )?;
    let mut client = Client::start(session, server, "harness-demo", FIXTURE_DEADLINE)?;
    mcp::ready(session, &mut client, "demo-ready", READY)?;
    wallpaper(session)?;
    let desktop = Desktop::start(session, scene)?;
    let recorder = record(session)?;
    session::pause(STILL);
    let mut director = Director::new(session, client)?;
    let played = scenes::play(&mut director, scene, &desktop, server);
    session::pause(STILL);
    let capture = recorder.stop().map(drop);
    let Director { client: done, .. } = director;
    played?;
    capture?;
    done.stop()?;
    desktop.stop()?;
    noctalia.stop()?;
    bus.stop()
}

/// A dark graphite gradient, made by `ffmpeg` and set through Noctalia.
fn wallpaper(session: &mut Session<'_>) -> Result<()> {
    let path = session.test_dir().data().join("wallpapers/demo.png");
    let source =
        "gradients=s=1280x720:c0=0x23272b:c1=0x0b0c0d:x0=0:y0=0:x1=1280:y1=720:nb_colors=2";
    let args = ["-v", "error", "-f", "lavfi", "-i", source, "-frames:v", "1"]
        .map(OsString::from)
        .into_iter()
        .chain([path.clone().into_os_string()]);
    session.run("ffmpeg", &args.collect::<Vec<_>>())?;
    let set = ["msg".into(), "wallpaper-set".into(), path.into_os_string()];
    session.wait_until(
        "demo-wallpaper",
        "Noctalia setting the wallpaper",
        WAIT,
        |session| Ok(session.run("noctalia", &set).ok()),
    )?;
    Ok(())
}

/// The demo's windows: the user's kitty, the chat, and for `Layout` a second kitty with
/// notes. The user's window has focus when the scene starts.
#[derive(Debug)]
pub(crate) struct Desktop {
    processes: Vec<Process>,
    pub(crate) users_window: u64,
    pub(crate) chat: u64,
    pub(crate) notes: Option<u64>,
}

impl Desktop {
    fn start(session: &mut Session<'_>, scene: Scene) -> Result<Self> {
        kitty_config(session)?;
        let root = session.test_dir().root().to_owned();
        let source = root.join(SOURCE.0);
        fs::write(&source, SOURCE.1).context(format!("write {}", source.display()))?;
        let mut processes = vec![kitty(session, &source, "kitty-code.log")?];
        let users_window = window(session, "kitty", 1)?;
        let chat_source = root.join("chat.py");
        fs::write(&chat_source, include_str!("../fixtures/chat.py"))
            .context(format!("write {}", chat_source.display()))?;
        let args = ["-I".into(), chat_source.into(), root.clone().into()];
        processes.push(session.start(
            "python3",
            &args,
            session.artifact("chat.log"),
            FIXTURE_DEADLINE,
        )?);
        let chat = window(session, "org.ncu.Chat", 1)?;
        let notes = if scene == Scene::Layout {
            let notes = root.join("notes.md");
            fs::write(&notes, NOTES).context(format!("write {}", notes.display()))?;
            processes.push(kitty(session, &notes, "kitty-notes.log")?);
            Some(window(session, "kitty", 2)?)
        } else {
            None
        };
        focus(session, users_window)?;
        Ok(Self {
            processes,
            users_window,
            chat,
            notes,
        })
    }

    fn stop(self) -> Result<()> {
        for process in self.processes {
            process.stop()?;
        }
        Ok(())
    }
}

const NOTES: &str = "# Release review\n\n- moved to 3pm\n- check the changelog\n- tag after CI\n";

/// kitty's config in the nested `XDG_CONFIG_HOME`: the demo's colours and font, and no
/// update check, which would reach the network.
fn kitty_config(session: &Session<'_>) -> Result<()> {
    let dir = session.test_dir().config().join("kitty");
    fs::create_dir_all(&dir).context(format!("create {}", dir.display()))?;
    let config = "font_family JetBrainsMono Nerd Font Mono
font_size 11.5
background #121416
foreground #d9dcdf
selection_background #33383d
cursor #f2a33a
window_padding_width 10 14
update_check_interval 0
confirm_os_window_close 0
enable_audio_bell no
";
    let path = dir.join("kitty.conf");
    fs::write(&path, config).context(format!("write {}", path.display()))
}

/// kitty running Neovim on `file`, without the user's Neovim config or state.
fn kitty(session: &Session<'_>, file: &std::path::Path, log: &str) -> Result<Process> {
    let args: Vec<OsString> = vec![
        "nvim".into(),
        "--clean".into(),
        "-n".into(),
        "-c".into(),
        "set number cursorline nowrap laststatus=0".into(),
        file.into(),
    ];
    session.start("kitty", &args, session.artifact(log), FIXTURE_DEADLINE)
}

/// The id of the `nth` window with `app_id`, by id, once niri has that many.
fn window(session: &mut Session<'_>, app_id: &str, nth: usize) -> Result<u64> {
    let what = format!("window {nth} of {app_id}");
    session.wait_until("demo-window", &what, WAIT, |session| {
        let mut ids: Vec<u64> = windows(session)?
            .into_iter()
            .filter(|window| window.app_id.as_deref() == Some(app_id))
            .map(|window| window.id)
            .collect();
        ids.sort_unstable();
        Ok(ids.get(nth - 1).copied())
    })
}

fn windows(session: &mut Session<'_>) -> Result<Vec<Window>> {
    let Response::Windows(windows) = session.request(&Request::Windows)? else {
        return Err(Failure::new("niri answered Windows with another response"));
    };
    Ok(windows)
}

fn focus(session: &mut Session<'_>, id: u64) -> Result<()> {
    session.request(&Request::Action(Action::FocusWindow { id }))?;
    session.wait_until("demo-focus", "the user's window focused", WAIT, |session| {
        let Response::FocusedWindow(window) = session.request(&Request::FocusedWindow)? else {
            return Err(Failure::new(
                "niri answered FocusedWindow with another response",
            ));
        };
        Ok(window.filter(|window| window.id == id).map(drop))
    })?;
    // Lets niri's scroll animation end before the recording starts.
    session::pause(Duration::from_secs(1));
    Ok(())
}

/// The `grim` loop: one JPEG per frame in `frames/`, named by the Unix time in
/// nanoseconds just before its capture.
fn record(session: &Session<'_>) -> Result<Process> {
    let frames = session.artifact("frames");
    fs::create_dir(&frames).context(format!("create {}", frames.display()))?;
    // About 30 frames a second: grim alone manages over a hundred, most of them repeats.
    let script =
        "while :; do grim -t jpeg -q 92 \"$1/$(date +%s%N).jpg\" || exit 1; sleep 0.025; done";
    let args = ["-c".into(), script.into(), "sh".into(), frames.into()];
    session.start(
        "sh",
        &args,
        session.artifact("recorder.log"),
        FIXTURE_DEADLINE,
    )
}

/// Makes the scene's calls and writes `timeline.jsonl`.
pub(crate) struct Director<'s, 'a> {
    session: &'s mut Session<'a>,
    client: Client,
    timeline: File,
}

impl<'s, 'a> Director<'s, 'a> {
    fn new(session: &'s mut Session<'a>, client: Client) -> Result<Self> {
        let path = session.artifact("timeline.jsonl");
        let timeline = File::create(&path).context(format!("create {}", path.display()))?;
        Ok(Self {
            session,
            client,
            timeline,
        })
    }

    /// Calls `tool`, which must succeed, and returns its structured content.
    pub(crate) fn ok(&mut self, tool: &str, arguments: &Value, hold: Duration) -> Result<Value> {
        let (error, content) = self.call(tool, arguments, hold)?;
        if error {
            return Err(Failure::new(format!("demo: {tool} failed: {content}")));
        }
        Ok(content)
    }

    /// Calls `tool`, which must be refused with the error `name`.
    pub(crate) fn refused(
        &mut self,
        tool: &str,
        arguments: &Value,
        name: &str,
        hold: Duration,
    ) -> Result<()> {
        let (error, content) = self.call(tool, arguments, hold)?;
        if error && field(&content, "/error") == name {
            Ok(())
        } else {
            Err(Failure::new(format!(
                "demo: expected {tool} refused with {name}; saw {content}"
            )))
        }
    }

    /// Records a line of the story that isn't a call, such as what the agent was asked.
    pub(crate) fn note(&mut self, text: &str) -> Result<()> {
        let at = now()?;
        self.write(&json!({"start_ns": at, "end_ns": at, "note": text}))
    }

    /// Calls `tool`, records the call, and holds still for `hold` so the recording shows
    /// what it did. Returns whether it failed, and its structured content: for a failure,
    /// `{"error", "detail"}`.
    fn call(&mut self, tool: &str, arguments: &Value, hold: Duration) -> Result<(bool, Value)> {
        let start = now()?;
        let result = self.client.call(self.session, tool, arguments.clone())?;
        let end = now()?;
        let error = field(&result, "/isError") == true;
        let content = field(&result, "/structuredContent").clone();
        self.write(&json!({
            "start_ns": start, "end_ns": end, "tool": tool, "arguments": arguments,
            "is_error": error, "result": content,
        }))?;
        session::pause(hold);
        Ok((error, content))
    }

    /// Runs a program in the nested session as the user would, such as the stop command,
    /// and records it.
    pub(crate) fn user(&mut self, program: &str, args: &[&str], hold: Duration) -> Result<()> {
        let start = now()?;
        let words: Vec<OsString> = args.iter().map(OsString::from).collect();
        self.session.run(program, &words)?;
        let end = now()?;
        self.write(&json!({
            "start_ns": start, "end_ns": end, "user": [program, args],
        }))?;
        session::pause(hold);
        Ok(())
    }

    fn write(&mut self, record: &Value) -> Result<()> {
        writeln!(self.timeline, "{record}").context("write timeline.jsonl")
    }
}

/// Unix time in nanoseconds, the clock the frames are named by.
fn now() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .context("read the clock")?
        .as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_is_found_by_its_name() {
        for scene in Scene::ALL {
            assert_eq!(Scene::from_name(scene.name()), Some(scene));
        }
        assert_eq!(Scene::from_name("hero"), None);
    }
}
