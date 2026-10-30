//! Whether the user's niri config binds a key that runs `niri-computer-use stop`: a
//! best-effort read of the config file the running niri was started with, found from its
//! command line and environment as niri 26.04 finds it (`config_path` in `src/main.rs`),
//! and of the files it `include`s. niri's IPC lists no binds, so the files are read.
//!
//! Anything that keeps the answer from being sure is `unknown`, never `not_found`: niri out
//! of reach, a file that can't be read or that niri would refuse, and niri reporting that
//! its last config load failed, after which it runs an earlier config or its default.

mod binds;
mod kdl;

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use binds::{Executable, Runs, Spawner};

use crate::control::procs;
use crate::niri::{self, Socket};

/// For reading niri's process and its config files.
const DEADLINE: Duration = Duration::from_secs(2);
/// Far above any config file; a larger one isn't read.
const MAX_FILE: u64 = 1 << 20;
/// niri's own limit on nested includes (`RECURSION_LIMIT` in `niri-config/src/lib.rs`):
/// a file this many includes below the main one is refused.
const INCLUDE_DEPTH: usize = 10;
const PROGRAM: &str = "niri-computer-use";
/// niri's config when the user has none of their own.
const SYSTEM_CONFIG: &str = "/etc/niri/config.kdl";

/// Whether a key runs `niri-computer-use stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StopKey {
    Bound,
    NotFound,
    Unknown,
}

/// The answer and what it rests on: the file with the bind, the files read, or why there
/// is no sure answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Checked {
    pub(crate) state: StopKey,
    pub(crate) detail: String,
}

impl Checked {
    /// The answer when niri's config wasn't read.
    pub(crate) fn unread() -> Self {
        Self::unknown("niri's config wasn't read for this report")
    }

    fn unknown(detail: impl Into<String>) -> Self {
        Self {
            state: StopKey::Unknown,
            detail: detail.into(),
        }
    }
}

/// Reads the config of the niri on `socket`. `config_failed` is what niri last said of its
/// config load, when that is known.
pub(crate) async fn check(socket: &Socket, config_failed: Option<bool>) -> Checked {
    if config_failed == Some(true) {
        return Checked::unknown(
            "niri reports that its last config load failed, so it runs an earlier config or its default; `niri validate` says why",
        );
    }
    let pid = match niri::pid(socket).await {
        Ok(pid) => pid,
        Err(error) => return Checked::unknown(format!("find niri's process: {}", error.detail)),
    };
    // A file on a stuck filesystem blocks its reader, so the reads run on a thread of
    // their own, abandoned at the deadline.
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let spawned = std::thread::Builder::new()
        .name("stop key".to_owned())
        .spawn(move || {
            sender
                .send(inspect(Path::new("/proc"), pid, Path::new(SYSTEM_CONFIG)))
                .ok();
        });
    if let Err(error) = spawned {
        return Checked::unknown(format!("start a thread to read niri's config: {error}"));
    }
    match tokio::time::timeout(DEADLINE, receiver).await {
        Ok(Ok(checked)) => checked,
        Ok(Err(_)) => Checked::unknown("the thread reading niri's config ended without an answer"),
        Err(_) => Checked::unknown(format!(
            "reading niri's config took longer than {DEADLINE:?}"
        )),
    }
}

/// The bind to add inside `binds`, for the binary at `program`.
pub(crate) fn bind_line(program: &Path) -> String {
    format!(
        "Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true hotkey-overlay-title=\"Stop the AI agent (niri-computer-use)\" {{ spawn \"{}\" \"stop\"; }}",
        program.display()
    )
}

/// What `acquire_desktop` adds while niri's config binds no stop key, with the bind for
/// the binary at `program`.
pub(crate) fn unbound_warning(program: &Path) -> String {
    format!(
        "The user has no stop key: niri's config binds no key to `{PROGRAM} stop`. Tell the user, and suggest this line inside `binds` in their niri config: {} The Noctalia plugin's Stop also stops an agent.",
        bind_line(program)
    )
}

/// This binary's path, as a bind should name it. A binary replaced since it started shows
/// as `<path> (deleted)`, and the new one is at `<path>`.
pub(crate) fn own_program() -> PathBuf {
    let Ok(exe) = std::env::current_exe() else {
        return PathBuf::from(PROGRAM);
    };
    exe.to_str()
        .and_then(|exe| exe.strip_suffix(" (deleted)"))
        .map_or_else(|| exe.clone(), PathBuf::from)
}

