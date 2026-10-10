//! `--measure`: what the servers cost against the nested niri, for docs/results/engine.md.
//! For 1, 3 and 10 clients, each process's PSS, RSS and open files, and its CPU time while
//! idle; the `status` and `screenshot` round trips with 1 and 10 clients; a `status`
//! behind another client's screenshot; and, in shared mode, 200 clients coming and going.
//! It writes `measure.json` to the artifacts and a summary to the log.

mod proc;

use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

use serde_json::{Value, json};

use crate::failure::{Context as _, Failure, Result};
use crate::mcp::{self, Client, field, structured};
use crate::session::{self, Session};

const NOCTALIA_DEADLINE: Duration = Duration::from_secs(280);
const SERVER_DEADLINE: Duration = Duration::from_secs(270);
const READY: Duration = Duration::from_secs(20);
const SETS: [usize; 3] = [1, 3, 10];
/// How long the clients idle before the samples, and between two samples.
const IDLE: Duration = Duration::from_secs(5);
const SAMPLE_GAP: Duration = Duration::from_secs(1);
const SAMPLES: usize = 3;
const CPU_WINDOW: Duration = Duration::from_secs(10);
const CALLS: usize = 50;
const BEHIND: u32 = 20;
/// How much later each try sends its `status` than the last one did, so that the tries
/// cover the whole screenshot, the capture and the encoding.
const BEHIND_STEP: Duration = Duration::from_micros(500);
const CYCLES: usize = 200;
const CYCLE_SAMPLE: usize = 50;
/// The engine's two seconds of idle grace, and room to exit.
const ENGINE_EXIT: Duration = Duration::from_secs(5);

pub(crate) fn run(session: &mut Session<'_>, server: &str) -> Result<()> {
    let noctalia = session.start(
        "noctalia",
        &[],
        session.artifact("noctalia.log"),
        NOCTALIA_DEADLINE,
    )?;
    let mut probe = Client::start(session, server, "harness-measure", SERVER_DEADLINE)?;
    let status = mcp::ready(session, &mut probe, "measure-ready", READY)?;
    let shared = field(&status, "/engine/mode") == "shared";
    probe.stop()?;
    settle(session, shared)?;
    let mut sets = Vec::new();
    for clients in SETS {
        sets.push(set(session, server, clients, shared)?);
    }
    let latency = json!({
        "1": latency(session, server, 1, shared)?,
        "10": latency(session, server, 10, shared)?,
    });
    let behind = behind_a_screenshot(session, server, shared)?;
    let churn = if shared {
        json!({
            "anchored": churn(session, server, true)?,
            "unanchored": churn(session, server, false)?,
        })
    } else {
        Value::Null
    };
    let report = json!({
        "mode": if shared { "shared" } else { "standalone" },
        "clock_ticks_per_second": clock_ticks(session)?,
        "sets": sets, "latency_ms": latency, "status_behind_a_screenshot_ms": behind,
        "churn": churn,
    });
    let path = session.artifact("measure.json");
    fs::write(&path, format!("{report:#}\n")).context(format!("write {}", path.display()))?;
    session.log(&format!("measure: {report}"))?;
    noctalia.stop().map(drop)
}

/// Starts `count` clients named after `label`, timing the first one's start: from starting
/// its server to the reply to `initialize`. A client's log, which holds its replies, is
/// named after the client, so each measurement's clients need names of their own.
fn start(
    session: &Session<'_>,
    server: &str,
    label: &str,
    count: usize,
) -> Result<(Vec<Client>, Duration)> {
    let mut clients = Vec::new();
    let mut first = Duration::ZERO;
    for n in 0..count {
        let name = format!("harness-measure-{label}-{n}");
        let (client, took) = Client::start_timed(session, server, &name, SERVER_DEADLINE)?;
        if n == 0 {
            first = took;
        }
        clients.push(client);
    }
    Ok((clients, first))
}

fn stop(session: &mut Session<'_>, clients: Vec<Client>, shared: bool) -> Result<()> {
    for client in clients {
        client.stop()?;
    }
    settle(session, shared)
}

/// In shared mode, waits for the engine to exit, so that the next measurement starts
/// cold, as a standalone one does.
fn settle(session: &mut Session<'_>, shared: bool) -> Result<()> {
    if !shared {
        return Ok(());
    }
    let socket = session.control_dir()?.join("engine.sock");
    session.wait_until(
        "measure-engine-exit",
        "the engine's idle exit",
        ENGINE_EXIT,
        |_| Ok((!socket.exists()).then_some(())),
    )
}

