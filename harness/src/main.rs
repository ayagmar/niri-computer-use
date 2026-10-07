//! Nested niri test harness for niri-desktop-mcp. See `docs/development.md`.

mod config;
mod environment;
mod failure;
mod interrupt;
mod log;
mod niri;
mod run;
mod runner;
mod scale;
mod snapshot;
mod supervise;
mod test_dir;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use failure::{Failure, Result};
use scale::Scale;
use test_dir::TestDir;

const USAGE: &str = "usage: harness run [--scale <scale>]
       harness supervise <TEST_DIR> <ARTIFACTS> <scale>";

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
        ["run"] => {
            interrupt::install()?;
            run::run(Scale::ONE)
        }
        ["run", "--scale", scale] => {
            let scale = scale.parse()?;
            interrupt::install()?;
            run::run(scale)
        }
        ["supervise", test_dir, artifacts, scale] => supervise::supervise(
            &TestDir::open(PathBuf::from(test_dir))?,
            Path::new(artifacts),
            scale.parse()?,
        ),
        _ => Err(Failure::new(USAGE)),
    }
}
