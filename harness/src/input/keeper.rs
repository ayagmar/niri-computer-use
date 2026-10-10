//! The paste keeper against other clipboard clients, driving `niri-computer-use
//! paste-keeper` directly so the checks can choose when its stdin ends. A copy that
//! arrives while the keeper is told to finish stays, a reader that asked for the text
//! before the keeper ended still gets all of it, and a `k` that comes after the keeper
//! stopped waiting for it gets no `armed`, so the server sends no key.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process};
use serde_json::Value;

use super::WAIT;
use super::paste::{copy, listed, offered, own, owners_copy};
use crate::clipboard::{CANCELLED, TYPES};
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::field;
use crate::runner::Process;
use crate::session::Session;
use crate::slow_reader::{READ, REQUESTED};

const KEEPER_DEADLINE: Duration = Duration::from_secs(20);
const READER_DEADLINE: Duration = Duration::from_secs(20);
/// Several pipe buffers, so the keeper's write waits for the reader.
const LARGE: usize = 256 * 1024;
/// Longer than the keeper takes to end once replaced, shorter than its two-second
/// transfer deadline.
const READER_DELAY: Duration = Duration::from_secs(1);
/// The keeper picks between its stdin's end and a pending replacement at random, so a
/// keeper that trusts its stdin's end fails some round.
const ROUNDS: usize = 8;
/// Past the keeper's ten-second wait for the server's commands.
const STOPPED_WAITING: Duration = Duration::from_secs(13);
/// Far longer than the keeper takes to answer a `k` it admits.
const NO_ANSWER: Duration = Duration::from_secs(1);

pub(super) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    for round in 0..ROUNDS {
        replaced_as_it_ends(session, server, round)?;
    }
    transfer_outlives_the_keeper(session, server)?;
    late_k(session, server)?;
    session.log(&format!(
        "Paste keeper: a copy made while the keeper was paused and told to end stayed in {ROUNDS} of {ROUNDS} rounds; a reader that asked before a replacement and read {} ms later got all {LARGE} bytes; a `k` after the keeper's wait ended got no `armed` within {} ms and the user's copy stayed offered",
        READER_DELAY.as_millis(),
        NO_ANSWER.as_millis()
    ))
}

/// The keeper waits for `k` until its wait ends, restores the user's copy and serves it;
/// a `k` after that must not be admitted, and the user's copy stays.
fn late_k(session: &mut Session<'_>, server: &str) -> Result<()> {
    let (owner, copied) = own(session, "keeper-late-k")?;
    let (mut keeper, log) = start(session, server, b"pasted", "late-k")?;
    owner.wait()?;
    let done = session.wait_until(
        "keeper-late-k-restored",
        "the keeper's done report",
        STOPPED_WAITING,
        |_| {
            Ok(reports(&log)?
                .into_iter()
                .find(|report| field(report, "/report") == "done"))
        },
    )?;
    if field(&done, "/clipboard") != "restored" {
        return Err(Failure::new(format!(
            "paste keeper late k: reported {done} once its wait ended"
        )));
    }
    keeper.send(b"k".to_vec())?;
    session.still_absent("keeper-late-k-armed", NO_ANSWER, || {
        Ok(reports(&log)?
            .iter()
            .any(|report| field(report, "/report") == "armed"))
    })?;
    let after = offered(session)?;
    keeper.stop()?;
    if after != copied {
        return Err(Failure::new(format!(
            "paste keeper late k: the clipboard offers {after:?}, not the user's copy {copied:?}"
        )));
    }
    Ok(())
}

/// Pauses the keeper, lets another client copy, ends the keeper's stdin and resumes it:
/// the replacement and the end are both waiting when it wakes, and the copy must stay.
fn replaced_as_it_ends(session: &mut Session<'_>, server: &str, round: usize) -> Result<()> {
    let name = format!("ending-{round}");
    let (mut keeper, log) = start(session, server, b"pasted", &name)?;
    signal(&keeper, Signal::STOP)?;
    let owner = take_over(session, &name)?;
    keeper.feed(Vec::new())?;
    signal(&keeper, Signal::CONT)?;
    let report = done(session, keeper, &log)?;
    let kept =
        offered(session)? == owners_copy() && !session.test_dir().root().join(CANCELLED).exists();
    owner.stop()?;
    if field(&report, "/clipboard") != "replaced" || !kept {
        return Err(Failure::new(format!(
            "paste keeper round {round}: reported {report}; the newer copy {}",
            if kept { "stayed" } else { "was overwritten" }
        )));
    }
    Ok(())
}

