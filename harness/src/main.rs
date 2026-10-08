//! Nested niri test harness for niri-desktop-mcp. See `docs/development.md`.

mod capture;
mod config;
mod environment;
mod failure;
mod image;
mod interrupt;
mod keyboard;
mod log;
mod nested;
mod niri;
mod noctalia;
mod pointer;
mod run;
mod runner;
mod scale;
mod session;
mod sitting;
mod snapshot;
mod supervise;
mod test_dir;
mod wev;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use failure::{Failure, Result};
use scale::Scale;
use supervise::Probes;
use test_dir::TestDir;

const USAGE: &str = "usage: harness run [--scale <scale>] [--noctalia | --sitting | --sitting-from-c8]
       harness host-capture <output>
       harness supervise <TEST_DIR> <ARTIFACTS> <scale> <vpointer> [<noctalia-socket> | --sitting | --sitting-from-c8]";

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
            run::run(options)
        }
        ["host-capture", output] => {
            interrupt::install()?;
            capture::host(output)
        }
        [
            "supervise",
            test_dir,
            artifacts,
            scale,
            vpointer,
            noctalia @ ..,
        ] => {
            let sitting = match noctalia {
                ["--sitting"] => Some(sitting::Mode::Full),
                ["--sitting-from-c8"] => Some(sitting::Mode::FromC8),
                _ => None,
            };
            let noctalia = match noctalia {
                [] | ["--sitting" | "--sitting-from-c8"] => None,
                [probe] if !probe.starts_with("--") => Some(*probe),
                _ => return Err(Failure::new(USAGE)),
            };
            supervise::supervise(
                &TestDir::open(PathBuf::from(test_dir))?,
                Path::new(artifacts),
                scale.parse()?,
                &Probes {
                    vpointer,
                    noctalia,
                    sitting,
                },
            )
        }
        _ => Err(Failure::new(USAGE)),
    }
}

/// Explicit automatic or supervised mode, with an optional output scale.
fn run_options(args: &[&str]) -> Result<run::Options> {
    let mut options = run::Options {
        scale: Scale::ONE,
        noctalia: false,
        sitting: None,
    };
    let mut args = args.iter();
    while let Some(&arg) = args.next() {
        match arg {
            "--scale" => {
                options.scale = args.next().ok_or_else(|| Failure::new(USAGE))?.parse()?;
            }
            "--noctalia" => options.noctalia = true,
            "--sitting" => options.sitting = Some(sitting::Mode::Full),
            "--sitting-from-c8" => options.sitting = Some(sitting::Mode::FromC8),
            _ => return Err(Failure::new(USAGE)),
        }
    }
    if options.sitting.is_some() && options.noctalia {
        return Err(Failure::new("--sitting and --noctalia cannot be combined"));
    }
    Ok(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supervise_takes_at_most_one_noctalia_probe() {
        let args = ["supervise", "/r/t", "/a", "1", "vpointer", "one", "two"].map(OsString::from);
        assert_eq!(dispatch(&args).unwrap_err().to_string(), USAGE);
    }

    #[test]
    fn sitting_is_explicit_and_cannot_start_noctalia() {
        assert_eq!(run_options(&[]).unwrap().sitting, None);
        assert_eq!(
            run_options(&["--sitting", "--scale", "1.5"])
                .unwrap()
                .sitting,
            Some(sitting::Mode::Full)
        );
        assert_eq!(
            run_options(&["--sitting-from-c8"]).unwrap().sitting,
            Some(sitting::Mode::FromC8)
        );
        assert!(run_options(&["--sitting", "--noctalia"]).is_err());
        assert!(run_options(&["--sitting-from-c8", "--noctalia"]).is_err());
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
