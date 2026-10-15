//! `--upgrade <version>`: installs a published release into a throwaway HOME with
//! `install.sh`, leaves the state a user's desktop has behind (two clients on the
//! release's shared engine, one holding the lease, an audit log and a policy file), then
//! installs the build under test over it and checks what an upgrade meets: the old engine
//! and its clients still running, a new client beside them, each reading the other's
//! input-dirty marker, an old engine killed mid-input and `recover` after it, an old
//! client meeting the new engine, the audit log and the policy file.

mod install;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process};

use serde_json::{Value, json};

use crate::control::ZEROED;
use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field, structured};
use crate::runner::Process;
use crate::session::Session;
use install::{CURRENT, Place};

/// Within what is left of the run's deadline.
const NOCTALIA_DEADLINE: Duration = Duration::from_secs(230);
const CLIENT_DEADLINE: Duration = Duration::from_secs(220);
const WEV_DEADLINE: Duration = Duration::from_secs(220);
const READY: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(5);
/// Long enough to be typing still when the other side looks.
const TYPED: usize = 1000;
const MARKER_POLL: Duration = Duration::from_millis(1);
/// The guardian's own ten seconds for its releases, and room to exit.
const GUARDIAN_WAIT: Duration = Duration::from_secs(12);
/// Valid in every release: a key each one reads.
const POLICY: &str = "deny_input_app_ids = [\"org.keepassxc.KeePassXC\"]\n";
/// A key added after 0.1.1.
const NEWER_POLICY: &str = "turn_on_accessibility = false\n";

pub(crate) fn run(session: &mut Session<'_>, server: &str, from: &str) -> Result<()> {
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let place = Place::create(session, server)?;
    place.release_current(session)?;
    let printed = place.install(session, from)?;
    session.log(&format!(
        "upgrade: installed {from}: {}",
        first_line(&printed)
    ))?;
    write_policy(session)?;
    newer_key_refused(session, &place)?;
    let wev = start_wev(session)?;
    let mut old = Old::start(session, &place, from)?;
    let audit_before = audit(session)?;
    old.install_while_typing(session, &place)?;
    if !place.holds_current()? {
        return Err(Failure::new(
            "upgrade: the installed binary or skill isn't the build under test's",
        ));
    }
    old.still_runs(session)?;
    let mut new = client(session, &place, "upgrade-new")?;
    beside(session, &mut old, &mut new)?;
    crashed(session, &place, old.a, &mut new, old.engine)?;
    let mut shared = client(session, &place, "upgrade-new-shared")?;
    shared_again(session, &mut shared, &mut old.b)?;
    audit_kept(session, &audit_before)?;
    policy_kept(session, &mut new)?;
    for client in [old.b, new, shared] {
        client.stop()?;
    }
    crate::shared::no_engine_left(session, "upgrade-engines-exit")?;
    wev.stop()?;
    noctalia.stop().map(drop)
}

fn first_line(text: &str) -> &str {
    text.lines()
        .find(|line| line.starts_with("Installed"))
        .unwrap_or_default()
}

fn write_policy(session: &Session<'_>) -> Result<()> {
    let dir = session.test_dir().config().join("niri-computer-use");
    fs::create_dir_all(&dir).context(format!("create {}", dir.display()))?;
    let path = dir.join("policy.toml");
    fs::write(&path, POLICY).context(format!("write {}", path.display()))
}

