//! Nested niri test harness for niri-computer-use. See `docs/development.md`.

mod a11y;
mod actions;
mod capture;
mod changes;
mod chromium;
mod clipboard;
mod compat;
mod config;
mod control;
mod demo;
mod environment;
mod eval;
mod failure;
mod headless;
mod image;
#[path = "../../src/image_header.rs"]
mod image_header;
mod input;
mod interrupt;
mod journal;
mod keyboard;
mod keymaps;
mod log;
mod matrix;
mod mcp;
mod measure;
mod nested;
mod niri;
mod noctalia;
mod run;
mod runner;
mod scale;
mod session;
mod shared;
mod shell;
mod sitting;
mod slow_reader;
mod snapshot;
mod supervise;
mod test_dir;
mod upgrade;
mod wev;
mod window;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use failure::{Failure, Result};
use scale::Scale;
use supervise::Probes;
use test_dir::TestDir;

const USAGE: &str = "usage: harness run [--visible | --headless-niri] [--shared] [--server <path>] [--niri <path>] [--scale <scale>] [--ssd] [--noctalia | --sitting | --control | --actions | --input | --shell | --a11y | --engine | --measure | --latency | --chromium | --chromium-a11y | --big | --compat | --multi | --overlay | --upgrade <version> | --eval <scenario> --skill <dir|none> --model <model> | --demo <scene>]
       harness matrix <builds> <output>
       harness host-capture <output>
       harness window <TEST_DIR> <app_id> [--count <n>] [--delay <ms>] [--late <ms>] [--keep-open] [--started <file>]
       harness keymaps <TEST_DIR> <directory> <deadline-ms>
       harness clipboard <TEST_DIR> <deadline-ms> [--secret]
       harness slow-reader <TEST_DIR> <delay-ms> <deadline-ms>
       harness supervise <TEST_DIR> <ARTIFACTS> <scale> [--ssd] [--noctalia <server> | --sitting | --control <server> | --actions <server> | --input <server> | --shell <server> | --a11y <server> | --engine <server> | --measure <server> | --latency <server> | --chromium <server> <browser> | --chromium-a11y <server> <browser> | --big <server> | --compat <server> | --multi <server> | --overlay <server> | --upgrade <server> <version> | --eval <server> <scenario> <skill|none> <model> | --demo <server> <scene>]";

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(failure) => {
            log::echo(&format!("harness: {failure}"));
            ExitCode::FAILURE
        }
    }
}

fn dispatch(args: &[OsString]) -> Result<()> {
    let args = args
        .iter()
        .map(|arg| {
            arg.to_str()
                .ok_or_else(|| Failure::new("arguments must be UTF-8"))
        })
        .collect::<Result<Vec<&str>>>()?;
    match args.as_slice() {
        ["run", options @ ..] => {
            let options = run_options(options)?;
            interrupt::install()?;
            run::run(&options)
        }
        ["matrix", builds, output] => {
            interrupt::install()?;
            matrix::run(builds, output)
        }
        ["window", options @ ..] => window::run(&window::Options::parse(options)?),
        ["keymaps", options @ ..] => keymaps::run(options),
        ["clipboard", options @ ..] => clipboard::run(options),
        ["slow-reader", options @ ..] => slow_reader::run(options),
        ["host-capture", output] => {
            interrupt::install()?;
            capture::host(output)
        }
        ["supervise", test_dir, artifacts, scale, rest @ ..] => {
            let (decorations, rest) = match rest {
                [flag, rest @ ..] if *flag == config::Decorations::SERVER_FLAG => {
                    (config::Decorations::Server, rest)
                }
                _ => (config::Decorations::Client, rest),
            };
            let mut probes = Probes {
                decorations,
                noctalia: None,
                sitting: false,
                server: None,
                browser: None,
                from: None,
                eval: None,
                demo: None,
            };
            match rest {
                [] => {}
                ["--demo", server, scene] => probes.demo = Some((scene_option(scene)?, *server)),
                ["--eval", server, scenario, skill, model] => {
                    probes.eval = Some((eval_options(scenario, skill, model)?, *server));
                }
                [flag @ ("--chromium" | "--chromium-a11y"), server, browser] => {
                    let checks =
                        run::ServerChecks::from_flag(flag).ok_or_else(|| Failure::new(USAGE))?;
                    probes.server = Some((checks, *server));
                    probes.browser = Some(*browser);
                }
                ["--upgrade", server, from] => {
                    probes.server = Some((run::ServerChecks::Upgrade, *server));
                    probes.from = Some(*from);
                }
                ["--sitting"] => probes.sitting = true,
                ["--noctalia", server] => probes.noctalia = Some(*server),
                [flag, server] => {
                    let checks =
                        run::ServerChecks::from_flag(flag).ok_or_else(|| Failure::new(USAGE))?;
                    probes.server = Some((checks, *server));
                }
                _ => return Err(Failure::new(USAGE)),
            }
            supervise::supervise(
                &TestDir::open(PathBuf::from(test_dir))?,
                Path::new(artifacts),
                scale.parse()?,
                &probes,
            )
        }
        _ => Err(Failure::new(USAGE)),
    }
}

