//! Nested niri test harness for niri-computer-use. See `docs/development.md`.

mod a11y;
mod actions;
mod capture;
mod clipboard;
mod config;
mod control;
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
mod mcp;
mod nested;
mod niri;
mod noctalia;
mod run;
mod runner;
mod scale;
mod session;
mod shell;
mod sitting;
mod slow_reader;
mod snapshot;
mod supervise;
mod test_dir;
mod wev;
mod window;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use failure::{Failure, Result};
use scale::Scale;
use supervise::Probes;
use test_dir::TestDir;

const USAGE: &str = "usage: harness run [--visible] [--scale <scale>] [--ssd] [--noctalia | --sitting | --control | --actions | --input | --shell | --a11y | --eval <scenario> --skill <dir|none> --model <model>]
       harness host-capture <output>
       harness window <TEST_DIR> <app_id> [--count <n>] [--delay <ms>] [--late <ms>] [--keep-open] [--started <file>]
       harness keymaps <TEST_DIR> <directory> <deadline-ms>
       harness clipboard <TEST_DIR> <deadline-ms> [--secret]
       harness slow-reader <TEST_DIR> <delay-ms> <deadline-ms>
       harness supervise <TEST_DIR> <ARTIFACTS> <scale> [--ssd] [--noctalia <server> | --sitting | --control <server> | --actions <server> | --input <server> | --shell <server> | --a11y <server> | --eval <server> <scenario> <skill|none> <model>]";

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
                eval: None,
            };
            match rest {
                [] => {}
                ["--eval", server, scenario, skill, model] => {
                    probes.eval = Some((eval_options(scenario, skill, model)?, *server));
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
    };
    let (mut scenario, mut skill, mut model) = (None, None, None);
    let mut args = args.iter();
    while let Some(&arg) = args.next() {
        match arg {
            "--scale" => {
                options.scale = args.next().ok_or_else(|| Failure::new(USAGE))?.parse()?;
            }
            "--visible" => options.visible = true,
            "--ssd" => options.decorations = config::Decorations::Server,
            "--noctalia" => options.noctalia = true,
            "--sitting" => options.sitting = true,
            "--control" | "--actions" | "--input" | "--shell" | "--a11y" => {
                options.server = run::ServerChecks::from_flag(arg);
            }
            "--eval" => scenario = Some(*args.next().ok_or_else(|| Failure::new(USAGE))?),
            "--skill" => skill = Some(*args.next().ok_or_else(|| Failure::new(USAGE))?),
            "--model" => model = Some(*args.next().ok_or_else(|| Failure::new(USAGE))?),
            _ => return Err(Failure::new(USAGE)),
        }
    }
    options.eval = match (scenario, skill, model) {
        (None, None, None) => None,
        (Some(scenario), Some(skill), Some(model)) => Some(eval_options(scenario, skill, model)?),
        _ => return Err(Failure::new("--eval needs --skill and --model")),
    };
    if usize::from(options.sitting)
        + usize::from(options.noctalia)
        + usize::from(options.server.is_some())
        + usize::from(options.eval.is_some())
        > 1
    {
        return Err(Failure::new(
            "--sitting, --noctalia, --control, --actions, --input, --shell, --a11y and --eval cannot be combined",
        ));
    }
    options.visible |= options.sitting;
    Ok(options)
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
