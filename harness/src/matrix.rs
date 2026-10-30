//! `harness matrix BUILDS OUTPUT`: this checkout's server on each niri in BUILDS, the
//! lines `scripts/niri-build.sh` prints. Each niri gets the `--compat` probe; a release
//! with the pinned `niri-ipc`'s major and minor, and any niri that isn't a release tag,
//! such as `main`, also get the suites that act. OUTPUT gets a Markdown table of what each
//! niri did. The matrix fails when a release does other than its expectation: the pinned
//! release must pass everything, and any other release must be read-only with the
//! observation tools working. A niri that isn't a release is only recorded.

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use crate::compat;
use crate::config::Decorations;
use crate::failure::{Context as _, Failure, Result};
use crate::interrupt;
use crate::log;
use crate::mcp::field;
use crate::run::{self, Options, ServerChecks, ServerMode};
use crate::scale::Scale;

/// The `niri-ipc` pin of the server and the harness. A test keeps it equal to the pins.
const IPC_CRATE: &str = "26.4.0";
/// The suites that act, run on the niri versions the server supports.
const SUITES: [ServerChecks; 5] = [
    ServerChecks::Control,
    ServerChecks::Actions,
    ServerChecks::Input,
    ServerChecks::Shell,
    ServerChecks::A11y,
];

/// One line of `scripts/niri-build.sh`'s output.
#[derive(Debug, PartialEq, Eq)]
struct Build {
    reference: String,
    commit: String,
    /// The binary, or why it wasn't built.
    niri: std::result::Result<PathBuf, String>,
}

/// What a niri is expected to do with this server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expectation {
    /// A release the server supports: everything works.
    Full,
    /// Any other release: read-only, and the observation tools work.
    Observe,
    /// Not a release: whatever happens is recorded.
    Record,
}

/// What one niri did.
#[derive(Debug)]
struct Row {
    build: Build,
    expectation: Expectation,
    /// `compat.json`, when the probe got far enough to write it.
    report: Option<Value>,
    probe: std::result::Result<(), String>,
    suites: Vec<(ServerChecks, std::result::Result<(), String>)>,
    /// The runs' names, which are their directories under `target/e2e`.
    runs: Vec<String>,
}

pub(crate) fn run(builds_path: &str, output: &str) -> Result<()> {
    let text = fs::read_to_string(builds_path).context(format!("read {builds_path}"))?;
    let builds = builds(&text)?;
    let base = run::stamp()?;
    let mut rows = Vec::new();
    for build in builds {
        let row = test(build, &base, rows.len())?;
        log::echo(&format!(
            "matrix: {}: {}",
            row.build.reference,
            verdict(&row).cell()
        ));
        rows.push(row);
    }
    fs::write(output, render(&rows)).context(format!("write {output}"))?;
    let failed: Vec<&str> = rows
        .iter()
        .filter(|row| matches!(verdict(row), Verdict::Failed(_)))
        .map(|row| row.build.reference.as_str())
        .collect();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Failure::new(format!(
            "niri {} did other than expected; see {output}",
            failed.join(", ")
        )))
    }
}

/// Probes one niri, and runs the suites unless it is a release the server doesn't support.
fn test(build: Build, base: &str, index: usize) -> Result<Row> {
    let expectation = expectation(&build.reference);
    let mut row = Row {
        build,
        expectation,
        report: None,
        probe: Err("not run".to_owned()),
        suites: Vec::new(),
        runs: Vec::new(),
    };
    let Ok(niri) = row.build.niri.clone() else {
        return Ok(row);
    };
    let probe = format!("{base}-{index}-0");
    row.probe = nested(&niri, ServerChecks::Compat, &probe)?;
    row.report = fs::read_to_string(run::artifacts(&probe)?.join(compat::REPORT))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok());
    row.runs.push(probe);
    if expectation == Expectation::Observe {
        return Ok(row);
    }
    for (number, checks) in (1..).zip(SUITES) {
        let name = format!("{base}-{index}-{number}");
        let outcome = nested(&niri, checks, &name)?;
        row.suites.push((checks, outcome));
        row.runs.push(name);
    }
    Ok(row)
}