/// Explicit automatic or supervised mode, with an optional output scale.
fn run_options(args: &[&str]) -> Result<run::Options> {
    let mut options = run::Options {
        scale: Scale::ONE,
        decorations: config::Decorations::Client,
        visible: false,
        noctalia: false,
        sitting: false,
        server: None,
        eval: None,
        demo: None,
        mode: run::ServerMode::Standalone,
        binary: None,
        niri: None,
        headless_outputs: None,
        browser: None,
        from: None,
    };
    let (mut scenario, mut skill, mut model) = (None, None, None);
    let mut args = args.iter();
    while let Some(&arg) = args.next() {
        match arg {
            "--scale" => {
                options.scale = args.next().ok_or_else(|| Failure::new(USAGE))?.parse()?;
            }
            "--visible" => options.visible = true,
            "--headless-niri" => options.headless_outputs = Some(chromium::HEADLESS_OUTPUTS),
            "--shared" => options.mode = run::ServerMode::Shared,
            "--ssd" => options.decorations = config::Decorations::Server,
            "--noctalia" => options.noctalia = true,
            "--sitting" => options.sitting = true,
            "--server" => {
                let path = args.next().ok_or_else(|| Failure::new(USAGE))?;
                let path = std::fs::canonicalize(path)
                    .map_err(|error| Failure::new(format!("server {path}: {error}")))?;
                options.binary = Some(path);
            }
            "--niri" => {
                options.niri = Some(niri_option(
                    args.next().ok_or_else(|| Failure::new(USAGE))?,
                )?);
            }
            "--control" | "--actions" | "--input" | "--shell" | "--a11y" | "--measure"
            | "--latency" | "--chromium" | "--chromium-a11y" | "--big" | "--compat" | "--multi"
            | "--overlay" => {
                options.server = run::ServerChecks::from_flag(arg);
            }
            "--engine" => {
                options.server = Some(run::ServerChecks::Engine);
                options.mode = run::ServerMode::Shared;
            }
            "--upgrade" => {
                let from = args.next().ok_or_else(|| Failure::new(USAGE))?;
                options.server = Some(run::ServerChecks::Upgrade);
                options.from = Some((*from).to_owned());
                options.mode = run::ServerMode::Shared;
            }
            "--eval" => scenario = Some(*args.next().ok_or_else(|| Failure::new(USAGE))?),
            "--skill" => skill = Some(*args.next().ok_or_else(|| Failure::new(USAGE))?),
            "--model" => model = Some(*args.next().ok_or_else(|| Failure::new(USAGE))?),
            "--demo" => {
                options.demo = Some(scene_option(
                    args.next().ok_or_else(|| Failure::new(USAGE))?,
                )?);
            }
            _ => return Err(Failure::new(USAGE)),
        }
    }
    options.eval = match (scenario, skill, model) {
        (None, None, None) => None,
        (Some(scenario), Some(skill), Some(model)) => Some(eval_options(scenario, skill, model)?),
        _ => return Err(Failure::new("--eval needs --skill and --model")),
    };
    one_mode(&options)?;
    headless_outputs(&mut options)?;
    options.visible |= options.sitting;
    Ok(options)
}