/// One set: `count` clients that each ask for `status`, `desktop_state` and a screenshot,
/// then idle; every process's memory and files, then its CPU time over `CPU_WINDOW`.
fn set(session: &mut Session<'_>, server: &str, count: usize, shared: bool) -> Result<Value> {
    let (mut clients, first) = start(session, server, &format!("set{count}"), count)?;
    let mut serving = Vec::new();
    for client in &mut clients {
        let status = structured(&client.call(session, "status", json!({}))?)?;
        structured(&client.call(session, "desktop_state", json!({}))?)?;
        structured(&client.call(session, "screenshot", json!({"target": "focused_output"}))?)?;
        let role = if shared { "bridge" } else { "server" };
        serving.push((client.pid(), role));
        // The engine, or for a server too old to name it, the server itself.
        let engine = field(&status, "/engine/pid").as_u64();
        if let Some(engine) = engine.filter(|_| shared) {
            serving.push((i32::try_from(engine).context("the engine's PID")?, "engine"));
        }
    }
    serving.sort_unstable();
    serving.dedup();
    let watched: Vec<i32> = serving
        .iter()
        .filter(|(_, role)| *role != "bridge")
        .map(|(pid, _)| *pid)
        .collect();
    for guardian in guardians(&watched)? {
        serving.push((guardian, "guardian"));
    }
    session::pause(IDLE);
    let processes = sample(&serving)?;
    stop(session, clients, shared)?;
    Ok(json!({"clients": count, "first_client_start_ms": as_millis(first), "processes": processes}))
}

/// Each process's median PSS, RSS and open files over `SAMPLES`, and its CPU ticks over
/// `CPU_WINDOW` after them.
fn sample(processes: &[(i32, &str)]) -> Result<Vec<Value>> {
    let mut samples: BTreeMap<i32, Vec<(u64, u64, u64)>> = BTreeMap::new();
    for round in 0..SAMPLES {
        if round > 0 {
            session::pause(SAMPLE_GAP);
        }
        for (pid, role) in processes {
            samples
                .entry(*pid)
                .or_default()
                .push(memory(*pid).context(format!("sample the {role}"))?);
        }
    }
    let at_start: Vec<u64> = processes
        .iter()
        .map(|(pid, role)| ticks(*pid).context(format!("sample the {role}")))
        .collect::<Result<_>>()?;
    session::pause(CPU_WINDOW);
    let mut values = Vec::new();
    for ((pid, role), before) in processes.iter().zip(at_start) {
        let taken = samples.get(pid).cloned().unwrap_or_default();
        let pick = |of: fn(&(u64, u64, u64)) -> u64| {
            proc::median(&taken.iter().map(of).collect::<Vec<_>>())
        };
        values.push(json!({
            "pid": pid, "role": role,
            "pss_kib": pick(|sample| sample.0), "rss_kib": pick(|sample| sample.1),
            "fds": pick(|sample| sample.2), "cpu_ticks": ticks(*pid)?.saturating_sub(before),
        }));
    }
    Ok(values)
}

/// PSS and RSS in KiB, and open files.
fn memory(pid: i32) -> Result<(u64, u64, u64)> {
    let read = |file: &str| {
        fs::read_to_string(format!("/proc/{pid}/{file}"))
            .context(format!("read /proc/{pid}/{file}"))
    };
    let pss = proc::pss_kib(&read("smaps_rollup")?);
    let rss = proc::rss_kib(&read("status")?);
    let fds = fs::read_dir(format!("/proc/{pid}/fd"))
        .context(format!("list /proc/{pid}/fd"))?
        .count();
    match (pss, rss) {
        (Some(pss), Some(rss)) => Ok((pss, rss, u64::try_from(fds).context("a count")?)),
        _ => Err(Failure::new(format!("no PSS or RSS for {pid}"))),
    }
}

fn ticks(pid: i32) -> Result<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))
        .context(format!("read /proc/{pid}/stat"))?;
    proc::cpu_ticks(&stat).ok_or_else(|| Failure::new(format!("no CPU time in /proc/{pid}/stat")))
}

/// The processes running `guard <pid>` for one of `watched`.
fn guardians(watched: &[i32]) -> Result<Vec<i32>> {
    let mut found = Vec::new();
    for entry in fs::read_dir("/proc").context("list /proc")? {
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        // A process can exit between the listing and this read.
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<&[u8]> = cmdline.split(|byte| *byte == 0).collect();
        let named = |watch: &i32| args.get(2) == Some(&watch.to_string().as_bytes());
        if args.get(1) == Some(&b"guard".as_slice()) && watched.iter().any(named) {
            found.push(pid);
        }
    }
    Ok(found)
}