/// One nested run of `checks` on `niri`; a failed run is an outcome, an interrupt is not.
fn nested(
    niri: &std::path::Path,
    checks: ServerChecks,
    name: &str,
) -> Result<std::result::Result<(), String>> {
    let options = Options {
        scale: Scale::ONE,
        decorations: Decorations::Client,
        visible: false,
        noctalia: false,
        sitting: false,
        server: Some(checks),
        eval: None,
        demo: None,
        mode: ServerMode::Standalone,
        binary: None,
        niri: Some(niri.to_path_buf()),
        headless_outputs: None,
        browser: None,
        from: None,
    };
    let outcome = run::run_named(&options, name).map_err(|failure| failure.to_string());
    if interrupt::requested() {
        return Err(Failure::new("interrupted"));
    }
    Ok(outcome)
}

/// `REF COMMIT PATH` or `REF COMMIT failed: REASON` per line.
fn builds(text: &str) -> Result<Vec<Build>> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut parts = line.splitn(3, ' ');
            let (Some(reference), Some(commit), Some(rest)) =
                (parts.next(), parts.next(), parts.next())
            else {
                return Err(Failure::new(format!("not a build line: {line:?}")));
            };
            let niri = rest.strip_prefix("failed: ").map_or_else(
                || Ok(PathBuf::from(rest)),
                |reason| Err(reason.trim().to_owned()),
            );
            Ok(Build {
                reference: reference.to_owned(),
                commit: commit.to_owned(),
                niri,
            })
        })
        .collect()
}

/// A release tag such as `v26.04` or `v26.04.1` with the pin's major and minor is `Full`,
/// any other release tag `Observe`, and anything else, such as `main`, `Record`.
fn expectation(reference: &str) -> Expectation {
    let release = |version: &str| -> Option<Vec<u32>> {
        let numbers = version
            .split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<Vec<u32>>>()?;
        (2..=3).contains(&numbers.len()).then_some(numbers)
    };
    let (Some(tag), Some(pin)) = (
        reference.strip_prefix('v').and_then(release),
        release(IPC_CRATE),
    ) else {
        return Expectation::Record;
    };
    if tag.get(..2) == pin.get(..2) {
        Expectation::Full
    } else {
        Expectation::Observe
    }
}

/// How a row went against its expectation.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    AsExpected,
    Failed(String),
    Recorded,
    NotBuilt,
}

impl Verdict {
    fn cell(&self) -> String {
        match self {
            Self::AsExpected => "as expected".to_owned(),
            Self::Failed(why) => format!("**failed**: {why}"),
            Self::Recorded => "recorded".to_owned(),
            Self::NotBuilt => "not built".to_owned(),
        }
    }
}

fn verdict(row: &Row) -> Verdict {
    if row.build.niri.is_err() {
        return match row.expectation {
            Expectation::Full => Verdict::Failed("the supported release didn't build".to_owned()),
            Expectation::Observe | Expectation::Record => Verdict::NotBuilt,
        };
    }
    let compat = row
        .report
        .as_ref()
        .and_then(|report| field(report, "/compat").as_str());
    let wanted = match row.expectation {
        Expectation::Record => return Verdict::Recorded,
        Expectation::Full => ["ok", "patch_warning"].contains(&compat.unwrap_or_default()),
        Expectation::Observe => compat == Some("read_only"),
    };
    if !wanted {
        return Verdict::Failed(format!(
            "status said compat {}",
            compat.unwrap_or("nothing")
        ));
    }
    let stream = row
        .report
        .as_ref()
        .and_then(|report| field(report, "/event_stream").as_str());
    if stream != Some("connected") {
        return Verdict::Failed(format!(
            "status said the event stream was {}",
            stream.unwrap_or("nothing")
        ));
    }
    if row.probe.is_err() {
        return Verdict::Failed("the probe went otherwise".to_owned());
    }
    if row.suites.iter().any(|(_, outcome)| outcome.is_err()) {
        return Verdict::Failed("a suite failed".to_owned());
    }
    Verdict::AsExpected
}