/// Reads niri `pid`'s start under `proc_root`, then its config, with `system` as the
/// config niri falls back to.
fn inspect(proc_root: &Path, pid: u32, system: &Path) -> Checked {
    let started = match Started::read(proc_root, pid) {
        Ok(started) => started,
        Err(error) => return Checked::unknown(format!("read niri's process (PID {pid}): {error}")),
    };
    let main = match locate(&started, system) {
        Ok(main) => main,
        Err(error) => return Checked::unknown(error),
    };
    let spawner = Spawner {
        home: started.home.as_deref().map(Path::new),
        path: started.path.as_deref(),
        cwd: started.cwd.as_deref(),
    };
    match scan(&main, spawner, &mut Host) {
        Ok(scan) => scan.checked(&main),
        Err(error) => Checked::unknown(format!("{error}, so niri wouldn't load it as it is")),
    }
}

/// What niri was started with that says where its config is.
#[derive(Debug, Default)]
struct Started {
    /// The arguments after the program's name.
    args: Vec<String>,
    niri_config: Option<String>,
    xdg_config_home: Option<String>,
    home: Option<String>,
    /// Where `spawn` looks for a bare program name.
    path: Option<String>,
    cwd: Option<PathBuf>,
}

impl Started {
    fn read(proc_root: &Path, pid: u32) -> io::Result<Self> {
        let var = |name| {
            procs::environ_var(proc_root, pid, name)
                .map(|value| value.filter(|value| !value.is_empty()))
        };
        Ok(Self {
            args: procs::args(proc_root, pid)?,
            niri_config: var("NIRI_CONFIG")?,
            xdg_config_home: var("XDG_CONFIG_HOME")?,
            home: var("HOME")?,
            path: var("PATH")?,
            cwd: std::fs::read_link(proc_root.join(pid.to_string()).join("cwd")).ok(),
        })
    }

    /// `-c`/`--config`'s value, before any `--` that starts niri's own command.
    fn config_arg(&self) -> Option<&str> {
        let mut args = self.args.iter().take_while(|arg| *arg != "--");
        while let Some(arg) = args.next() {
            if arg == "-c" || arg == "--config" {
                return args.next().map(String::as_str);
            }
            if let Some(path) = arg
                .strip_prefix("--config=")
                .or_else(|| arg.strip_prefix("-c"))
            {
                return Some(path);
            }
        }
        None
    }
}

/// The config file niri loaded: the one its command line or `NIRI_CONFIG` names, else
/// `$XDG_CONFIG_HOME/niri/config.kdl` (or `~/.config/niri/config.kdl`) when it exists, else
/// `system`.
fn locate(started: &Started, system: &Path) -> Result<PathBuf, String> {
    if let Some(explicit) = started.config_arg().or(started.niri_config.as_deref()) {
        let path = Path::new(explicit);
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }
        return started
            .cwd
            .as_ref()
            .map(|cwd| cwd.join(path))
            .ok_or_else(|| {
                format!("niri's config {explicit} is relative to a directory that can't be read")
            });
    }
    let config_home = started
        .xdg_config_home
        .as_deref()
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .or_else(|| Some(Path::new(started.home.as_deref()?).join(".config")))
        .ok_or("niri has neither XDG_CONFIG_HOME nor HOME, so its config is unknown")?;
    let user = config_home.join("niri").join("config.kdl");
    for path in [user.as_path(), system] {
        match path.try_exists() {
            Ok(true) => return Ok(path.to_path_buf()),
            Ok(false) => {}
            Err(error) => return Err(format!("check {}: {error}", path.display())),
        }
    }
    Err(format!(
        "neither {} nor {} exists, so niri runs a config this can't find",
        user.display(),
        system.display()
    ))
}

/// The config files, read through `read`, and the programs they name, looked at through
/// `executable`.
trait Disk: Executable {
    fn read(&mut self, path: &Path) -> io::Result<String>;
}

/// This machine's files.
struct Host;