/// `--multi` and `--overlay` always run on niri's headless backend, and `--headless-niri` asks for it
/// with `--chromium`, the one other suite run on it.
fn headless_outputs(options: &mut run::Options) -> Result<()> {
    if matches!(
        options.server,
        Some(run::ServerChecks::Multi | run::ServerChecks::Overlay)
    ) {
        if options.headless_outputs.is_some() {
            return Err(Failure::new(
                "--multi and --overlay already run niri's headless backend; drop --headless-niri",
            ));
        }
        options.headless_outputs = Some(input::multi::HEADLESS_OUTPUTS);
    } else if options.headless_outputs.is_some()
        && options.server != Some(run::ServerChecks::Chromium)
    {
        return Err(Failure::new("--headless-niri runs only with --chromium"));
    }
    Ok(())
}

/// At most one of the run's modes: a sitting, Noctalia's checks, the server's, an eval or
/// a demo.
fn one_mode(options: &run::Options) -> Result<()> {
    if usize::from(options.sitting)
        + usize::from(options.noctalia)
        + usize::from(options.server.is_some())
        + usize::from(options.eval.is_some())
        + usize::from(options.demo.is_some())
        > 1
    {
        return Err(Failure::new(
            "--sitting, --noctalia, --control, --actions, --input, --shell, --a11y, --engine, --measure, --latency, --chromium, --chromium-a11y, --big, --compat, --multi, --overlay, --upgrade, --eval and --demo cannot be combined",
        ));
    }
    Ok(())
}

/// An existing niri binary, made absolute. It must be named `niri`: the server finds the
/// nested niri by its process name.
fn niri_option(path: &str) -> Result<PathBuf> {
    let absolute = std::fs::canonicalize(path)
        .map_err(|error| Failure::new(format!("--niri {path}: {error}")))?;
    if absolute.file_name() != Some("niri".as_ref()) {
        return Err(Failure::new(format!(
            "--niri {}: the binary must be named niri, which the server looks for",
            absolute.display()
        )));
    }
    Ok(absolute)
}

/// A known demo scene.
fn scene_option(name: &str) -> Result<demo::Scene> {
    demo::Scene::from_name(name).ok_or_else(|| {
        let names: Vec<_> = demo::Scene::ALL.iter().map(|s| s.name()).collect();
        Failure::new(format!(
            "unknown demo scene {name:?}; one of {}",
            names.join(", ")
        ))
    })
}

