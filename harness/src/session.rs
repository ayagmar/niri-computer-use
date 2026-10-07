//! Test steps in the nested niri. A `Session` exists only after the supervisor has
//! identified the nested niri on its connection. Every process a step starts goes through
//! `run` or `start`, which re-check the nested endpoints first, and every wait goes through
//! `wait_until` or `still_absent`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, Instant};

use niri_ipc::{Action, Request, Response};

use crate::failure::{Failure, Result};
use crate::log::Log;
use crate::nested::Nested;
use crate::niri::Connection;
use crate::runner::{self, ChildEnv, Group, Invocation, Process, Sink};
use crate::test_dir::TestDir;

const STEP_DEADLINE: Duration = Duration::from_secs(10);
/// How often `wait_until` checks its condition.
const POLL: Duration = Duration::from_millis(50);

#[derive(Debug)]
pub(crate) struct Session<'a> {
    test_dir: &'a TestDir,
    artifacts: &'a Path,
    log: Log,
    niri: Connection,
}

impl<'a> Session<'a> {
    /// `niri` must be the connection the nested niri was identified on.
    pub(crate) const fn new(
        test_dir: &'a TestDir,
        artifacts: &'a Path,
        log: Log,
        niri: Connection,
    ) -> Self {
        Self {
            test_dir,
            artifacts,
            log,
            niri,
        }
    }

    pub(crate) fn log(&mut self, text: &str) -> Result<()> {
        self.log.line(text)
    }

    pub(crate) fn artifact(&self, name: &str) -> PathBuf {
        self.artifacts.join(name)
    }

    pub(crate) fn request(&mut self, request: &Request) -> Result<Response> {
        self.niri.request(request)
    }

    /// Sends `Quit` on the identified connection.
    pub(crate) fn quit(mut self) -> Result<()> {
        self.request(&Request::Action(Action::Quit {
            skip_confirmation: true,
        }))
        .map(drop)
    }

    /// Runs a program to completion in NESTED and captures its output.
    pub(crate) fn run(&self, program: &str, args: &[OsString]) -> Result<Output> {
        Nested::from_env(self.test_dir)?;
        runner::run(&nested(program, args, Sink::Capture, STEP_DEADLINE))
    }

    /// Starts a program in NESTED that runs until it is stopped, with its output in `log`.
    /// Stopping it after `deadline` fails.
    pub(crate) fn start(
        &self,
        program: &str,
        args: &[OsString],
        log: PathBuf,
        deadline: Duration,
    ) -> Result<Process> {
        Nested::from_env(self.test_dir)?;
        runner::start(&nested(program, args, Sink::File(log), deadline))
    }

    /// Starts a step with stdin held open and a deadline, capturing its output.
    pub(crate) fn start_with_stdin(
        &self,
        program: &str,
        args: &[OsString],
        deadline: Duration,
    ) -> Result<Process> {
        Nested::from_env(self.test_dir)?;
        runner::start_with_stdin(&nested(program, args, Sink::Capture, deadline))
    }

    /// The nested Noctalia's socket once it exists, checked like the other endpoints.
    pub(crate) fn noctalia_socket(&self) -> Result<Option<PathBuf>> {
        Nested::from_env(self.test_dir)?.noctalia_socket(&self.test_dir.run())
    }

    pub(crate) fn bind_marker(&self) -> PathBuf {
        self.test_dir.bind_marker()
    }

    /// Checks for the full interval, including at its end. Any observed event fails
    /// and saves a failure screenshot. Unlike `wait_until`, absence cannot pass early.
    pub(crate) fn still_absent(
        &self,
        step: &str,
        interval: Duration,
        mut present: impl FnMut() -> Result<bool>,
    ) -> Result<()> {
        let end = Instant::now() + interval;
        loop {
            if present()? {
                return self.failed(step, "unexpected event during absence check");
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(());
            }
            pause(POLL.min(left));
        }
    }

    fn failed<T>(&self, step: &str, failure: &str) -> Result<T> {
        let name = format!("failure-{step}.png");
        Err(Failure::new(match self.screenshot(&name) {
            Ok(()) => format!("{step}: {failure}; see {name}"),
            Err(screenshot) => format!("{step}: {failure}; no screenshot: {screenshot}"),
        }))
    }

    /// Saves a PNG of the nested output in the artifacts directory.
    pub(crate) fn screenshot(&self, name: &str) -> Result<()> {
        let path = self.artifact(name);
        self.run("grim", &["-o".into(), "winit".into(), path.into()])
            .map(drop)
    }

