//! Installed desktop entries, which `launch` starts by id when the policy file sets
//! `launch_desktop_entries = true`: where they are, from the client's XDG variables, and
//! finding and reading them within a deadline. Only entries the user can't write are
//! read (`read`); parsing one is in `entry`.

mod entry;
mod read;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{ErrorName, ToolError};
use crate::policy::{self, Launchable, Loaded};
use crate::runner::{self, Place};
use crate::session::Settings;

/// How long finding and reading the entries may take.
const DEADLINE: Duration = Duration::from_secs(2);
/// How deep under an `applications` directory entries are looked for.
const MAX_DEPTH: usize = 3;
/// The most files and directories one search looks at.
const MAX_FILES: usize = 8192;
/// How many ids `status` lists.
const IDS_SHOWN: usize = 64;
/// How many close matches an unknown id's refusal names.
const CLOSE_SHOWN: usize = 5;

/// Where a client's desktop entries are, from its own environment, as a menu in its
/// session would look: `$XDG_DATA_HOME/applications`, or `~/.local/share/applications`,
/// then each of `$XDG_DATA_DIRS`, or `/usr/local/share` and `/usr/share`, in that order of
/// precedence. Relative directories are ignored, as the base directory spec says, and so
/// are ones that aren't UTF-8.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Search {
    applications: Vec<String>,
    /// `$XDG_CURRENT_DESKTOP`'s names, or `niri` when it is unset, for `OnlyShowIn` and
    /// `NotShowIn`.
    desktops: Vec<String>,
    /// Who an entry file and its directories must belong to: root. Never sent in a hello;
    /// only tests change it, since they can't make files of root's.
    #[serde(skip)]
    owner: u32,
}

impl Search {
    /// What `var`, which counts an empty variable as unset, and `home` give.
    pub(crate) fn read(var: impl Fn(&str) -> Option<OsString>, home: Option<&Path>) -> Self {
        let absolute = |dir: PathBuf| dir.is_absolute().then_some(dir);
        let data_home = var("XDG_DATA_HOME")
            .and_then(|dir| absolute(dir.into()))
            .or_else(|| home.map(|home| home.join(".local/share")));
        let data_dirs: Vec<PathBuf> = var("XDG_DATA_DIRS").map_or_else(
            || vec!["/usr/local/share".into(), "/usr/share".into()],
            |dirs| std::env::split_paths(&dirs).filter_map(absolute).collect(),
        );
        let mut applications: Vec<String> = Vec::new();
        for dir in data_home.into_iter().chain(data_dirs) {
            let dir: PathBuf = dir.join("applications").components().collect();
            if let Some(dir) = dir.to_str()
                && !applications.iter().any(|seen| seen == dir)
            {
                applications.push(dir.to_owned());
            }
        }
        let desktops: Vec<String> = var("XDG_CURRENT_DESKTOP")
            .and_then(|names| names.into_string().ok())
            .map(|names| {
                names
                    .split(':')
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .filter(|names: &Vec<String>| !names.is_empty())
            .unwrap_or_else(|| vec!["niri".to_owned()]);
        Self {
            applications,
            desktops,
            owner: 0,
        }
    }
}

/// What `launch` starts for `name`: the preset of that name, or else, with
/// `launch_desktop_entries`, the installed entry with that id, `.desktop` or not, read now.
/// `entry` gets the entry's id once a file has it, for the audit log. Without the key, no
/// file is read.
pub(crate) async fn resolve(
    settings: &Settings,
    name: &str,
    entry: &OnceLock<String>,
) -> Result<Launchable, ToolError> {
    let policy = &settings.policy;
    let unknown = match policy.preset(name) {
        Ok(preset) => return Ok(preset.launchable()),
        Err(unknown) => unknown,
    };
    if !policy.launch_desktop_entries() {
        return Err(unknown);
    }
    let id = name.strip_suffix(".desktop").unwrap_or(name).to_owned();
    let search = settings.apps.clone();
    let place = Owned::of(settings.place());
    let wanted = id.clone();
    let found = blocking(move || look_up(&search, place.place(), &wanted)).await?;
    match found {
        Lookup::Found(start) => {
            entry.get_or_init(|| id.clone());
            let start = start.map_err(|why| {
                ToolError::new(
                    ErrorName::EntryRefused,
                    format!("desktop entry {id:?} {why}"),
                )
            })?;
            launchable(policy, &id, start)
        }
        Lookup::Missing(all) => Err(not_found(policy, &id, &unknown, &all)),
    }
}

/// What `status` reports about the installed entries `launch` would start: how many, and
/// the first `IDS_SHOWN` ids in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Listing {
    count: usize,
    ids: Vec<String>,
    /// How many entries take precedence but are in a file or directory the user can write,
    /// which `launch` refuses: a user-writable entry needs a preset.
    user_writable: usize,
    /// Why they couldn't be listed.
    error: Option<String>,
}

/// The entries `launch` would start for this session; none without `launch_desktop_entries`,
/// so no file is read.
pub(crate) async fn listing(policy: &Loaded, search: &Search, place: Place<'_>) -> Option<Listing> {
    if !policy.launch_desktop_entries() {
        return None;
    }
    let search = search.clone();
    let place = Owned::of(place);
    let all = match blocking(move || every(&search, place.place())).await {
        Ok(all) => all,
        Err(error) => {
            return Some(Listing {
                count: 0,
                ids: Vec::new(),
                user_writable: 0,
                error: Some(error.detail),
            });
        }
    };
    let ids: Vec<String> = startable(policy, &all).map(|(id, _)| id.clone()).collect();
    Some(Listing {
        count: ids.len(),
        ids: ids.into_iter().take(IDS_SHOWN).collect(),
        user_writable: all.values().filter(|checked| checked.writable).count(),
        error: None,
    })
}

/// An entry file as `launch` would read it: its `Name`, and its command or why it has none,
/// and whether that is because the user can write it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Checked {
    name: String,
    start: Result<entry::Startable, String>,
    writable: bool,
}