/// The old build's `status` reads a policy file with a key added since as invalid, which
/// leaves its servers read-only.
fn newer_key_refused(session: &mut Session<'_>, place: &Place) -> Result<()> {
    let config = session.test_dir().root().join("newer-config");
    let dir = config.join("niri-computer-use");
    fs::create_dir_all(&dir).context(format!("create {}", dir.display()))?;
    let path = dir.join("policy.toml");
    fs::write(&path, NEWER_POLICY).context(format!("write {}", path.display()))?;
    let mut xdg = OsString::from("XDG_CONFIG_HOME=");
    xdg.push(&config);
    let args = place.env(&[xdg, place.binary().into(), "status".into()]);
    let output = session.run("env", &args)?;
    let status: Value = serde_json::from_slice(&output.stdout).context("parse the old status")?;
    let policy = field(&status, "/policy");
    expect(
        field(policy, "/state") == "invalid",
        "the old build refusing a newer policy key",
        &policy.to_string(),
    )?;
    session.log(&format!(
        "upgrade: the old build reads a policy file with {} as {policy}",
        NEWER_POLICY.trim_end()
    ))
}

fn start_wev(session: &mut Session<'_>) -> Result<Process> {
    let args = ["-oL", "wev"].map(OsString::from);
    let process = session.start("stdbuf", &args, session.artifact("wev.log"), WEV_DEADLINE)?;
    crate::supervise::wait_for_wev(session)?;
    Ok(process)
}

/// A client of the installed binary, in shared mode, ready once Noctalia runs.
fn client(session: &mut Session<'_>, place: &Place, name: &str) -> Result<Client> {
    let args = place.env(&[place.binary().into(), "serve".into()]);
    let mut client = Client::start_command(session, "env", &args, name, CLIENT_DEADLINE)?;
    mcp::ready(session, &mut client, &format!("{name}-ready"), READY)?;
    Ok(client)
}

/// The release's two clients and the engine they share. A holds the lease.
struct Old {
    a: Client,
    b: Client,
    engine: u64,
}

impl Old {
    fn start(session: &mut Session<'_>, place: &Place, from: &str) -> Result<Self> {
        let mut a = client(session, place, "upgrade-old-a")?;
        let status = structured(&a.call(session, "status", json!({}))?)?;
        let engine = field(&status, "/engine");
        if field(engine, "/mode") != "shared" || field(engine, "/version") != from {
            return Err(Failure::new(format!(
                "upgrade: {from}'s client isn't on a shared engine of {from}: {engine}"
            )));
        }
        let engine = field(engine, "/pid")
            .as_u64()
            .ok_or_else(|| Failure::new(format!("status names no engine: {status}")))?;
        structured(&a.call(session, "acquire_desktop", json!({}))?)?;
        let mut b = client(session, place, "upgrade-old-b")?;
        let second = structured(&b.call(session, "status", json!({}))?)?;
        if field(&second, "/engine/pid") != engine {
            return Err(Failure::new(format!(
                "upgrade: {from}'s second client isn't on engine {engine}: {second}"
            )));
        }
        session.log(&format!(
            "upgrade: {from}'s engine {engine} serves two clients, one holding the lease"
        ))?;
        Ok(Self { a, b, engine })
    }

    /// Installs the build under test while A's `type_text` is held mid-input, as an
    /// upgrade may land while an agent types, and requires that typing to finish.
    fn install_while_typing(&mut self, session: &mut Session<'_>, place: &Place) -> Result<()> {
        let serving = i32::try_from(self.engine).context("the engine's PID")?;
        let id = start_typing(session, &mut self.a, serving)?;
        let installed = place.install(session, CURRENT);
        signal(serving, Signal::CONT)?;
        installed?;
        let typed = self.a.result(session, id)?;
        expect(
            field(&typed, "/isError") == false,
            "the typing held over the install finished",
            &typed.to_string(),
        )?;
        marker_gone(session)?;
        session.log("upgrade: installed the build under test while A's type_text was held mid-input; it finished")
    }

