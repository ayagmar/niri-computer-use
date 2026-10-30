//! What one bind in niri's config does, as far as the stop key goes: its key as niri
//! compares keys (`impl FromStr for Key` in v26.04 `niri-config/src/binds.rs`), and whether
//! its action starts `niri-computer-use stop` with a program that is there. The program is
//! found as niri's spawn finds it (`spawn_sync` in `src/utils/spawning.rs`): `~` at the
//! start under niri's own `HOME`, a relative path from niri's directory, a bare name on the
//! `PATH` niri gives the programs it starts, which its config's `environment` can change.
//! A shell niri starts expands `~` and `$HOME` with the `HOME` niri gives it.

use std::io;
use std::path::{Path, PathBuf};

use super::{PROGRAM, kdl};

/// What the scan knows of niri's process to find the program a bind starts.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct Spawner<'a> {
    /// niri's own `HOME`, where `spawn` finds a program written with `~`.
    pub(super) home: Option<&'a Path>,
    /// The `HOME` of the programs niri starts, where a shell finds `~` and `$HOME`.
    pub(super) child_home: Option<&'a str>,
    /// The `PATH` of the programs niri starts, which `spawn` and `sh` search for a bare
    /// name.
    pub(super) path: Option<&'a str>,
    pub(super) cwd: Option<&'a Path>,
}

/// Whether a path is a file niri could start: `Ok(false)` when it is missing or not
/// executable, an error when that can't be told.
pub(super) trait Executable {
    fn executable(&self, path: &Path) -> io::Result<bool>;
}

/// What a bind's action does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Runs {
    /// `niri-computer-use stop`, the program found at this path.
    Stop(PathBuf),
    /// `niri-computer-use stop`, the program named by a path where no executable is.
    Missing(PathBuf),
    /// `niri-computer-use stop`, the program not to be found for sure, and why.
    Unsure(String),
    /// Something that names the program in a form this doesn't read, such as a wrapper
    /// command, extra arguments or a shell redirection.
    Unread,
    Other,
}

/// `name`, a bind's key, as niri compares two keys: the set of modifiers, with niri's
/// aliases, and the trigger, whose case niri ignores. Errors on a modifier niri refuses.
pub(super) fn key(name: &str) -> Result<String, String> {
    let mut parts = name.split('+');
    let trigger = parts.next_back().unwrap_or_default();
    let mut modifiers = 0_u8;
    for part in parts {
        modifiers |= match part.trim().to_ascii_lowercase().as_str() {
            "mod" => 1,
            "ctrl" | "control" => 2,
            "shift" => 4,
            "alt" => 8,
            "super" | "win" => 16,
            "iso_level3_shift" | "mod5" => 32,
            "iso_level5_shift" | "mod3" => 64,
            _ => return Err(format!("{name}: niri knows no modifier {part}")),
        };
    }
    Ok(format!("{modifiers}+{}", trigger.to_lowercase()))
}

/// What `action`, the one child of a bind, runs.
pub(super) fn runs(action: &kdl::Node, spawner: Spawner<'_>, disk: &impl Executable) -> Runs {
    let read = match (action.name.as_str(), action.args.as_slice()) {
        ("spawn", [program, command]) if names_program(program) => Some(if command == "stop" {
            found(program, spawner.home, spawner, disk)
        } else {
            Runs::Other
        }),
        ("spawn", [shell, dash_c, line]) if is_shell(shell) && dash_c == "-c" => {
            Some(shell_runs(line, spawner, disk))
        }
        ("spawn-sh", [line]) => Some(shell_runs(line, spawner, disk)),
        _ => None,
    };
    read.unwrap_or_else(|| {
        if action.args.iter().any(|arg| words(arg).any(names_program)) {
            Runs::Unread
        } else {
            Runs::Other
        }
    })
}

/// `text` taken apart at spaces, quotes dropped.
fn words(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace()
        .map(|word| word.trim_matches(|c| c == '"' || c == '\''))
}

fn names_program(word: &str) -> bool {
    Path::new(word)
        .file_name()
        .is_some_and(|name| name == PROGRAM)
}

fn is_shell(program: &str) -> bool {
    Path::new(program)
        .file_name()
        .is_some_and(|name| name == "sh" || name == "bash")
}