/// An entry file found under `data_dir`, the directory its `applications` is in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Located {
    path: PathBuf,
    data_dir: PathBuf,
}

/// What looking for one id found: its entry, or every entry, for close matches.
#[derive(Debug)]
enum Lookup {
    Found(Result<entry::Startable, String>),
    Missing(BTreeMap<String, Checked>),
}

fn look_up(search: &Search, place: Place<'_>, id: &str) -> Lookup {
    let files = index(&search.applications);
    files.get(id).map_or_else(
        || Lookup::Missing(checked(&files, search, place)),
        |file| Lookup::Found(check(file, search, place).start),
    )
}

fn every(search: &Search, place: Place<'_>) -> BTreeMap<String, Checked> {
    checked(&index(&search.applications), search, place)
}

fn checked(
    files: &BTreeMap<String, Located>,
    search: &Search,
    place: Place<'_>,
) -> BTreeMap<String, Checked> {
    files
        .iter()
        .map(|(id, file)| (id.clone(), check(file, search, place)))
        .collect()
}

/// The entries in `all` that `launch` would start under `policy`, by id.
fn startable<'a>(
    policy: &'a Loaded,
    all: &'a BTreeMap<String, Checked>,
) -> impl Iterator<Item = (&'a String, &'a Checked)> {
    all.iter().filter(|(id, checked)| {
        checked
            .start
            .as_ref()
            .is_ok_and(|start| launchable(policy, id, start.clone()).is_ok())
    })
}

/// The entry `id` as `launch` starts it, unless the preset rules refuse its command
/// (`entry_refused`) or the deny list has its id or `StartupWMClass` (`app_denied`).
fn launchable(policy: &Loaded, id: &str, start: entry::Startable) -> Result<Launchable, ToolError> {
    if let Some(why) = policy::refuse_command(&start.argv) {
        return Err(ToolError::new(
            ErrorName::EntryRefused,
            format!("desktop entry {id:?} {why}"),
        ));
    }
    let mut app_ids = vec![id.to_owned()];
    app_ids.extend(start.wm_class.filter(|class| class != id));
    if let Some(denied) = policy::refuse_entry(policy, id, &app_ids) {
        return Err(denied);
    }
    Ok(Launchable {
        command: start.argv,
        app_ids,
    })
}

/// `unknown_preset` for a name no preset and no entry has, naming the presets and the
/// startable entries whose id or `Name` contains it, ignoring case.
fn not_found(
    policy: &Loaded,
    id: &str,
    unknown: &ToolError,
    all: &BTreeMap<String, Checked>,
) -> ToolError {
    let wanted = id.to_lowercase();
    let close: Vec<String> = startable(policy, all)
        .filter(|(other, checked)| {
            other.to_lowercase().contains(&wanted) || checked.name.to_lowercase().contains(&wanted)
        })
        .take(CLOSE_SHOWN)
        .map(|(other, checked)| format!("{other} ({})", checked.name))
        .collect();
    ToolError::new(
        ErrorName::UnknownPreset,
        format!(
            "{}, and no installed desktop entry has that id; the entries whose id or name contains it are {close:?}",
            unknown.detail
        ),
    )
}

