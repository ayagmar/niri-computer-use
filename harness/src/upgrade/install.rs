//! The installs an upgrade check makes: a release from GitHub, then the build under test
//! from a release of its own, each through the repository's `install.sh` into a HOME under
//! `TEST_DIR`. Nothing reads or writes the user's HOME, `~/.local/bin` or runtime
//! directory: every command gets the throwaway HOME, and `TMPDIR` under `TEST_DIR`.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::failure::{Context as _, Failure, Result};
use crate::session::Session;

/// A download of a few megabytes from GitHub, and the binary's `--version`.
const INSTALL_DEADLINE: Duration = Duration::from_secs(120);
const PACK_DEADLINE: Duration = Duration::from_secs(30);
/// The version the build under test is released as, which names only its files.
pub(super) const CURRENT: &str = "current";

/// Where the upgrade check installs and what it installs from.
#[derive(Debug)]
pub(super) struct Place {
    /// The throwaway HOME.
    pub(super) home: PathBuf,
    tmp: PathBuf,
    /// The releases the build under test is installed from, as `install.sh` finds them.
    releases: PathBuf,
    /// The checkout: its `install.sh` and its skill.
    checkout: PathBuf,
    /// The build under test.
    server: PathBuf,
}

impl Place {
    /// Creates the HOME and the other directories under `TEST_DIR`.
    pub(super) fn create(session: &Session<'_>, server: &str) -> Result<Self> {
        let root = session.test_dir().root();
        let checkout = std::env::current_dir().context("read the working directory")?;
        if !checkout.join("install.sh").is_file() {
            return Err(Failure::new(format!(
                "{} has no install.sh; run the harness from the checkout",
                checkout.display()
            )));
        }
        let place = Self {
            home: root.join("home"),
            tmp: root.join("tmp"),
            releases: root.join("releases"),
            checkout,
            server: PathBuf::from(server),
        };
        for dir in [&place.home, &place.tmp, &place.releases] {
            fs::create_dir(dir).context(format!("create {}", dir.display()))?;
        }
        Ok(place)
    }

    /// The installed binary.
    pub(super) fn binary(&self) -> PathBuf {
        self.home.join(".local/bin/niri-computer-use")
    }

    /// `env` with the throwaway HOME and `TMPDIR`, then `args`: the program and its
    /// arguments for `Session::start` and `Client::start_command`.
    pub(super) fn env(&self, args: &[OsString]) -> Vec<OsString> {
        let mut home = OsString::from("HOME=");
        home.push(&self.home);
        let mut tmp = OsString::from("TMPDIR=");
        tmp.push(&self.tmp);
        let mut all = vec![home, tmp];
        all.extend_from_slice(args);
        all
    }

    /// Packs the build under test as the release workflow packs one, with its skill, and
    /// its `.sha256`, where `install.sh` looks for release `CURRENT`.
    pub(super) fn release_current(&self, session: &Session<'_>) -> Result<()> {
        let script = format!(
            "set -eu; name=niri-computer-use-{CURRENT}-$(uname -m)-linux; dir='{}/download/v{CURRENT}'; \
             mkdir -p \"$dir/$name/skills\"; cp '{}' \"$dir/$name/niri-computer-use\"; \
             cp -R '{}/skills/niri-computer-use' \"$dir/$name/skills/\"; \
             tar -C \"$dir\" -czf \"$dir/$name.tar.gz\" \"$name\"; rm -rf \"${{dir:?}}/$name\"; \
             cd \"$dir\" && sha256sum \"$name.tar.gz\" > \"$name.tar.gz.sha256\"",
            self.releases.display(),
            self.server.display(),
            self.checkout.display(),
        );
        let log = session.artifact("upgrade-pack.log");
        session
            .start("sh", &["-c".into(), script.into()], log, PACK_DEADLINE)?
            .wait()
            .map(drop)
    }

    /// Installs `version` with `install.sh`: from GitHub, or `CURRENT` from the release
    /// `release_current` made. Returns what the installer printed.
    pub(super) fn install(&self, session: &Session<'_>, version: &str) -> Result<String> {
        let mut vars = vec![
            format!("NCU_VERSION={version}"),
            format!("NCU_BIN_DIR={}", self.home.join(".local/bin").display()),
            "NCU_REGISTER=0".to_owned(),
        ];
        if version == CURRENT {
            vars.push(format!(
                "NCU_RELEASES_URL=file://{}",
                self.releases.display()
            ));
        }
        let mut args: Vec<OsString> = vars.into_iter().map(OsString::from).collect();
        args.push("sh".into());
        args.push(self.checkout.join("install.sh").into());
        let log = session.artifact(&format!("upgrade-install-{version}.log"));
        session
            .start("env", &self.env(&args), log.clone(), INSTALL_DEADLINE)?
            .wait()?;
        let printed = fs::read_to_string(&log).context(format!("read {}", log.display()))?;
        self.installed(&printed)?;
        Ok(printed)
    }

    /// Checks what an install left: the binary, and the skill linked from both agents'
    /// skill directories.
    fn installed(&self, printed: &str) -> Result<()> {
        let skill = self
            .home
            .join(".local/share/niri-computer-use/skills/niri-computer-use");
        if !printed.contains("Installed niri-computer-use") || !self.binary().is_file() {
            return Err(Failure::new(format!(
                "install.sh installed no binary: {printed}"
            )));
        }
        for agent in [".claude/skills", ".agents/skills"] {
            let link = self.home.join(agent).join("niri-computer-use");
            let target = fs::read_link(&link).context(format!("read {}", link.display()))?;
            if target != skill || !skill.join("SKILL.md").is_file() {
                return Err(Failure::new(format!(
                    "{} points to {}, not the installed skill",
                    link.display(),
                    target.display()
                )));
            }
        }
        Ok(())
    }

    /// Whether the installed binary and skill are the build under test's, byte for byte.
    pub(super) fn holds_current(&self) -> Result<bool> {
        let skill = |root: &Path| root.join("SKILL.md");
        let installed_skill = self
            .home
            .join(".local/share/niri-computer-use/skills/niri-computer-use");
        let source_skill = self.checkout.join("skills/niri-computer-use");
        Ok(same(&self.binary(), &self.server)?
            && same(&skill(&installed_skill), &skill(&source_skill))?)
    }
}

fn same(a: &Path, b: &Path) -> Result<bool> {
    let read = |path: &Path| fs::read(path).context(format!("read {}", path.display()));
    Ok(read(a)? == read(b)?)
}