impl Executable for Host {
    fn executable(&self, path: &Path) -> io::Result<bool> {
        use std::os::unix::fs::PermissionsExt as _;
        match std::fs::metadata(path) {
            Ok(meta) => Ok(meta.is_file() && meta.permissions().mode() & 0o111 != 0),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

impl Disk for Host {
    fn read(&mut self, path: &Path) -> io::Result<String> {
        read_limited(path)
    }
}

/// A bind in effect, or one a later bind for its key replaced.
#[derive(Debug, PartialEq, Eq)]
struct Bind {
    /// The key as niri compares keys (`binds::key`).
    key: String,
    /// The key as the config writes it.
    shown: String,
    file: PathBuf,
    runs: Runs,
}

impl Bind {
    fn place(&self) -> String {
        format!("{}: {}", self.file.display(), self.shown)
    }

    /// What this bind alone says of the stop key, with its rank: the lowest wins.
    fn verdict(&self) -> Option<(u8, Checked)> {
        let place = self.place();
        let (rank, state, detail) = match &self.runs {
            Runs::Stop(program) => (
                0,
                StopKey::Bound,
                format!("{place} runs `{} stop`", program.display()),
            ),
            Runs::Unsure(why) => (
                1,
                StopKey::Unknown,
                format!("{place} runs `{PROGRAM} stop`, but {why}"),
            ),
            Runs::Unread => (
                2,
                StopKey::Unknown,
                format!("{place} runs {PROGRAM} in a form this can't read"),
            ),
            Runs::Missing(program) => (
                3,
                StopKey::NotFound,
                format!(
                    "{place} runs `{} stop`, which isn't an executable file",
                    program.display()
                ),
            ),
            Runs::Other => return None,
        };
        Some((rank, Checked { state, detail }))
    }
}

/// What the files hold: the binds niri ends up with, as it merges `binds` sections in
/// document order with a later bind for a key replacing an earlier one; the first stop
/// bind replaced that way and the file that replaced it; and how many files were read.
#[derive(Debug, PartialEq, Eq)]
struct Scan {
    binds: Vec<Bind>,
    replaced: Option<(Bind, PathBuf)>,
    files: usize,
}

impl Scan {
    /// The answer: a bind in effect that starts the stop with a program that is there
    /// makes it `bound`; else one that can't be told for sure makes it `unknown`.
    fn checked(self, main: &Path) -> Checked {
        let said = self
            .binds
            .iter()
            .filter_map(Bind::verdict)
            .min_by_key(|(rank, _)| *rank);
        if let Some((_, checked)) = said {
            return checked;
        }
        Checked {
            state: StopKey::NotFound,
            detail: self.not_found(main),
        }
    }

    /// Why no bind in effect runs the stop.
    fn not_found(&self, main: &Path) -> String {
        if let Some((bind, by)) = &self.replaced {
            return format!(
                "{} runs `{PROGRAM} stop`, but {} binds the same key later, which replaces it",
                bind.place(),
                by.display()
            );
        }
        let included = match self.files - 1 {
            0 => String::new(),
            1 => " or the file it includes".to_owned(),
            more => format!(" or the {more} files it includes"),
        };
        format!(
            "no bind runs `{PROGRAM} stop` in {}{included}",
            main.display()
        )
    }
}

/// Reads `main` and what it includes from `disk`, with `spawner` saying how niri finds a
/// program. An error is anything niri would refuse the config for, or a file not read.
fn scan(main: &Path, spawner: Spawner<'_>, disk: &mut impl Disk) -> Result<Scan, String> {
    let text = disk
        .read(main)
        .map_err(|error| format!("read {}: {error}", main.display()))?;
    let mut walk = Walk {
        spawner,
        disk,
        stack: vec![main.to_path_buf()],
        scan: Scan {
            binds: Vec::new(),
            replaced: None,
            files: 1,
        },
    };
    walk.file(main, &text)?;
    Ok(walk.scan)
}

struct Walk<'a, D> {
    spawner: Spawner<'a>,
    disk: &'a mut D,
    /// The file being read and those that include it, outermost first.
    stack: Vec<PathBuf>,
    scan: Scan,
}

impl<D: Disk> Walk<'_, D> {
    fn file(&mut self, path: &Path, text: &str) -> Result<(), String> {
        let nodes = kdl::parse(text).map_err(|error| format!("{}: {error}", path.display()))?;
        for node in &nodes {
            match node.name.as_str() {
                "include" => self.include(path, node)?,
                "binds" => self.binds(path, node)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// Merges a `binds` section as niri does (`Config::parse` in `niri-config`): a key
    /// bound twice in one section is refused, and the section's binds replace earlier ones
    /// for the same keys.
    fn binds(&mut self, path: &Path, section: &kdl::Node) -> Result<(), String> {
        let mut binds: Vec<Bind> = Vec::new();
        for bind in &section.children {
            let key =
                binds::key(&bind.name).map_err(|error| format!("{}: {error}", path.display()))?;
            if binds.iter().any(|earlier| earlier.key == key) {
                return Err(format!("{}: {} is bound twice", path.display(), bind.name));
            }
            let runs = bind.children.first().map_or(Runs::Other, |action| {
                binds::runs(action, self.spawner, &*self.disk)
            });
            binds.push(Bind {
                key,
                shown: bind.name.clone(),
                file: path.to_path_buf(),
                runs,
            });
        }
        let (replaced, kept): (Vec<Bind>, Vec<Bind>) = std::mem::take(&mut self.scan.binds)
            .into_iter()
            .partition(|earlier| binds.iter().any(|bind| bind.key == earlier.key));
        if self.scan.replaced.is_none() {
            self.scan.replaced = replaced
                .into_iter()
                .find(|bind| matches!(bind.runs, Runs::Stop(_)))
                .map(|bind| (bind, path.to_path_buf()));
        }
        self.scan.binds = kept;
        self.scan.binds.extend(binds);
        Ok(())
    }

    fn include(&mut self, from: &Path, node: &kdl::Node) -> Result<(), String> {
        let [target] = node.args.as_slice() else {
            return Err(format!(
                "{}: an include names no single file",
                from.display()
            ));
        };
        if self.stack.len() >= INCLUDE_DEPTH {
            return Err(format!(
                "{}: includes can't be {INCLUDE_DEPTH} levels deep in niri",
                from.display()
            ));
        }
        let path = self.resolve(from, target)?;
        if self.stack.contains(&path) {
            return Err(format!("{} includes itself", path.display()));
        }
        let text = match self.disk.read(&path) {
            Ok(text) => text,
            Err(error)
                if error.kind() == io::ErrorKind::NotFound
                    && node.prop("optional") == Some("true") =>
            {
                return Ok(());
            }
            Err(error) => return Err(format!("read {}: {error}", path.display())),
        };
        self.scan.files += 1;
        self.stack.push(path.clone());
        let read = self.file(&path, &text);
        self.stack.pop();
        read
    }

    /// An include's path: `~/` under niri's home, absolute, or relative to the file that
    /// includes it.
    fn resolve(&self, from: &Path, target: &str) -> Result<PathBuf, String> {
        if let Some(rest) = target.strip_prefix("~/") {
            let home = self
                .spawner
                .home
                .ok_or_else(|| format!("{}: niri has no HOME for {target}", from.display()))?;
            return Ok(home.join(rest));
        }
        let dir = from.parent().unwrap_or_else(|| Path::new("/"));
        Ok(dir.join(target))
    }
}

/// A file's text, unless it is larger than `MAX_FILE` or isn't UTF-8.
fn read_limited(path: &Path) -> io::Result<String> {
    use std::io::Read as _;
    let mut text = String::new();
    let file = std::fs::File::open(path)?;
    file.take(MAX_FILE + 1).read_to_string(&mut text)?;
    if u64::try_from(text.len()).unwrap_or(u64::MAX) > MAX_FILE {
        return Err(io::Error::other(format!("larger than {MAX_FILE} bytes")));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const STOP: &str = r#"Mod+Shift+Escape allow-when-locked=true { spawn "/home/u/.local/bin/niri-computer-use" "stop"; }"#;

    /// Config files by path, and one installed program.
    struct Files(BTreeMap<PathBuf, String>);

    impl Executable for Files {
        fn executable(&self, path: &Path) -> io::Result<bool> {
            Ok(path == Path::new("/home/u/.local/bin/niri-computer-use"))
        }
    }

    impl Disk for Files {
        fn read(&mut self, path: &Path) -> io::Result<String> {
            self.0
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    fn scanned(main: &str, all: &[(&str, &str)]) -> Checked {
        let main = Path::new(main);
        let mut files = Files(
            all.iter()
                .map(|(path, text)| (PathBuf::from(path), (*text).to_owned()))
                .collect(),
        );
        let spawner = Spawner {
            home: Some(Path::new("/home/u")),
            path: Some("/usr/bin:/home/u/.local/bin"),
            cwd: None,
        };
        match scan(main, spawner, &mut files) {
            Ok(scan) => scan.checked(main),
            Err(error) => Checked::unknown(error),
        }
    }

    fn state(main: &str) -> StopKey {
        scanned("/c/config.kdl", &[("/c/config.kdl", main)]).state
    }

    #[test]
    fn finds_the_bind_however_it_names_the_program() {
        for bind in [
            STOP.to_owned(),
            r#"Mod+Shift+Escape { spawn "niri-computer-use" "stop"; }"#.to_owned(),
            r#"Mod+Shift+Escape { spawn-sh "niri-computer-use stop"; }"#.to_owned(),
            r#"Mod+Shift+Escape { spawn-sh "~/.local/bin/niri-computer-use stop && notify-send stopped"; }"#.to_owned(),
            r#"Mod+Shift+Escape { spawn-sh "\"$HOME/.local/bin/niri-computer-use\" stop"; }"#.to_owned(),
        ] {
            assert_eq!(state(&format!("binds {{\n    {bind}\n}}")), StopKey::Bound, "{bind}");
        }
    }

    #[test]
    fn a_bind_commented_out_or_running_something_else_is_not_found() {
        for main in [
            format!("binds {{\n    // {STOP}\n}}"),
            format!("binds {{\n    /-{STOP}\n}}"),
            format!("/*\nbinds {{\n    {STOP}\n}}\n*/"),
            format!("/-binds {{\n    {STOP}\n}}"),
            r#"binds { Mod+Shift+Escape { spawn "niri-computer-use" "resume"; } }"#.to_owned(),
            r#"binds { Mod+Shift+Escape { spawn "niri-computer-use-helper" "stop"; } }"#.to_owned(),
            // A spawn outside `binds` is a startup command, not a key.
            r#"spawn-at-startup "niri-computer-use" "stop""#.to_owned(),
            String::new(),
        ] {
            assert_eq!(state(&main), StopKey::NotFound, "{main}");
        }
    }

    #[test]
    fn follows_includes_as_niri_resolves_them() {
        let main = r#"
include "binds.kdl"
include optional=true "missing.kdl"
include "~/extra/more.kdl"
"#;
        let binds = "include \"/abs/stop.kdl\"\nbinds { Mod+T { spawn \"foot\"; } }";
        let found = scanned(
            "/c/config.kdl",
            &[
                ("/c/config.kdl", main),
                ("/c/binds.kdl", binds),
                ("/abs/stop.kdl", &format!("binds {{ {STOP} }}")),
                ("/home/u/extra/more.kdl", "layout { gaps 8; }"),
            ],
        );
        assert_eq!(
            found,
            Checked {
                state: StopKey::Bound,
                detail: "/abs/stop.kdl: Mod+Shift+Escape runs `/home/u/.local/bin/niri-computer-use stop`"
                    .to_owned()
            }
        );
        let none = scanned(
            "/c/config.kdl",
            &[
                ("/c/config.kdl", "include \"a.kdl\"\ninclude \"b.kdl\""),
                ("/c/a.kdl", ""),
                ("/c/b.kdl", ""),
            ],
        );
        assert_eq!(
            none,
            Checked {
                state: StopKey::NotFound,
                detail: "no bind runs `niri-computer-use stop` in /c/config.kdl or the 2 files it includes"
                    .to_owned()
            }
        );
    }

    // niri refuses each of these configs and runs an earlier one or its default, so a bind
    // in them says nothing about the keys in use.
    #[test]
    fn a_config_niri_would_refuse_is_unknown_even_with_the_bind() {
        let bound = format!("binds {{ {STOP} }}");
        let cases: [(&str, Vec<(&str, String)>); 5] = [
            (
                "cycle",
                vec![
                    ("/c/config.kdl", format!("{bound}\ninclude \"a.kdl\"")),
                    ("/c/a.kdl", "include \"config.kdl\"".to_owned()),
                ],
            ),
            (
                "missing include",
                vec![("/c/config.kdl", format!("{bound}\ninclude \"gone.kdl\""))],
            ),
            (
                "unbalanced",
                vec![("/c/config.kdl", format!("{bound}\nlayout {{"))],
            ),
            (
                "bad include",
                vec![
                    ("/c/config.kdl", format!("{bound}\ninclude \"a.kdl\"")),
                    ("/c/a.kdl", "window-rule { match title=\"open".to_owned()),
                ],
            ),
            ("no main", vec![]),
        ];
        for (name, all) in cases {
            let all: Vec<(&str, &str)> = all
                .iter()
                .map(|(path, text)| (*path, text.as_str()))
                .collect();
            let checked = scanned("/c/config.kdl", &all);
            assert_eq!(checked.state, StopKey::Unknown, "{name}: {checked:?}");
        }
    }

    // niri reads a file nine includes below the main one and refuses one ten below.
    #[test]
    fn includes_nest_as_deep_as_niri_allows_and_no_deeper() {
        let chain = |files: usize| -> Checked {
            let mut all: Vec<(String, String)> = (1..files)
                .map(|n| (format!("/c/{}.kdl", n - 1), format!("include \"{n}.kdl\"")))
                .collect();
            all.push((
                format!("/c/{}.kdl", files - 1),
                format!("binds {{ {STOP} }}"),
            ));
            let all: Vec<(&str, &str)> = all
                .iter()
                .map(|(path, text)| (path.as_str(), text.as_str()))
                .collect();
            scanned("/c/0.kdl", &all)
        };
        assert_eq!(chain(INCLUDE_DEPTH).state, StopKey::Bound);
        let deeper = chain(INCLUDE_DEPTH + 1);
        assert_eq!(deeper.state, StopKey::Unknown);
        assert!(deeper.detail.contains("levels deep"), "{deeper:?}");
    }

    // niri merges `binds` sections in document order, includes where they stand, and a
    // later bind for a key replaces the earlier one.
    #[test]
    fn the_last_bind_for_a_key_is_the_one_that_counts() {
        let rebound = scanned(
            "/c/config.kdl",
            &[
                (
                    "/c/config.kdl",
                    &format!("binds {{ {STOP} }}\ninclude \"dms.kdl\""),
                ),
                ("/c/dms.kdl", "binds { shift+MOD+escape { close-window; } }"),
            ],
        );
        assert_eq!(rebound.state, StopKey::NotFound);
        assert!(
            rebound
                .detail
                .ends_with("but /c/dms.kdl binds the same key later, which replaces it"),
            "{rebound:?}"
        );
        let restored = scanned(
            "/c/config.kdl",
            &[
                (
                    "/c/config.kdl",
                    &format!("include \"dms.kdl\"\nbinds {{ {STOP} }}"),
                ),
                ("/c/dms.kdl", "binds { Mod+Shift+Escape { close-window; } }"),
            ],
        );
        assert_eq!(restored.state, StopKey::Bound);
        // Another key with the same action leaves the stop key alone.
        let other_key = scanned(
            "/c/config.kdl",
            &[(
                "/c/config.kdl",
                &format!("binds {{ {STOP} }}\nbinds {{ Mod+Escape {{ close-window; }} }}"),
            )],
        );
        assert_eq!(other_key.state, StopKey::Bound);
        // niri refuses a key bound twice in one section.
        let twice = format!("binds {{\n{STOP}\nMod+Shift+Escape {{ close-window; }}\n}}");
        assert_eq!(state(&twice), StopKey::Unknown);
    }

    #[test]
    fn a_stop_bind_whose_program_isnt_there_is_said_so() {
        let stale = state_of(
            r#"Mod+Shift+Escape { spawn "/home/u/.cargo/bin/niri-computer-use" "stop"; }"#,
        );
        assert_eq!(stale.state, StopKey::NotFound);
        assert!(
            stale.detail.contains(
                "/home/u/.cargo/bin/niri-computer-use stop`, which isn't an executable file"
            ),
            "{stale:?}"
        );
        let wrapped = state_of(
            r#"Mod+Shift+Escape { spawn "systemd-run" "--user" "niri-computer-use" "stop"; }"#,
        );
        assert_eq!(wrapped.state, StopKey::Unknown);
        assert!(
            wrapped.detail.contains("in a form this can't read"),
            "{wrapped:?}"
        );
    }

    fn state_of(bind: &str) -> Checked {
        scanned(
            "/c/config.kdl",
            &[("/c/config.kdl", &format!("binds {{\n    {bind}\n}}"))],
        )
    }

    #[test]
    fn finds_the_config_niri_was_started_with() {
        let dir = crate::test_support::fresh_dir("stop-key-locate");
        let system = dir.join("etc-config.kdl");
        let home = dir.join("home");
        let started = |args: &[&str], niri_config: Option<&str>, xdg: Option<&Path>| Started {
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            niri_config: niri_config.map(str::to_owned),
            xdg_config_home: xdg.map(|xdg| xdg.display().to_string()),
            home: Some(home.display().to_string()),
            path: None,
            cwd: Some(dir.clone()),
        };
        let at = |process: &Started| locate(process, &system);

        assert_eq!(
            at(&started(&["-c", "/x.kdl"], None, None)),
            Ok("/x.kdl".into())
        );
        assert_eq!(
            at(&started(&["--config=rel.kdl"], None, None)),
            Ok(dir.join("rel.kdl"))
        );
        assert_eq!(
            at(&started(&["--session", "-c/y.kdl"], Some("/z.kdl"), None)),
            Ok("/y.kdl".into())
        );
        assert_eq!(at(&started(&[], Some("/z.kdl"), None)), Ok("/z.kdl".into()));
        // A `-c` for the command niri runs isn't niri's.
        assert!(at(&started(&["--", "sh", "-c", "/x.kdl"], None, None)).is_err());

        std::fs::write(&system, "").unwrap();
        assert_eq!(at(&started(&[], None, None)), Ok(system.clone()));
        let user = home.join(".config/niri/config.kdl");
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::write(&user, "").unwrap();
        assert_eq!(at(&started(&[], None, None)), Ok(user));
        let xdg = dir.join("xdg");
        std::fs::create_dir_all(xdg.join("niri")).unwrap();
        std::fs::write(xdg.join("niri/config.kdl"), "").unwrap();
        assert_eq!(
            at(&started(&[], None, Some(&xdg))),
            Ok(xdg.join("niri/config.kdl"))
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_niris_process_from_proc() {
        let dir = crate::test_support::fresh_dir("stop-key-proc");
        let proc_root = dir.join("proc");
        let process = proc_root.join("40");
        std::fs::create_dir_all(&process).unwrap();
        let config = dir.join("niri.kdl");
        std::fs::write(
            &config,
            r#"binds { Mod+Shift+Escape { spawn "niri-computer-use" "stop"; } }"#,
        )
        .unwrap();
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        crate::test_support::script(&bin.join("niri-computer-use"), "true");
        std::fs::write(
            process.join("cmdline"),
            format!("niri\0-c\0{}\0", config.display()),
        )
        .unwrap();
        let environ = format!("HOME={}\0PATH=/nowhere:{}\0", dir.display(), bin.display());
        std::fs::write(process.join("environ"), &environ).unwrap();
        let system = dir.join("none.kdl");
        assert_eq!(inspect(&proc_root, 40, &system).state, StopKey::Bound);
        // The bare name is looked up on niri's PATH, not this process's.
        std::fs::write(process.join("environ"), format!("HOME={}\0", dir.display())).unwrap();
        let no_path = inspect(&proc_root, 40, &system);
        assert_eq!(no_path.state, StopKey::Unknown);
        assert!(no_path.detail.contains("niri has no PATH"), "{no_path:?}");
        let unreadable = inspect(&proc_root, 41, &system);
        assert_eq!(unreadable.state, StopKey::Unknown);
        assert!(unreadable.detail.contains("PID 41"), "{unreadable:?}");
        // The default place, empty here, falls back to the system config, missing here.
        std::fs::write(process.join("cmdline"), "niri\0--session\0").unwrap();
        assert_eq!(inspect(&proc_root, 40, &system).state, StopKey::Unknown);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_failed_config_load_or_no_niri_is_unknown() {
        let failed = check(&Socket::default(), Some(true)).await;
        assert_eq!(failed.state, StopKey::Unknown);
        assert!(failed.detail.contains("niri validate"), "{failed:?}");
        let gone = check(&Socket::default(), None).await;
        assert_eq!(gone.state, StopKey::Unknown);
        assert!(gone.detail.contains("NIRI_SOCKET is not set"), "{gone:?}");
    }

    // The install script can't call this, so it prints the line itself.
    #[test]
    fn the_install_script_prints_the_same_bind_line() {
        let script = include_str!("../install.sh");
        assert!(script.contains(&bind_line(Path::new("${binary}"))));
    }

    #[test]
    fn the_bind_line_is_one_niri_reads_as_the_stop_key() {
        let line = bind_line(Path::new("/home/u/.local/bin/niri-computer-use"));
        assert_eq!(state(&format!("binds {{\n    {line}\n}}")), StopKey::Bound);
    }
}