fn render(rows: &[Row]) -> String {
    let mut text = format!(
        "# niri compatibility matrix\n\n\
         Written by `make niri-matrix`: this checkout's server, with `niri-ipc` {IPC_CRATE}, \
         on each niri built by `scripts/niri-build.sh`, nested in the headless cage. \
         `docs/development.md` says what each column checks.\n\n\
         | niri | Commit | Expected | `Version` reply | `compat` | Event stream | Observation | \
         Without the lease | `acquire_desktop` | Suites | Result |\n\
         |---|---|---|---|---|---|---|---|---|---|---|\n"
    );
    for row in rows {
        text.push_str(&table_row(row));
    }
    text.push_str("\n## What went otherwise\n");
    for row in rows {
        text.push_str(&details(row));
    }
    text.push_str("\n## Runs\n\nEach run's artifacts are in `target/e2e/<run>`.\n\n");
    text.extend(
        rows.iter()
            .filter(|row| !row.runs.is_empty())
            .map(|row| format!("- {}: {}\n", row.build.reference, row.runs.join(", "))),
    );
    text
}

fn table_row(row: &Row) -> String {
    let report = row.report.as_ref().unwrap_or(&Value::Null);
    let text = |pointer: &str| {
        field(report, pointer)
            .as_str()
            .map_or_else(|| "–".to_owned(), |text| format!("`{text}`"))
    };
    let suites = if row.suites.is_empty() {
        "–".to_owned()
    } else {
        let passed = row.suites.iter().filter(|(_, outcome)| outcome.is_ok());
        format!("{}/{} pass", passed.count(), row.suites.len())
    };
    format!(
        "| {} | {} | {:?} | {} | {} | {} | {} | {} | {} | {suites} | {} |\n",
        row.build.reference,
        row.build.commit.get(..7).unwrap_or(&row.build.commit),
        row.expectation,
        text("/version"),
        text("/compat"),
        text("/event_stream"),
        observed(report),
        refused(report),
        acquired(report),
        verdict(row).cell(),
    )
}

fn calls(report: &Value) -> impl Iterator<Item = &Value> {
    field(report, "/calls").as_array().into_iter().flatten()
}

/// How many of the observation tools answered.
fn observed(report: &Value) -> String {
    let answered = calls(report)
        .filter(|call| {
            compat::OBSERVING.contains(&field(call, "/tool").as_str().unwrap_or_default())
        })
        .filter(|call| field(call, "/got") == "ok")
        .count();
    if report.is_null() {
        "–".to_owned()
    } else {
        format!("{answered}/{} answer", compat::OBSERVING.len())
    }
}

/// How many of the tools that need the lease were refused for the lack of it.
fn refused(report: &Value) -> String {
    let needing: Vec<&Value> = calls(report)
        .filter(|call| field(call, "/expected") == "lease_required")
        .collect();
    if needing.is_empty() {
        return "–".to_owned();
    }
    let refused = needing
        .iter()
        .filter(|call| field(call, "/got") == "lease_required")
        .count();
    format!("{refused}/{} `lease_required`", needing.len())
}

fn acquired(report: &Value) -> String {
    calls(report)
        .find(|call| field(call, "/tool") == "acquire_desktop")
        .and_then(|call| field(call, "/got").as_str())
        .map_or_else(|| "–".to_owned(), |got| format!("`{got}`"))
}