/// A known scenario, an existing skill directory (made absolute) or `none`, and a model.
fn eval_options(scenario: &str, skill: &str, model: &str) -> Result<eval::Options> {
    let scenario = eval::Scenario::from_name(scenario).ok_or_else(|| {
        let names: Vec<_> = eval::Scenario::ALL.iter().map(|s| s.name()).collect();
        Failure::new(format!(
            "unknown scenario {scenario:?}; one of {}",
            names.join(", ")
        ))
    })?;
    let skill = match skill {
        "none" => None,
        path => Some(
            std::fs::canonicalize(path)
                .map_err(|error| Failure::new(format!("skill directory {path}: {error}")))?,
        ),
    };
    Ok(eval::Options {
        scenario,
        skill,
        model: model.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_runs_are_headless_unless_visibility_is_explicit() {
        for args in [
            &[][..],
            &["--input"],
            &["--actions"],
            &["--shell"],
            &["--control"],
        ] {
            assert!(!run_options(args).unwrap().visible);
        }
        assert!(run_options(&["--visible", "--input"]).unwrap().visible);
        assert!(run_options(&["--sitting"]).unwrap().visible);
    }

    #[test]
    fn only_multi_and_chromium_run_on_niris_headless_backend() {
        let multi = run_options(&["--multi"]).unwrap();
        assert_eq!(multi.headless_outputs, Some(input::multi::HEADLESS_OUTPUTS));
        let chromium = run_options(&["--headless-niri", "--chromium"]).unwrap();
        assert_eq!(chromium.headless_outputs, Some(chromium::HEADLESS_OUTPUTS));
        assert_eq!(run_options(&["--chromium"]).unwrap().headless_outputs, None);
        for refused in [
            &["--headless-niri"][..],
            &["--headless-niri", "--input"],
            &["--headless-niri", "--multi"],
        ] {
            assert!(run_options(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn supervise_takes_one_flag_and_the_server() {
        for rest in [
            &["one", "two"][..],
            &["--noctalia"],
            &["--noctalia", "/s", "x"],
            &["/s"],
        ] {
            let args = ["supervise", "/r/t", "/a", "1"]
                .iter()
                .chain(rest)
                .map(OsString::from)
                .collect::<Vec<_>>();
            assert_eq!(dispatch(&args).unwrap_err().to_string(), USAGE, "{rest:?}");
        }
    }

    #[test]
    fn sitting_is_explicit_and_cannot_start_noctalia() {
        assert!(!run_options(&[]).unwrap().sitting);
        assert!(
            run_options(&["--sitting", "--scale", "1.5"])
                .unwrap()
                .sitting
        );
        assert!(run_options(&["--sitting", "--noctalia"]).is_err());
        assert!(run_options(&["--sitting-from-c8"]).is_err());
    }

    #[test]
    fn niri_must_exist_and_be_named_niri() {
        let dir = std::env::temp_dir().join(format!("harness-niri-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/niri"), "").unwrap();
        std::fs::write(dir.join("niri-26.04"), "").unwrap();
        let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
        let chosen = run_options(&["--niri", &path("bin/niri"), "--compat"]).unwrap();
        assert_eq!(
            chosen.niri,
            Some(std::fs::canonicalize(dir.join("bin/niri")).unwrap())
        );
        let renamed = run_options(&["--niri", &path("niri-26.04")]).unwrap_err();
        assert!(
            renamed.to_string().contains("must be named niri"),
            "{renamed}"
        );
        assert!(run_options(&["--niri", &path("bin/none")]).is_err());
        assert!(run_options(&["--niri"]).is_err());
        assert_eq!(run_options(&[]).unwrap().niri, None);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_demo_names_a_known_scene_and_runs_alone() {
        let demo = run_options(&["--demo", "errand"]).unwrap();
        assert_eq!(demo.demo, Some(demo::Scene::Errand));
        assert!(!demo.visible);
        let unknown = run_options(&["--demo", "hero"]).unwrap_err().to_string();
        assert!(
            unknown.contains("one of errand, layout, keyboard, stop"),
            "{unknown}"
        );
        assert!(run_options(&["--demo"]).is_err());
        assert!(run_options(&["--demo", "errand", "--input"]).is_err());
        assert!(run_options(&["--demo", "errand", "--noctalia"]).is_err());
    }

    #[test]
    fn run_takes_a_scale_and_an_optional_noctalia_stage() {
        let default = run_options(&[]).unwrap();
        assert_eq!(
            (default.scale.to_string(), default.noctalia),
            ("1".to_owned(), false)
        );
        let both = run_options(&["--noctalia", "--scale", "1.5"]).unwrap();
        assert_eq!(
            (both.scale.to_string(), both.noctalia),
            ("1.5".to_owned(), true)
        );
        for bad in [
            &["--scale"][..],
            &["--scale", "0"],
            &["--noctalia=1"],
            &["1.5"],
        ] {
            assert!(run_options(bad).is_err(), "{bad:?}");
        }
    }
}