    /// Checks `condition` until it returns a value or `deadline` passes. On timeout it
    /// saves `failure-<step>.png` before failing.
    pub(crate) fn wait_until<T>(
        &mut self,
        step: &str,
        what: &str,
        deadline: Duration,
        mut condition: impl FnMut(&mut Self) -> Result<Option<T>>,
    ) -> Result<T> {
        let end = Instant::now() + deadline;
        loop {
            let value = condition(self)?;
            // A check can take a niri request's own deadline; a value it returns late
            // doesn't count.
            if Instant::now() > end {
                break;
            }
            if let Some(value) = value {
                return Ok(value);
            }
            pause(POLL);
        }
        let failure = format!("{step}: {what} not seen within {deadline:?}");
        let name = format!("failure-{step}.png");
        Err(Failure::new(match self.screenshot(&name) {
            Ok(()) => format!("{failure}; see {name}"),
            Err(screenshot) => format!("{failure}; no screenshot: {screenshot}"),
        }))
    }
}

/// Test steps stay in the supervisor's process group, so the deadline in `harness run`
/// kills them along with niri.
fn nested<'a>(
    program: &'a str,
    args: &[OsString],
    output: Sink,
    deadline: Duration,
) -> Invocation<'a> {
    Invocation {
        program,
        args: args.to_vec(),
        env: ChildEnv::Inherit,
        output,
        group: Group::Caller,
        deadline,
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "the harness is synchronous, so there is no async runtime to block; this is \
              the poll interval of a wait that has its own deadline"
)]
fn pause(duration: Duration) {
    std::thread::sleep(duration);
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::net::UnixListener;

    use super::*;

    /// A session whose endpoints never pass the nested check, so nothing can be spawned.
    fn session<'a>(test_dir: &'a TestDir, dir: &'a Path) -> (Session<'a>, UnixListener) {
        let socket = dir.join("niri.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let niri = Connection::open(&socket).unwrap();
        let log = Log::create(&dir.join("log"), false).unwrap();
        (Session::new(test_dir, dir, log, niri), listener)
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("harness-session-{name}-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn wait_until_returns_once_the_condition_holds() {
        let dir = scratch("holds");
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let (mut session, _listener) = session(&test_dir, &dir);
        let mut checks = 0;
        let value = session.wait_until("step", "three checks", Duration::from_secs(5), |_| {
            checks += 1;
            Ok((checks == 3).then_some(checks))
        });
        assert_eq!(value.unwrap(), 3);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wait_until_times_out_and_tries_a_failure_screenshot() {
        let dir = scratch("timeout");
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let (mut session, _listener) = session(&test_dir, &dir);
        let started = Instant::now();
        let failure = session
            .wait_until("c4", "a motion", Duration::from_millis(200), |_| {
                Ok(None::<()>)
            })
            .unwrap_err()
            .to_string();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(
            failure.starts_with("c4: a motion not seen within 200ms; no screenshot: "),
            "{failure}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wait_until_rejects_a_value_found_after_the_deadline() {
        let dir = scratch("late");
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let (mut session, _listener) = session(&test_dir, &dir);
        let failure = session.wait_until("step", "a slow value", Duration::from_millis(20), |_| {
            pause(Duration::from_millis(100));
            Ok(Some(42))
        });
        let message = failure.unwrap_err().to_string();
        assert!(
            message.starts_with("step: a slow value not seen within 20ms"),
            "{message}"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn wait_until_stops_at_the_first_error() {
        let dir = scratch("error");
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let (mut session, _listener) = session(&test_dir, &dir);
        let failure = session.wait_until("step", "x", Duration::from_secs(5), |_| {
            Err::<Option<()>, _>(Failure::new("niri went away"))
        });
        assert_eq!(failure.unwrap_err().to_string(), "niri went away");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn absence_waits_the_full_interval_and_checks_at_the_end() {
        let dir = scratch("absent");
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let (session, _listener) = session(&test_dir, &dir);
        let started = Instant::now();
        let mut checks = 0;
        session
            .still_absent("step", Duration::from_millis(120), || {
                checks += 1;
                Ok(false)
            })
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(120));
        assert!(checks >= 3);
        let failure = session.still_absent("step", Duration::from_millis(100), || Ok(true));
        assert!(
            failure
                .unwrap_err()
                .to_string()
                .contains("unexpected event")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn absence_rejects_an_event_at_the_final_check_and_propagates_errors() {
        let dir = scratch("absence-final");
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let (session, _listener) = session(&test_dir, &dir);
        let started = Instant::now();
        assert!(
            session
                .still_absent("step", Duration::from_millis(100), || {
                    Ok(started.elapsed() >= Duration::from_millis(100))
                })
                .is_err()
        );
        let error = session
            .still_absent("step", Duration::from_secs(1), || {
                Err(Failure::new("read failed"))
            })
            .unwrap_err();
        assert_eq!(error.to_string(), "read failed");
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nothing_starts_outside_the_nested_session() {
        let dir = scratch("outside");
        let test_dir = TestDir::open(PathBuf::from("/r/t")).unwrap();
        let (session, _listener) = session(&test_dir, &dir);
        assert!(session.run("true", &[]).is_err());
        let started = session.start("true", &[], dir.join("out"), Duration::from_secs(5));
        assert!(started.is_err());
        assert!(!dir.join("out").exists());
        assert!(
            session
                .start_with_stdin("true", &[], Duration::from_secs(1))
                .is_err()
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