/// A reader asks for a large text and reads it slowly; meanwhile another client copies,
/// which ends the keeper's part. The reader still gets every byte.
fn transfer_outlives_the_keeper(session: &mut Session<'_>, server: &str) -> Result<()> {
    let root = session.test_dir().root().to_owned();
    for note in [REQUESTED, READ] {
        if root.join(note).exists() {
            fs::remove_file(root.join(note)).context(format!("remove {note}"))?;
        }
    }
    let (keeper, log) = start(session, server, &vec![b'x'; LARGE], "transfer")?;
    let harness = std::env::current_exe().context("find the harness binary")?;
    let program = harness
        .to_str()
        .ok_or_else(|| Failure::new("the harness path isn't UTF-8"))?;
    let args: Vec<OsString> = vec![
        "slow-reader".into(),
        root.clone().into(),
        READER_DELAY.as_millis().to_string().into(),
        READER_DEADLINE.as_millis().to_string().into(),
    ];
    let reader = session.start(
        program,
        &args,
        session.artifact("slow-reader.log"),
        READER_DEADLINE,
    )?;
    session.wait_until(
        "keeper-transfer-requested",
        "the slow reader's request",
        WAIT,
        |session| {
            Ok(session
                .test_dir()
                .root()
                .join(REQUESTED)
                .exists()
                .then_some(()))
        },
    )?;
    let owner = take_over(session, "transfer")?;
    let report = done(session, keeper, &log)?;
    reader.wait()?;
    owner.stop()?;
    let read = fs::read_to_string(root.join(READ)).context("read the slow reader's count")?;
    if field(&report, "/clipboard") != "replaced" || read != LARGE.to_string() {
        return Err(Failure::new(format!(
            "paste keeper transfer: reported {report}; the slow reader got {read} of {LARGE} bytes"
        )));
    }
    Ok(())
}

/// Starts the clipboard owner and waits until the selection offers its types. It reads
/// none of them: one read from a paused keeper would wait for it, and one listed just
/// before the owner took over would fail.
fn take_over(session: &mut Session<'_>, name: &str) -> Result<Process> {
    let owner = copy(session, &format!("keeper-{name}"), false)?;
    let types: Vec<String> = TYPES.iter().map(|(mime, _)| (*mime).to_owned()).collect();
    session.wait_until(
        &format!("keeper-{name}-copied"),
        "the owner's types",
        WAIT,
        |session| Ok((listed(session)? == types).then_some(())),
    )?;
    Ok(owner)
}

/// Starts a keeper holding `text`, and waits until it holds the selection.
fn start(
    session: &mut Session<'_>,
    server: &str,
    text: &[u8],
    name: &str,
) -> Result<(Process, PathBuf)> {
    let log = session.artifact(&format!("keeper-{name}.log"));
    let mut keeper = session.serve(
        server,
        &["paste-keeper".into()],
        log.clone(),
        KEEPER_DEADLINE,
    )?;
    let length = u64::try_from(text.len()).context("the text's length")?;
    let mut framed = length.to_le_bytes().to_vec();
    framed.extend_from_slice(text);
    keeper.send(framed)?;
    session.wait_until(
        &format!("keeper-{name}-ready"),
        "the keeper's ready report",
        WAIT,
        |_| {
            let ready = reports(&log)?
                .iter()
                .any(|report| field(report, "/report") == "ready");
            Ok(ready.then_some(()))
        },
    )?;
    Ok((keeper, log))
}

/// Waits for the keeper's `done` report and then for it to end. A keeper that restored
/// keeps serving, so it is stopped instead and the caller sees the report.
fn done(session: &mut Session<'_>, keeper: Process, log: &Path) -> Result<Value> {
    let report = session.wait_until("keeper-done", "the keeper's done report", WAIT, |_| {
        Ok(reports(log)?
            .into_iter()
            .rfind(|report| field(report, "/report") == "done"))
    })?;
    if field(&report, "/clipboard") == "restored" {
        keeper.stop()?;
    } else {
        keeper.wait()?;
    }
    Ok(report)
}

/// The keeper's report lines so far.
fn reports(log: &Path) -> Result<Vec<Value>> {
    let text = fs::read_to_string(log).context(format!("read {}", log.display()))?;
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

fn signal(keeper: &Process, signal: Signal) -> Result<()> {
    let pid = Pid::from_raw(keeper.pid()).ok_or_else(|| Failure::new("the keeper has no PID"))?;
    kill_process(pid, signal).context(format!("send {signal:?} to the keeper"))
}