/// With `count` clients connected, the first one's median `status` and `screenshot`
/// round trips over `CALLS` calls each.
fn latency(session: &mut Session<'_>, server: &str, count: usize, shared: bool) -> Result<Value> {
    let (mut clients, _) = start(session, server, &format!("latency{count}"), count)?;
    let first = clients
        .first_mut()
        .ok_or_else(|| Failure::new("no client"))?;
    let status = round_trips(first, "status", &json!({}))?;
    let shot = round_trips(first, "screenshot", &json!({"target": "focused_output"}))?;
    stop(session, clients, shared)?;
    Ok(json!({"status": status, "screenshot": shot}))
}

/// The median of `CALLS` round trips of `tool`, in milliseconds.
fn round_trips(client: &mut Client, tool: &str, arguments: &Value) -> Result<f64> {
    let mut times = Vec::new();
    for _ in 0..CALLS {
        let (result, round_trip) = client.timed_call(tool, arguments.clone())?;
        structured(&result)?;
        times.push(round_trip);
    }
    median_millis(&times)
}

fn median_millis(took: &[Duration]) -> Result<f64> {
    let median = proc::median(took).ok_or_else(|| Failure::new("no round trips"))?;
    Ok(as_millis(median))
}

fn as_millis(took: Duration) -> f64 {
    took.as_secs_f64() * 1000.0
}

/// The median and longest `status` round trip of one client while another's screenshot
/// runs, over `BEHIND` tries: what the engine's one thread costs a client behind another's
/// work.
fn behind_a_screenshot(session: &mut Session<'_>, server: &str, shared: bool) -> Result<Value> {
    let (mut clients, _) = start(session, server, "behind", 2)?;
    let [shooter, asker] = clients.as_mut_slice() else {
        return Err(Failure::new("two clients"));
    };
    let mut took = Vec::new();
    for n in 0..BEHIND {
        let id = shooter.start_call("screenshot", json!({"target": "focused_output"}))?;
        session::pause(BEHIND_STEP * n);
        let (status, round_trip) = asker.timed_call("status", json!({}))?;
        structured(&status)?;
        took.push(round_trip);
        structured(&shooter.result(session, id)?)?;
    }
    stop(session, clients, shared)?;
    let longest = took.iter().max().copied().unwrap_or_default();
    Ok(json!({"median": median_millis(&took)?, "max": as_millis(longest)}))
}

/// `CYCLES` clients that start, ask for `status` and go, one after another, with or
/// without an anchor client connected throughout. The engine's PSS and open files after
/// the first cycle and every `CYCLE_SAMPLE` cycles; without an anchor, whether an engine,
/// guardian or socket is left once the engine's idle grace has passed.
fn churn(session: &mut Session<'_>, server: &str, anchored: bool) -> Result<Value> {
    let kind = if anchored { "anchored" } else { "unanchored" };
    let anchor = if anchored {
        Some(Client::start(
            session,
            server,
            "harness-measure-anchor",
            SERVER_DEADLINE,
        )?)
    } else {
        None
    };
    let mut engine = None;
    let mut series = Vec::new();
    for cycle in 1..=CYCLES {
        let name = format!("harness-measure-churn-{kind}-{cycle}");
        let mut client = Client::start(session, server, &name, SERVER_DEADLINE)?;
        let status = structured(&client.call(session, "status", json!({}))?)?;
        let pid = i32::try_from(field(&status, "/engine/pid").as_u64().unwrap_or_default())
            .context("the engine's PID")?;
        client.stop()?;
        if engine.is_some_and(|engine| engine != pid) {
            return Err(Failure::new(format!(
                "churn: the engine changed at cycle {cycle}"
            )));
        }
        engine = Some(pid);
        if cycle == 1 || cycle % CYCLE_SAMPLE == 0 {
            let (pss, _, fds) = memory(pid)?;
            series.push(json!({"cycle": cycle, "pss_kib": pss, "fds": fds}));
        }
    }
    if let Some(anchor) = anchor {
        anchor.stop()?;
    }
    settle(session, true)?;
    let left = engine.filter(|pid| fs::metadata(format!("/proc/{pid}")).is_ok());
    let guardians = guardians(&engine.into_iter().collect::<Vec<_>>())?;
    Ok(json!({"series": series, "engine_left": left, "guardians_left": guardians}))
}

fn clock_ticks(session: &Session<'_>) -> Result<u64> {
    let output = session.run("getconf", &["CLK_TCK".into()])?;
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .context("read getconf CLK_TCK")
}