/// Reads and checks the entry `file`, if the user can't write it, looking for its
/// `TryExec` in `place`.
fn check(file: &Located, search: &Search, place: Place<'_>) -> Checked {
    let path = &file.path;
    let refused = |why: String, writable| Checked {
        name: String::new(),
        start: Err(format!("at {} {why}", path.display())),
        writable,
    };
    let text = match read::read(path, &file.data_dir, search.owner) {
        Ok(text) => text,
        Err(read::Unread::Writable(why)) => {
            return refused(
                format!(
                    "is one the user can write ({why}), and a user-writable entry needs a preset"
                ),
                true,
            );
        }
        Err(read::Unread::Failed(why)) => return refused(why, false),
    };
    let entry = match entry::parse(&text) {
        Ok(entry) => entry,
        Err(why) => return refused(format!("is invalid: {why}"), false),
    };
    let desktops = &search.desktops;
    let start =
        entry::startable(&entry, desktops, &path.to_string_lossy()).and_then(|start| match &start
            .try_exec
        {
            Some(program) if !runner::finds(place, program) => {
                Err(format!("has TryExec={program}, which isn't installed"))
            }
            _ => Ok(start),
        });
    Checked {
        name: entry.name.unwrap_or_default(),
        start,
        writable: false,
    }
}

/// The entry files under `applications`, by id, each the one that takes precedence: the
/// first directory's. A file in a subdirectory has the id of its path with `/` as `-`, as
/// `kde4/okular.desktop` is `kde4-okular`; when two files in one directory have one id, the
/// shallower wins, then the first by name. A directory that is missing or can't be read is
/// skipped.
fn index(applications: &[String]) -> BTreeMap<String, Located> {
    let mut found = BTreeMap::new();
    let mut budget = MAX_FILES;
    for dir in applications {
        let dir = Path::new(dir);
        let data_dir = dir.parent().unwrap_or(dir).to_owned();
        let mut here = BTreeMap::new();
        walk(dir, "", 0, &mut here, &mut budget);
        for (id, (_, path)) in here {
            found.entry(id).or_insert_with(|| Located {
                path,
                data_dir: data_dir.clone(),
            });
        }
    }
    found
}

/// Adds the entries under `dir`, whose ids start with `prefix`, to `found`, with their
/// depth. Symlinks are followed, as flatpak's exported entries are symlinks.
fn walk(
    dir: &Path,
    prefix: &str,
    depth: usize,
    found: &mut BTreeMap<String, (usize, PathBuf)>,
    budget: &mut usize,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .collect();
    names.sort_unstable();
    for name in names {
        let Some(left) = budget.checked_sub(1) else {
            return;
        };
        *budget = left;
        let path = dir.join(&name);
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() && depth < MAX_DEPTH {
            walk(&path, &format!("{prefix}{name}-"), depth + 1, found, budget);
        } else if let Some(stem) = name.strip_suffix(".desktop").filter(|_| metadata.is_file()) {
            let id = format!("{prefix}{stem}");
            let shallower = found.get(&id).is_none_or(|(seen, _)| depth < *seen);
            if shallower {
                found.insert(id, (depth, path));
            }
        }
    }
}

/// A session's `Place`, owned, for a blocking task.
#[derive(Debug)]
struct Owned {
    path: Option<OsString>,
    cwd: Option<PathBuf>,
}

impl Owned {
    fn of(place: Place<'_>) -> Self {
        Self {
            path: place.path.map(ToOwned::to_owned),
            cwd: place.cwd.map(ToOwned::to_owned),
        }
    }

    fn place(&self) -> Place<'_> {
        Place {
            path: self.path.as_deref(),
            cwd: self.cwd.as_deref(),
        }
    }
}

