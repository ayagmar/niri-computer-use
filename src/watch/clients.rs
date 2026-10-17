//! The servers agents run on this niri instance, read from `/proc`.

use std::path::Path;

use serde::Serialize;

use super::parent_pid;
use crate::control::procs;

/// What `/proc` shows after the target of an `exe` link whose file was replaced or removed.
const DELETED: &str = " (deleted)";

/// One agent's server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Client {
    pub(crate) pid: u32,
    /// The name of the process that started the server, such as `claude`.
    pub(crate) agent: Option<String>,
    /// The last part of that process's working directory.
    pub(crate) project: Option<String>,
    /// The server's binary was replaced since it started, as by an update: it keeps the
    /// old build until its agent restarts it.
    pub(crate) replaced: bool,
}

/// Every `niri-computer-use serve` under `proc_root` for `instance`, niri's socket name,
/// by PID. A server whose environment names no niri socket found one itself, and counts.
pub(crate) fn scan(proc_root: &Path, instance: &str) -> Vec<Client> {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut clients: Vec<Client> = entries
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .filter_map(|pid| client(proc_root, pid, instance))
        .collect();
    clients.sort_by_key(|client| client.pid);
    clients
}

fn client(proc_root: &Path, pid: u32, instance: &str) -> Option<Client> {
    let dir = proc_root.join(pid.to_string());
    let exe = std::fs::read_link(dir.join("exe")).ok()?;
    let exe = exe.to_str()?;
    let (path, replaced) = exe
        .strip_suffix(DELETED)
        .map_or((exe, false), |path| (path, true));
    if Path::new(path).file_name()? != "niri-computer-use" || !serves(&dir)? {
        return None;
    }
    let socket = procs::environ_var(proc_root, pid, "NIRI_SOCKET").ok()?;
    if socket.is_some_and(|socket| Path::new(&socket).file_name() != Some(instance.as_ref())) {
        return None;
    }
    let parent = parent_pid(&std::fs::read_to_string(dir.join("status")).ok()?)?;
    Some(Client {
        pid,
        agent: procs::comm(proc_root, parent),
        project: project(proc_root, parent),
        replaced,
    })
}

/// Whether the process was started as `<program> serve`.
fn serves(dir: &Path) -> Option<bool> {
    let cmdline = std::fs::read(dir.join("cmdline")).ok()?;
    let mut args = cmdline.split(|&byte| byte == 0).skip(1);
    Some(args.next() == Some(b"serve".as_slice()))
}

/// The last part of the working directory of the process `pid`.
pub(crate) fn project(proc_root: &Path, pid: u32) -> Option<String> {
    let cwd = std::fs::read_link(proc_root.join(pid.to_string()).join("cwd")).ok()?;
    Some(cwd.file_name()?.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    struct Fake {
        root: PathBuf,
    }

    impl Fake {
        /// A process running `exe` as `<program> <command>`, with no niri socket.
        fn process(&self, pid: u32, parent: u32, exe: &str, command: &str) {
            let dir = self.root.join(pid.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            std::os::unix::fs::symlink(exe, dir.join("exe")).unwrap();
            std::fs::write(dir.join("cmdline"), format!("program\0{command}\0")).unwrap();
            std::fs::write(dir.join("environ"), "HOME=/home/u\0").unwrap();
            let status = format!("Pid:\t{pid}\nPPid:\t{parent}\n");
            std::fs::write(dir.join("status"), status).unwrap();
        }

        fn socket(&self, pid: u32, socket: &str) {
            let environ = format!("NIRI_SOCKET={socket}\0");
            std::fs::write(self.root.join(pid.to_string()).join("environ"), environ).unwrap();
        }

        fn agent(&self, pid: u32, comm: &str, cwd: &str) {
            let dir = self.root.join(pid.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("comm"), format!("{comm}\n")).unwrap();
            std::os::unix::fs::symlink(cwd, dir.join("cwd")).unwrap();
        }
    }

    #[test]
    fn lists_this_instances_servers_with_their_agent_and_whether_an_update_replaced_them() {
        let dir = crate::test_support::fresh_dir("watch-clients");
        let fake = Fake {
            root: dir.join("proc"),
        };
        let bin = "/home/u/.local/bin/niri-computer-use";
        let ours = "/run/user/1000/niri.wayland-1.9.sock";
        fake.agent(100, "claude", "/home/u/projects/shop");
        fake.agent(200, "pi", "/home/u/site");
        fake.process(4205, 100, &format!("{bin} (deleted)"), "serve");
        fake.socket(4205, ours);
        fake.process(5585, 200, bin, "serve");
        // Its engine, a server for another niri, and a program that only shares the name's start.
        fake.process(4208, 4205, bin, "engine");
        fake.socket(4208, ours);
        fake.process(7000, 100, bin, "serve");
        fake.socket(7000, "/tmp/niri.nested.1.sock");
        fake.process(7100, 100, "/usr/bin/niri-computer-use-helper", "serve");
        fake.socket(7100, ours);

        let clients = scan(&fake.root, "niri.wayland-1.9.sock");

        assert_eq!(
            clients,
            [
                Client {
                    pid: 4205,
                    agent: Some("claude".into()),
                    project: Some("shop".into()),
                    replaced: true,
                },
                Client {
                    pid: 5585,
                    agent: Some("pi".into()),
                    project: Some("site".into()),
                    replaced: false,
                },
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