/// The build's failure, the probe's misses and each failed suite, in full.
fn details(row: &Row) -> String {
    let mut lines = Vec::new();
    if let Err(reason) = &row.build.niri {
        lines.push(format!("build: {reason}"));
    }
    let misses = row.report.as_ref().map(compat::misses).unwrap_or_default();
    match (&row.probe, misses.is_empty()) {
        (Err(failure), true) if row.build.niri.is_ok() => lines.push(format!("probe: {failure}")),
        _ => lines.extend(misses.into_iter().map(|miss| format!("probe: {miss}"))),
    }
    for (checks, outcome) in &row.suites {
        if let Err(failure) = outcome {
            lines.push(format!("`{}`: {failure}", checks.flag()));
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    let mut text = format!("\n### {}\n\n", row.build.reference);
    text.extend(
        lines
            .iter()
            .map(|line| format!("- {}\n", line.replace('\n', " "))),
    );
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_pin_matches_both_manifests() {
        let pin = format!("niri-ipc = \"={IPC_CRATE}\"");
        assert!(include_str!("../../Cargo.toml").contains(&pin));
        assert!(include_str!("../Cargo.toml").contains(&pin));
    }

    #[test]
    fn releases_with_the_pins_major_and_minor_are_supported() {
        assert_eq!(expectation("v26.04"), Expectation::Full);
        assert_eq!(expectation("v26.04.1"), Expectation::Full);
        assert_eq!(expectation("v25.11"), Expectation::Observe);
        assert_eq!(expectation("v26.10"), Expectation::Observe);
        assert_eq!(expectation("v25.05.1"), Expectation::Observe);
        for other in [
            "main",
            "fc9e8c42",
            "v26",
            "v26.04-rc1",
            "26.04",
            "v26.04.1.2",
        ] {
            assert_eq!(expectation(other), Expectation::Record, "{other}");
        }
    }

    #[test]
    fn reads_built_and_failed_lines() {
        let text = "v26.04 8ed0da4 /c/bin/8ed0da4/niri\n\
                    v25.08 01be0e6 failed:   error: failed to run custom build command\n";
        let builds = builds(text).unwrap();
        assert_eq!(builds[0].niri, Ok(PathBuf::from("/c/bin/8ed0da4/niri")));
        assert_eq!(
            builds[1].niri,
            Err("error: failed to run custom build command".to_owned())
        );
        assert!(super::builds("v26.04 8ed0da4\n").is_err());
    }

    fn row(reference: &str, compat: &str, probe: bool, suites: &[bool]) -> Row {
        Row {
            build: Build {
                reference: reference.to_owned(),
                commit: "8ed0da44".to_owned(),
                niri: Ok(PathBuf::from("/b/niri")),
            },
            expectation: expectation(reference),
            report: Some(json!({"compat": compat, "event_stream": "connected", "calls": []})),
            probe: if probe { Ok(()) } else { Err("x".to_owned()) },
            suites: suites
                .iter()
                .map(|&passed| {
                    let outcome = if passed { Ok(()) } else { Err("y".to_owned()) };
                    (ServerChecks::Actions, outcome)
                })
                .collect(),
            runs: Vec::new(),
        }
    }

    #[test]
    fn the_supported_release_must_pass_everything() {
        let passing = row("v26.04", "ok", true, &[true, true]);
        assert_eq!(verdict(&passing), Verdict::AsExpected);
        assert!(matches!(
            verdict(&row("v26.04", "ok", true, &[true, false])),
            Verdict::Failed(_)
        ));
        assert!(matches!(
            verdict(&row("v26.04", "ok", false, &[true])),
            Verdict::Failed(_)
        ));
        assert!(matches!(
            verdict(&row("v26.04", "read_only", true, &[])),
            Verdict::Failed(_)
        ));
        let mut unbuilt = passing;
        unbuilt.build.niri = Err("no".to_owned());
        assert!(matches!(verdict(&unbuilt), Verdict::Failed(_)));
    }

    #[test]
    fn another_release_must_be_read_only_and_observable() {
        assert_eq!(
            verdict(&row("v25.11", "read_only", true, &[])),
            Verdict::AsExpected
        );
        assert!(matches!(
            verdict(&row("v25.11", "ok", true, &[])),
            Verdict::Failed(_)
        ));
        assert!(matches!(
            verdict(&row("v25.11", "read_only", false, &[])),
            Verdict::Failed(_)
        ));
        let mut stopped = row("v25.11", "read_only", true, &[]);
        stopped.report = Some(json!({
            "compat": "read_only", "event_stream": "schema_incompatible", "calls": []
        }));
        assert!(matches!(verdict(&stopped), Verdict::Failed(_)));
        let mut unbuilt = row("v25.08", "read_only", true, &[]);
        unbuilt.build.niri = Err("no".to_owned());
        assert_eq!(verdict(&unbuilt), Verdict::NotBuilt);
    }

    #[test]
    fn main_is_only_recorded() {
        assert_eq!(
            verdict(&row("main", "ok", false, &[false])),
            Verdict::Recorded
        );
    }
}