/// Runs `work`, which reads files, off the async runtime, and gives up on it after
/// `DEADLINE`, as on a hung network mount.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, ToolError> {
    match tokio::time::timeout(DEADLINE, tokio::task::spawn_blocking(work)).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(ToolError::new(
            ErrorName::UpstreamError,
            format!("reading the desktop entries failed: {error}"),
        )),
        Err(_) => Err(ToolError::new(
            ErrorName::DeadlineExceeded,
            format!(
                "reading the desktop entries took over {} seconds",
                DEADLINE.as_secs()
            ),
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;
    use crate::policy::Source;
    use crate::session::Given;

    /// A fresh directory with a data home and two data directories, which the test removes.
    struct Tree {
        root: PathBuf,
    }

    impl Tree {
        fn new(name: &str) -> Self {
            Self {
                root: crate::test_support::fresh_dir(&format!("apps-{name}")),
            }
        }

        /// The `applications` directory of data directory `which`: `home`, `one` or `two`.
        fn applications(&self, which: &str) -> PathBuf {
            self.root.join(which).join("applications")
        }

        /// Writes an entry file only its owner may write, in directories only it may write,
        /// as a package manager installs one.
        fn write(&self, which: &str, file: &str, text: &str) -> PathBuf {
            let path = self.applications(which).join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            let mode = |mode| std::fs::Permissions::from_mode(mode);
            std::fs::set_permissions(&path, mode(0o644)).unwrap();
            let dirs = path.ancestors().skip(1);
            for dir in dirs.take_while(|dir| dir.starts_with(&self.root)) {
                std::fs::set_permissions(dir, mode(0o755)).unwrap();
            }
            path
        }

        /// A session whose policy file is `policy` and whose XDG variables name this tree,
        /// which trusts files the test's user owns, as a real one trusts root's.
        fn settings(&self, policy: &str) -> Settings {
            let mut settings = self.untrusted(policy);
            settings.apps.owner = rustix::process::geteuid().as_raw();
            settings
        }

        /// A session as `settings` gives, which trusts only root's files, as a real one does.
        fn untrusted(&self, policy: &str) -> Settings {
            let dirs = |name: &str| self.root.join(name).into_os_string();
            let mut data_dirs = dirs("one");
            data_dirs.push(":");
            data_dirs.push(dirs("two"));
            let var = |name: &str| match name {
                "XDG_DATA_HOME" => Some(dirs("home")),
                "XDG_DATA_DIRS" => Some(data_dirs.clone()),
                _ => None,
            };
            Settings::new(Given {
                unrestricted: None,
                confirm_lease: None,
                tools: None,
                shared: None,
                home: None,
                path: Some(crate::test_support::NO_PROGRAMS.into()),
                policy: Source::Text {
                    path: PathBuf::from("/c/policy.toml"),
                    text: policy.to_owned(),
                },
                apps: Search::read(var, None),
            })
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    const ON: &str = "launch_desktop_entries = true\n";

    fn app(exec: &str, extra: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName=App\nExec={exec}\n{extra}")
    }

    async fn launch(
        settings: &Settings,
        name: &str,
    ) -> (Result<Launchable, ToolError>, Option<String>) {
        let entry = OnceLock::new();
        let resolved = resolve(settings, name, &entry).await;
        (resolved, entry.get().cloned())
    }

    fn refused(resolved: &Result<Launchable, ToolError>) -> (ErrorName, &str) {
        let error = resolved.as_ref().unwrap_err();
        (error.name, error.detail.as_str())
    }

    #[test]
    fn the_session_s_variables_name_the_directories_in_order() {
        let var = |name: &str| match name {
            "XDG_DATA_HOME" => Some("relative/share".into()),
            "XDG_DATA_DIRS" => Some("/opt/share/:relative:/usr/share".into()),
            "XDG_CURRENT_DESKTOP" => Some("niri:GNOME".into()),
            _ => None,
        };
        let search = Search::read(var, Some(Path::new("/home/me")));
        assert_eq!(
            search.applications,
            [
                "/home/me/.local/share/applications",
                "/opt/share/applications",
                "/usr/share/applications"
            ]
        );
        assert_eq!(search.desktops, ["niri", "GNOME"]);
        let unset = Search::read(|_| None, None);
        assert_eq!(
            unset.applications,
            ["/usr/local/share/applications", "/usr/share/applications"]
        );
        assert_eq!(unset.desktops, ["niri"]);
    }

    #[tokio::test]
    async fn the_data_home_wins_and_a_hidden_override_hides_the_system_entry() {
        let tree = Tree::new("precedence");
        tree.write("two", "org.example.Mail.desktop", &app("mail-two", ""));
        tree.write("one", "org.example.Mail.desktop", &app("mail-one %U", ""));
        tree.write(
            "home",
            "org.example.Editor.desktop",
            &app("editor", "Hidden=true\n"),
        );
        tree.write("one", "org.example.Editor.desktop", &app("editor", ""));
        let settings = tree.settings(ON);
        let (mail, mail_id) = launch(&settings, "org.example.Mail").await;
        assert_eq!(mail.unwrap().command, ["mail-one"]);
        assert_eq!(mail_id.as_deref(), Some("org.example.Mail"));
        let (editor, editor_id) = launch(&settings, "org.example.Editor.desktop").await;
        assert_eq!(refused(&editor).0, ErrorName::EntryRefused);
        assert!(refused(&editor).1.contains("Hidden=true"), "{editor:?}");
        assert_eq!(editor_id.as_deref(), Some("org.example.Editor"));
    }

    #[tokio::test]
    async fn a_subdirectory_gives_a_dashed_id_and_bad_directories_are_skipped() {
        let tree = Tree::new("subdirectories");
        // The data home's `applications` is a file, and the first data directory has none.
        std::fs::create_dir_all(tree.root.join("home")).unwrap();
        std::fs::write(tree.applications("home"), "not a directory").unwrap();
        tree.write("two", "kde4/okular.desktop", &app("okular", ""));
        let (okular, _) = launch(&tree.settings(ON), "kde4-okular").await;
        assert_eq!(okular.unwrap().command, ["okular"]);
    }

    #[tokio::test]
    async fn a_malformed_or_unreadable_file_is_refused_with_its_path() {
        let tree = Tree::new("malformed");
        let path = tree.write("one", "broken.desktop", "Exec=x\n[Desktop Entry]\n");
        let path = path.display().to_string();
        std::fs::write(
            tree.applications("one").join("binary.desktop"),
            [0xff, 0xfe],
        )
        .unwrap();
        let settings = tree.settings(ON);
        let (broken, _) = launch(&settings, "broken").await;
        assert_eq!(refused(&broken).0, ErrorName::EntryRefused);
        assert!(refused(&broken).1.contains(&path), "{broken:?}");
        let (binary, _) = launch(&settings, "binary").await;
        assert!(refused(&binary).1.contains("can't be read"), "{binary:?}");
    }

    #[tokio::test]
    async fn the_preset_rules_hold_for_the_command_of_an_entry() {
        let tree = Tree::new("rules");
        for (id, exec) in [
            ("shell", "sh -c \"firefox; rm -rf ~\""),
            ("env", "env GDK_BACKEND=x11 app"),
            (
                "flatpak-shell",
                "flatpak run --command=bash org.example.App",
            ),
            ("terminal-command", "foot -e htop"),
            ("terminal", "foot"),
            (
                "flatpak",
                "/usr/bin/flatpak run --command=firefox --file-forwarding org.mozilla.firefox @@u %u @@",
            ),
        ] {
            tree.write("one", &format!("{id}.desktop"), &app(exec, ""));
        }
        tree.write("one", "htop.desktop", &app("htop", "Terminal=true\n"));
        let settings = tree.settings(ON);
        for id in ["shell", "env", "flatpak-shell", "terminal-command", "htop"] {
            let (resolved, _) = launch(&settings, id).await;
            assert_eq!(
                refused(&resolved).0,
                ErrorName::EntryRefused,
                "{id}: {resolved:?}"
            );
        }
        for id in ["terminal", "flatpak"] {
            let (resolved, _) = launch(&settings, id).await;
            assert!(resolved.is_ok(), "{id}: {resolved:?}");
        }
    }

    #[tokio::test]
    async fn a_denied_app_id_or_window_class_is_not_launched() {
        let tree = Tree::new("denied");
        tree.write(
            "one",
            "org.keepassxc.KeePassXC.desktop",
            &app("keepassxc", ""),
        );
        tree.write(
            "one",
            "vault.desktop",
            &app("vault", "StartupWMClass=org.keepassxc.KeePassXC\n"),
        );
        let policy = format!("{ON}deny_input_app_ids = [\"org.keepassxc.KeePassXC\"]\n");
        let settings = tree.settings(&policy);
        for id in ["org.keepassxc.KeePassXC", "vault"] {
            let (resolved, _) = launch(&settings, id).await;
            assert_eq!(refused(&resolved).0, ErrorName::AppDenied, "{id}");
        }
        tree.write("one", "chat.desktop", &app("chat", "StartupWMClass=Chat\n"));
        let (chat, _) = launch(&settings, "chat").await;
        assert_eq!(chat.unwrap().app_ids, ["chat", "Chat"]);
    }

    #[tokio::test]
    async fn try_exec_must_name_an_installed_program() {
        let tree = Tree::new("try-exec");
        let program = tree.root.join("installed");
        std::fs::write(&program, "").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let installed = format!("TryExec={}\n", program.display());
        tree.write("one", "here.desktop", &app("here", &installed));
        tree.write(
            "one",
            "gone.desktop",
            &app("gone", "TryExec=/nonexistent/gone\n"),
        );
        let settings = tree.settings(ON);
        assert!(launch(&settings, "here").await.0.is_ok());
        let (gone, _) = launch(&settings, "gone").await;
        assert!(refused(&gone).1.contains("TryExec"), "{gone:?}");
    }

    #[tokio::test]
    async fn a_preset_wins_and_without_the_key_no_entry_is_used() {
        let tree = Tree::new("presets");
        tree.write("one", "files.desktop", &app("entry-files", ""));
        let preset =
            "[[preset]]\nname = \"files\"\nargv = [\"preset-files\"]\napp_id = \"files\"\n";
        let (files, logged) = launch(&tree.settings(&format!("{ON}{preset}")), "files").await;
        assert_eq!(files.unwrap().command, ["preset-files"]);
        assert_eq!(logged, None);
        let off = tree.settings(preset);
        let (other, _) = launch(&tree.settings(""), "files").await;
        assert_eq!(refused(&other).0, ErrorName::UnknownPreset);
        assert!(!refused(&other).1.contains("desktop entry"), "{other:?}");
        let status = listing(&off.policy, &off.apps, off.place()).await;
        assert_eq!(status, None);
    }

    #[tokio::test]
    async fn an_unknown_id_names_close_matches_and_status_lists_what_would_start() {
        let tree = Tree::new("close");
        tree.write(
            "one",
            "org.gnome.Nautilus.desktop",
            &app("nautilus", "").replace("Name=App", "Name=Files"),
        );
        tree.write(
            "one",
            "org.gnome.Settings.desktop",
            &app("settings", "NoDisplay=true\n"),
        );
        tree.write("one", "shell.desktop", &app("bash", ""));
        let settings = tree.settings(ON);
        let (files, logged) = launch(&settings, "files").await;
        assert_eq!(refused(&files).0, ErrorName::UnknownPreset);
        assert!(
            refused(&files).1.contains("org.gnome.Nautilus (Files)"),
            "{files:?}"
        );
        assert_eq!(logged, None);
        let status = listing(&settings.policy, &settings.apps, settings.place()).await;
        let expected = Listing {
            count: 1,
            ids: vec!["org.gnome.Nautilus".to_owned()],
            user_writable: 0,
            error: None,
        };
        assert_eq!(status, Some(expected));
    }

    #[tokio::test]
    async fn an_entry_the_user_can_write_needs_a_preset() {
        let tree = Tree::new("user-writable");
        let path = tree.write("home", "evil.desktop", &app("touch pwned", ""));
        tree.write("one", "files.desktop", &app("files", ""));
        let settings = tree.untrusted(ON);
        for id in ["evil", "files"] {
            let (resolved, _) = launch(&settings, id).await;
            assert_eq!(refused(&resolved).0, ErrorName::EntryRefused, "{id}");
            assert!(
                refused(&resolved)
                    .1
                    .contains("a user-writable entry needs a preset"),
                "{resolved:?}"
            );
        }
        assert!(
            launch(&settings, "evil")
                .await
                .0
                .unwrap_err()
                .detail
                .contains(&path.display().to_string())
        );
        let status = listing(&settings.policy, &settings.apps, settings.place()).await;
        let expected = Listing {
            count: 0,
            ids: Vec::new(),
            user_writable: 2,
            error: None,
        };
        assert_eq!(status, Some(expected));
    }

    #[tokio::test]
    async fn the_file_is_read_again_at_launch() {
        let tree = Tree::new("changed");
        tree.write("one", "app.desktop", &app("old", ""));
        let settings = tree.settings(ON);
        let listed = listing(&settings.policy, &settings.apps, settings.place()).await;
        assert_eq!(listed.unwrap().count, 1);
        tree.write("one", "app.desktop", &app("new --flag", ""));
        assert_eq!(
            launch(&settings, "app").await.0.unwrap().command,
            ["new", "--flag"]
        );
    }
}