/// What a shell command line runs: the stop when one of its commands is exactly the
/// program and `stop`; something else when every command that names the program is it
/// and one other word, such as `resume`; else it can't be read. `~` and `$HOME` at the
/// start of the program are the shell's `HOME`.
fn shell_runs(line: &str, spawner: Spawner<'_>, disk: &impl Executable) -> Runs {
    let commands: Vec<Vec<&str>> = line
        .split(|c: char| ";&|()\n".contains(c))
        .map(|command| words(command).collect())
        .collect();
    let stop = commands
        .iter()
        .find_map(|command| match command.as_slice() {
            [program, "stop"] if names_program(program) => Some(*program),
            _ => None,
        });
    if let Some(program) = stop {
        let program = ["$HOME/", "${HOME}/"]
            .iter()
            .find_map(|home| program.strip_prefix(home))
            .map_or_else(|| program.to_owned(), |rest| format!("~/{rest}"));
        return found(&program, spawner.child_home.map(Path::new), spawner, disk);
    }
    let read = commands.iter().all(|command| match command.as_slice() {
        [program, _] if names_program(program) => true,
        command => !command.iter().copied().any(names_program),
    });
    if read { Runs::Other } else { Runs::Unread }
}

/// Where the program `word` names is, as niri would start it, with `~` under `home`.
fn found(word: &str, home: Option<&Path>, spawner: Spawner<'_>, disk: &impl Executable) -> Runs {
    if word.contains('$') {
        return Runs::Unsure(format!("`{word}` names it through a shell variable"));
    }
    let path = if let Some(rest) = word.strip_prefix("~/") {
        let Some(home) = home else {
            return Runs::Unsure(format!("there is no HOME to find `{word}` under"));
        };
        home.join(rest)
    } else if word.contains('/') {
        match (Path::new(word).is_absolute(), spawner.cwd) {
            (true, _) => PathBuf::from(word),
            (false, Some(cwd)) => cwd.join(word),
            (false, None) => {
                return Runs::Unsure(format!(
                    "`{word}` is relative to niri's directory, which can't be read"
                ));
            }
        }
    } else {
        return on_path(word, spawner, disk);
    };
    match disk.executable(&path) {
        Ok(true) => Runs::Stop(path),
        Ok(false) => Runs::Missing(path),
        Err(error) => Runs::Unsure(format!("check {}: {error}", path.display())),
    }
}