    /// After the install, the old engine runs the build it started with, whose file is
    /// gone, and still answers its clients.
    fn still_runs(&mut self, session: &mut Session<'_>) -> Result<()> {
        let exe = fs::read_link(format!("/proc/{}/exe", self.engine))
            .context(format!("read engine {}'s exe", self.engine))?;
        let status = structured(&self.b.call(session, "status", json!({}))?)?;
        let replaced = field(&status, "/engine/replaced");
        if !exe.to_string_lossy().ends_with(" (deleted)") || replaced != true {
            return Err(Failure::new(format!(
                "upgrade: the old engine runs {}, replaced {replaced}",
                exe.display()
            )));
        }
        session.log(&format!(
            "upgrade: the old engine still runs {} and says replaced: true",
            exe.display()
        ))
    }
}

/// A new client beside the old engine: served standalone, told why and what to do, and
/// refused the lease A holds. Each side reads the other's marker while it types.
fn beside(session: &mut Session<'_>, old: &mut Old, new: &mut Client) -> Result<()> {
    let status = structured(&new.call(session, "status", json!({}))?)?;
    let fallback = field(&status, "/engine/fallback");
    session.log(&format!(
        "upgrade: beside the old engine, the new client is {} with fallback {fallback}",
        field(&status, "/engine/mode")
    ))?;
    expect(
        field(&status, "/engine/mode") == "standalone"
            && fallback.as_str().is_some_and(|text| {
                text.contains("engine_version")
                    && text.contains("restart the MCP clients the engine serves")
            }),
        "the new client served standalone, refused by the old engine, and told what to do",
        &status.to_string(),
    )?;
    let refused = new.call(session, "acquire_desktop", json!({}))?;
    let detail = field(&refused, "/structuredContent/detail");
    expect(
        field(&refused, "/structuredContent/error") == "lease_held"
            && detail
                .as_str()
                .is_some_and(|text| text.contains("upgrade-old-a")),
        "the new client refused the lease the old one holds",
        &refused.to_string(),
    )?;
    structured(
        &old.a
            .call(session, "release_desktop", json!({"restore_focus": false}))?,
    )?;
    structured(&new.call(session, "acquire_desktop", json!({}))?)?;
    let seen = while_typing(session, new, &mut old.b)?;
    session.log(&format!(
        "upgrade: the old engine reads the new marker as {seen}"
    ))?;
    structured(&new.call(session, "release_desktop", json!({"restore_focus": false}))?)?;
    structured(&old.a.call(session, "acquire_desktop", json!({}))?).map(drop)
}

/// Starts `typist`'s long `type_text` into `wev`, holds it while its marker is set, and
/// returns how `reader`'s `status` reads the marker then. Waits for the typing to end.
fn while_typing(
    session: &mut Session<'_>,
    typist: &mut Client,
    reader: &mut Client,
) -> Result<Value> {
    let serving = typist.pid();
    let id = start_typing(session, typist, serving)?;
    let read = structured(&reader.call(session, "status", json!({}))?);
    signal(serving, Signal::CONT)?;
    let status = read?;
    let seen = field(&status, "/input_dirty").clone();
    expect(
        !seen.is_null(),
        "the other build sees the marker",
        &status.to_string(),
    )?;
    let typed = typist.result(session, id)?;
    expect(
        field(&typed, "/isError") == false,
        "the typing finished",
        &typed.to_string(),
    )?;
    marker_gone(session)?;
    Ok(seen)
}

/// Starts a long `type_text` and returns its id once its marker names the input that went
/// out, with `serving`, the process that types, stopped: native keys, or 0.1.0's `wtype`
/// child. Typing a thousand characters takes less than a tenth of a second, so the marker
/// is looked for every millisecond.
fn start_typing(session: &Session<'_>, typist: &mut Client, serving: i32) -> Result<u64> {
    let id = typist.start_call(
        "type_text",
        json!({"text": "a".repeat(TYPED), "expect": {"app_id": "wev"}}),
    )?;
    let marker = marker(session)?;
    let end = Instant::now() + WAIT;
    while !names_input(&marker) {
        if Instant::now() > end {
            return Err(Failure::new(format!(
                "upgrade: no input-dirty marker within {WAIT:?} of a type_text"
            )));
        }
        crate::session::pause(MARKER_POLL);
    }
    signal(serving, Signal::STOP)?;
    // The typing may have ended between the look and the stop.
    if !names_input(&marker) {
        signal(serving, Signal::CONT)?;
        return Err(Failure::new(
            "upgrade: the typing ended before it could be held; type more",
        ));
    }
    Ok(id)
}