/// The first executable `name` in the directories on `PATH`, as `execvp` searches them: an
/// empty or relative one is taken from niri's directory.
fn on_path(name: &str, spawner: Spawner<'_>, disk: &impl Executable) -> Runs {
    let Some(path) = spawner.path else {
        return Runs::Unsure(format!(
            "niri starts programs with no PATH to find `{name}` on"
        ));
    };
    for dir in path.split(':') {
        let dir = match (Path::new(dir).is_absolute(), spawner.cwd) {
            (true, _) => PathBuf::from(dir),
            (false, Some(cwd)) => cwd.join(dir),
            (false, None) => {
                return Runs::Unsure(format!(
                    "PATH holds `{dir}`, relative to niri's directory, which can't be read"
                ));
            }
        };
        let candidate = dir.join(name);
        match disk.executable(&candidate) {
            Ok(true) => return Runs::Stop(candidate),
            Ok(false) => {}
            Err(error) => {
                return Runs::Unsure(format!("check {}: {error}", candidate.display()));
            }
        }
    }
    Runs::Unsure(format!(
        "`{name}` is in none of the directories on the PATH niri starts programs with"
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const INSTALLED: &str = "/home/u/.local/bin/niri-computer-use";

    struct Installed(BTreeSet<PathBuf>);

    impl Executable for Installed {
        fn executable(&self, path: &Path) -> io::Result<bool> {
            if path.starts_with("/locked") {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            Ok(self.0.contains(path))
        }
    }

    fn installed(paths: &[&str]) -> Installed {
        Installed(paths.iter().map(PathBuf::from).collect())
    }

    fn spawner() -> Spawner<'static> {
        Spawner {
            home: Some(Path::new("/home/u")),
            child_home: Some("/home/u"),
            path: Some("/usr/bin:relative:/home/u/.local/bin"),
            cwd: Some(Path::new("/home/u")),
        }
    }

    fn action(name: &str, args: &[&str]) -> kdl::Node {
        kdl::Node {
            name: name.to_owned(),
            args: args.iter().map(|&arg| arg.to_owned()).collect(),
            ..kdl::Node::default()
        }
    }

    fn run(name: &str, args: &[&str]) -> Runs {
        runs(&action(name, args), spawner(), &installed(&[INSTALLED]))
    }

    #[test]
    fn keys_compare_as_niri_compares_them() {
        assert_eq!(key("Mod+Shift+Escape"), key("shift+MOD+escape"));
        assert_eq!(key("Ctrl+Win+A"), key("Control+Super+a"));
        assert_eq!(key("Mod5+x"), key("ISO_Level3_Shift+X"));
        assert_ne!(key("Mod+Escape"), key("Super+Escape"));
        assert_ne!(key("Mod+Escape"), key("Mod+Shift+Escape"));
        assert!(key("Hyper+Escape").is_err());
    }

    #[test]
    fn a_stop_is_found_where_niri_would_start_it() {
        let stop = Runs::Stop(PathBuf::from(INSTALLED));
        for (name, args) in [
            ("spawn", vec![INSTALLED, "stop"]),
            ("spawn", vec!["~/.local/bin/niri-computer-use", "stop"]),
            ("spawn", vec![".local/bin/niri-computer-use", "stop"]),
            ("spawn", vec!["niri-computer-use", "stop"]),
            ("spawn-sh", vec!["niri-computer-use stop"]),
            (
                "spawn-sh",
                vec!["\"$HOME/.local/bin/niri-computer-use\" stop"],
            ),
            (
                "spawn-sh",
                vec!["niri-computer-use stop && notify-send stopped"],
            ),
            ("spawn", vec!["sh", "-c", "niri-computer-use stop"]),
            (
                "spawn",
                vec!["/usr/bin/bash", "-c", "~/.local/bin/niri-computer-use stop"],
            ),
        ] {
            assert_eq!(run(name, &args), stop, "{name} {args:?}");
        }
    }

    #[test]
    fn a_program_that_isnt_there_or_cant_be_found_says_which() {
        assert_eq!(
            run("spawn", &["/home/u/.cargo/bin/niri-computer-use", "stop"]),
            Runs::Missing(PathBuf::from("/home/u/.cargo/bin/niri-computer-use"))
        );
        let off_path = runs(
            &action("spawn", &["niri-computer-use", "stop"]),
            spawner(),
            &installed(&["/opt/niri-computer-use"]),
        );
        assert!(
            matches!(&off_path, Runs::Unsure(why) if why.contains("PATH")),
            "{off_path:?}"
        );
        let no_path = Spawner {
            path: None,
            ..spawner()
        };
        let unsure = runs(
            &action("spawn", &["niri-computer-use", "stop"]),
            no_path,
            &installed(&[INSTALLED]),
        );
        assert!(matches!(unsure, Runs::Unsure(_)), "{unsure:?}");
        for (name, args) in [
            ("spawn", vec!["/locked/niri-computer-use", "stop"]),
            ("spawn-sh", vec!["$BIN/niri-computer-use stop"]),
        ] {
            let said = run(name, &args);
            assert!(matches!(said, Runs::Unsure(_)), "{args:?}: {said:?}");
        }
    }

    // `execvp` takes an empty or relative directory on `PATH` from the directory it runs
    // in, which is niri's.
    #[test]
    fn a_relative_or_empty_path_entry_is_niris_directory() {
        let stop = action("spawn", &["niri-computer-use", "stop"]);
        let at = |path, cwd| {
            let spawner = Spawner {
                path: Some(path),
                cwd,
                ..spawner()
            };
            runs(&stop, spawner, &installed(&[INSTALLED]))
        };
        let found = Runs::Stop(PathBuf::from(INSTALLED));
        assert_eq!(at("", Some(Path::new("/home/u/.local/bin"))), found);
        assert_eq!(at("/usr/bin:.local/bin", Some(Path::new("/home/u"))), found);
        let unreadable = at("/usr/bin::/home/u/.local/bin", None);
        assert!(
            matches!(&unreadable, Runs::Unsure(why) if why.contains("niri's directory")),
            "{unreadable:?}"
        );
    }

    #[test]
    fn a_form_that_names_the_program_but_isnt_read_is_unread() {
        for (name, args) in [
            (
                "spawn",
                vec!["systemd-run", "--user", "niri-computer-use", "stop"],
            ),
            ("spawn", vec!["niri-computer-use", "stop", "--quiet"]),
            ("spawn-sh", vec!["pkill -f 'niri-computer-use stop'"]),
            ("spawn-sh", vec!["exec niri-computer-use stop"]),
            ("spawn-sh", vec!["niri-computer-use stop 2>/dev/null"]),
            ("spawn-sh", vec!["echo niri-computer-use; stop"]),
        ] {
            assert_eq!(run(name, &args), Runs::Unread, "{name} {args:?}");
        }
        // Read in full, and not the stop.
        for (name, args) in [
            ("spawn", vec!["niri-computer-use", "resume"]),
            (
                "spawn-sh",
                vec!["niri-computer-use resume; notify-send resumed"],
            ),
            ("spawn", vec!["niri-computer-use-helper", "stop"]),
            ("spawn-sh", vec!["foot"]),
            ("close-window", vec![]),
        ] {
            assert_eq!(run(name, &args), Runs::Other, "{name} {args:?}");
        }
    }
}