fn names_input(marker: &Path) -> bool {
    fs::read(marker)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|marker| marker.get("keyboard").is_some() || marker.get("child").is_some())
}

fn signal(pid: i32, signal: Signal) -> Result<()> {
    let pid = Pid::from_raw(pid).ok_or_else(|| Failure::new("no serving PID"))?;
    kill_process(pid, signal).context(format!("send {signal:?} to PID {}", pid.as_raw_nonzero()))
}

/// Whether `pid` has exited: gone, or a zombie not yet reaped.
fn ended(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat")).map_or(true, |stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| rest.starts_with('Z') || rest.starts_with('X'))
    })
}

fn marker(session: &Session<'_>) -> Result<PathBuf> {
    Ok(session.control_dir()?.join("input-dirty"))
}

fn marker_gone(session: &mut Session<'_>) -> Result<()> {
    let marker = marker(session)?;
    session.wait_until("upgrade-marker-gone", "the marker removed", WAIT, |_| {
        Ok((!marker.try_exists().context("check the marker")?).then_some(()))
    })
}

/// The old engine killed while A types: the new client reads its marker, is refused until
/// the new build's `recover`, which clears it.
fn crashed(
    session: &mut Session<'_>,
    place: &Place,
    mut a: Client,
    new: &mut Client,
    engine: u64,
) -> Result<()> {
    let serving = i32::try_from(engine).context("the engine's PID")?;
    start_typing(session, &mut a, serving)?;
    let read = structured(&new.call(session, "status", json!({}))?);
    let waited = new.call(session, "acquire_desktop", json!({}));
    let guardian = crate::input::guardian::of(u32::try_from(engine).context("the engine's PID")?)?;
    signal(serving, Signal::KILL)?;
    let seen = field(&read?, "/input_dirty").clone();
    session.log(&format!(
        "upgrade: the new build reads the old marker as {seen}"
    ))?;
    read_whole(&seen, "the new build reading the old marker")?;
    let waited = field(&waited?, "/structuredContent").clone();
    session.log(&format!(
        "upgrade: while the old engine types, the new client is refused with {waited}"
    ))?;
    expect(
        field(&waited, "/error") == "recovery_required"
            && field(&waited, "/detail").as_str().is_some_and(|detail| {
                detail.contains("another server's input may still be finishing")
            }),
        "the new client told the old engine's input may still be finishing",
        &waited.to_string(),
    )?;
    a.stop()?;
    // The guardian holds the lease while it sends the releases the marker names, and
    // `recover` needs the lease.
    session.wait_until(
        "upgrade-guardian",
        "the killed engine's crash guardian ending",
        GUARDIAN_WAIT,
        |_| Ok(ended(guardian).then_some(())),
    )?;
    let left = fs::read_to_string(marker(session)?).context("read the marker left")?;
    session.log(&format!(
        "upgrade: the killed engine left the marker {left}"
    ))?;
    let refused = new.call(session, "acquire_desktop", json!({}))?;
    session.log(&format!(
        "upgrade: the new client is refused with {}",
        field(&refused, "/structuredContent")
    ))?;
    expect(
        field(&refused, "/structuredContent/error") == "recovery_required",
        "recovery_required for the old marker",
        &refused.to_string(),
    )?;
    recover(session, place, &left)?;
    structured(&new.call(session, "acquire_desktop", json!({}))?)?;
    structured(&new.call(session, "release_desktop", json!({"restore_focus": false}))?).map(drop)
}

/// A marker `status` read as a marker, not as unreadable.
fn read_whole(seen: &Value, what: &str) -> Result<()> {
    expect(
        field(seen, "/operation").is_string() && field(seen, "/error").is_null(),
        what,
        &seen.to_string(),
    )
}

/// The new build's `recover`, answered `yes`, clears the old marker `left`, ending the
/// input child a 0.1.0 marker names.
fn recover(session: &mut Session<'_>, place: &Place, left: &str) -> Result<()> {
    let script = format!(
        "printf 'yes\\n' | '{}' recover 2>&1; echo exit=$?",
        place.binary().display()
    );
    let args = place.env(&["sh".into(), "-c".into(), script.into()]);
    let output = session.run("env", &args)?;
    let said = String::from_utf8_lossy(&output.stdout).into_owned();
    session.log(&format!("upgrade: recover said: {said}"))?;
    let child =
        serde_json::from_str::<Value>(left).is_ok_and(|marker| marker.get("child").is_some());
    expect(
        said.contains("exit=0")
            && (!child || said.contains("input child, PID") && said.contains(ZEROED)),
        "recover of the old marker",
        &said,
    )?;
    marker_gone(session)
}

/// With the old engine gone, a new client starts the new build's engine, and the old
/// client still running meets it.
fn shared_again(session: &mut Session<'_>, shared: &mut Client, old: &mut Client) -> Result<()> {
    let status = structured(&shared.call(session, "status", json!({}))?)?;
    expect(
        field(&status, "/engine/mode") == "shared" && field(&status, "/engine/fallback").is_null(),
        "a new client on the new build's engine",
        &status.to_string(),
    )?;
    let first = old.call(session, "status", json!({}))?;
    let met = if field(&first, "/structuredContent/error") == "engine_lost" {
        old.call(session, "status", json!({}))?
    } else {
        first
    };
    session.log(&format!(
        "upgrade: the old client meets the new engine: {}",
        field(&met, "/structuredContent")
    ))?;
    expect(
        field(&met, "/structuredContent/error") == "engine_unavailable"
            && field(&met, "/structuredContent/detail")
                .as_str()
                .is_some_and(|detail| detail.contains("restart this MCP client")),
        "the old client refused by the new engine and told to restart",
        &met.to_string(),
    )
}

/// Every line of the audit log, as JSON.
fn audit(session: &Session<'_>) -> Result<Vec<Value>> {
    let path = session
        .test_dir()
        .state()
        .join("niri-computer-use/audit.jsonl");
    let text = fs::read_to_string(&path).context(format!("read {}", path.display()))?;
    text.lines()
        .map(|line| serde_json::from_str(line).context(format!("parse the audit line {line}")))
        .collect()
}

/// The new build appends to the old build's audit log, which keeps every old line.
fn audit_kept(session: &mut Session<'_>, before: &[Value]) -> Result<()> {
    let after = audit(session)?;
    let kept = after.get(..before.len()) == Some(before);
    expect(
        kept && after.len() > before.len(),
        "the audit log kept and appended to",
        &format!("{} lines before, {} after", before.len(), after.len()),
    )?;
    session.log(&format!(
        "upgrade: the audit log kept its {} old lines and has {} new ones",
        before.len(),
        after.len() - before.len()
    ))
}

/// The new build reads the release's policy file.
fn policy_kept(session: &mut Session<'_>, new: &mut Client) -> Result<()> {
    let status = structured(&new.call(session, "status", json!({}))?)?;
    let policy = field(&status, "/policy");
    expect(
        field(policy, "/state") == "loaded",
        "the release's policy file loaded",
        &policy.to_string(),
    )?;
    session.log(&format!(
        "upgrade: the new build loads the release's policy: {policy}"
    ))
}

fn expect(holds: bool, what: &str, seen: &str) -> Result<()> {
    if holds {
        Ok(())
    } else {
        Err(Failure::new(format!("upgrade: {what} failed; saw {seen}")))
    }
}
