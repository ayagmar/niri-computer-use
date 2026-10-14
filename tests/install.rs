//! `install.sh` against fake releases served over HTTP from a directory of the test's own.
//! The script runs with that directory as `HOME` and `TMPDIR`, and a fake `uname` that
//! names the machine, so nothing here reaches the real home directory or the network.
//!
//! Everything is `cfg(test)`, which always holds here, so Clippy treats the helpers as
//! test code too.

#[cfg(test)]
mod script {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::os::unix::fs::PermissionsExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{Output, Stdio};
    use std::time::Duration;

    const LATEST: &str = "0.1.0";
    const DEADLINE: Duration = Duration::from_secs(60);

    struct Release {
        dir: PathBuf,
    }

    impl Release {
        /// A home, a temporary directory, a fake `uname` for `arch`, and a release server.
        fn new(name: &str, arch: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = std::env::temp_dir()
                .join(format!("ncu-install-{name}-{}-{nanos}", std::process::id()));
            for sub in ["home", "tmp", "fakes", "releases", "stage"] {
                std::fs::create_dir_all(dir.join(sub)).unwrap();
            }
            write_program(
                &dir.join("fakes/uname"),
                &format!("case $1 in -s) echo Linux ;; -m) echo {arch} ;; esac\n"),
            );
            Self { dir }
        }

        fn home(&self) -> PathBuf {
            self.dir.join("home")
        }

        /// Publishes `version`'s tarball for `arch` and its `.sha256`, which is `sha256` of
        /// the tarball's own bytes when `None`.
        fn publish(&self, version: &str, arch: &str, sha256: Option<&str>) {
            let name = format!("niri-computer-use-{version}-{arch}-linux");
            let tree = self.dir.join("stage").join(&name);
            std::fs::create_dir_all(tree.join("skills/niri-computer-use/references")).unwrap();
            write_program(
                &tree.join("niri-computer-use"),
                &format!("echo 'niri-computer-use {version}'\n"),
            );
            std::fs::write(tree.join("skills/niri-computer-use/SKILL.md"), version).unwrap();
            std::fs::write(
                tree.join("skills/niri-computer-use/references/tools.md"),
                "tools",
            )
            .unwrap();
            let download = self
                .dir
                .join("releases/download")
                .join(format!("v{version}"));
            std::fs::create_dir_all(&download).unwrap();
            let tarball = download.join(format!("{name}.tar.gz"));
            run_tool(
                "tar",
                &[
                    "-C".as_ref(),
                    self.dir.join("stage").as_os_str(),
                    "-czf".as_ref(),
                    tarball.as_os_str(),
                    name.as_ref(),
                ],
            );
            let sum = sha256.map_or_else(|| sha256_of(&tarball), str::to_owned);
            let line = format!("{sum}  {name}.tar.gz\n");
            std::fs::write(download.join(format!("{name}.tar.gz.sha256")), line).unwrap();
        }

        /// Runs the script with `env` on top of the test's environment.
        async fn install(&self, env: &[(&str, &str)]) -> Output {
            let releases = serve(self.dir.join("releases"));
            let path = format!("{}:/usr/local/bin:/usr/bin:/bin", self.path("fakes"));
            let mut command = script();
            command
                .env_clear()
                .env("PATH", path)
                .env("HOME", self.home())
                .env("TMPDIR", self.dir.join("tmp"))
                .env("NCU_RELEASES_URL", releases)
                .envs(env.iter().copied())
                .stdin(Stdio::null())
                .kill_on_drop(true);
            tokio::time::timeout(DEADLINE, command.output())
                .await
                .unwrap()
                .unwrap()
        }

        fn path(&self, relative: &str) -> String {
            self.dir.join(relative).display().to_string()
        }

        /// Whether the script left nothing in its temporary directory.
        fn tmp_is_empty(&self) -> bool {
            std::fs::read_dir(self.dir.join("tmp"))
                .unwrap()
                .next()
                .is_none()
        }
    }

    impl Drop for Release {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the test runs the install script itself; it isn't part of the server"
    )]
    fn script() -> tokio::process::Command {
        tokio::process::Command::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh"))
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the test builds its fake release with the system's tar and sha256sum"
    )]
    fn run_tool(program: &str, args: &[&std::ffi::OsStr]) -> Output {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{program}: {output:?}");
        output
    }

    fn sha256_of(file: &Path) -> String {
        let output = run_tool("sha256sum", &[file.as_os_str()]);
        let line = String::from_utf8(output.stdout).unwrap();
        line.split_whitespace().next().unwrap().to_owned()
    }

    fn version_of(binary: &Path) -> String {
        let output = run_tool(
            binary.as_os_str().to_str().unwrap(),
            &["--version".as_ref()],
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn write_program(path: &Path, body: &str) {
        std::fs::write(path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Serves `root` over HTTP until the test process ends, with `/latest` redirecting to the
    /// latest release's page as GitHub does, and returns the base URL.
    fn serve(root: PathBuf) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}/releases", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                answer(&root, stream.unwrap());
            }
        });
        base
    }

    fn answer(root: &Path, mut stream: TcpStream) {
        let mut request = String::new();
        let mut reader = BufReader::new(&stream);
        reader.read_line(&mut request).unwrap();
        let mut header = String::new();
        while reader.read_line(&mut header).unwrap() > 2 {
            header.clear();
        }
        let path = request.split(' ').nth(1).unwrap_or_default();
        let relative = path.strip_prefix("/releases/").unwrap_or_default();
        let (status, headers, body) = if relative == "latest" {
            let location = format!("Location: /releases/tag/v{LATEST}\r\n");
            ("302 Found", location, Vec::new())
        } else if relative.starts_with("tag/") {
            ("200 OK", String::new(), b"release page".to_vec())
        } else {
            std::fs::read(root.join(relative)).map_or_else(
                |_| ("404 Not Found", String::new(), Vec::new()),
                |bytes| ("200 OK", String::new(), bytes),
            )
        };
        let head = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(&body).unwrap();
    }

    #[tokio::test]
    async fn installs_the_latest_release_and_links_its_skill_for_both_agents() {
        let release = Release::new("latest", "x86_64");
        release.publish("0.0.9", "x86_64", None);
        release.publish(LATEST, "x86_64", None);

        let output = release.install(&[]).await;

        assert!(output.status.success(), "{output:?}");
        let home = release.home();
        let binary = home.join(".local/bin/niri-computer-use");
        assert_eq!(version_of(&binary), "niri-computer-use 0.1.0\n");
        let skill = home.join(".local/share/niri-computer-use/skills/niri-computer-use");
        for agent in [".claude", ".agents"] {
            let link = home.join(agent).join("skills/niri-computer-use");
            assert_eq!(std::fs::read_link(&link).unwrap(), skill);
            assert_eq!(
                std::fs::read_to_string(link.join("SKILL.md")).unwrap(),
                LATEST
            );
            assert!(link.join("references/tools.md").is_file());
        }
        let stdout = String::from_utf8(output.stdout).unwrap();
        let register = format!(
            "claude mcp add --scope user niri-computer-use -- {} serve",
            binary.display()
        );
        assert!(stdout.contains(&register), "{stdout}");
        assert!(stdout.contains(&format!("spawn \"{}\" \"stop\"", binary.display())));
        assert!(release.tmp_is_empty());
    }

    #[tokio::test]
    async fn installs_the_named_version_for_aarch64_into_the_named_directory() {
        let release = Release::new("named", "aarch64");
        release.publish("0.0.9", "aarch64", None);
        release.publish(LATEST, "aarch64", None);
        let bin_dir = release.path("tools");

        let output = release
            .install(&[("NCU_VERSION", "v0.0.9"), ("NCU_BIN_DIR", &bin_dir)])
            .await;

        assert!(output.status.success(), "{output:?}");
        let binary = Path::new(&bin_dir).join("niri-computer-use");
        assert_eq!(version_of(&binary), "niri-computer-use 0.0.9\n");
        assert!(!release.home().join(".local/bin").exists());
    }

    #[tokio::test]
    async fn an_update_replaces_the_skill_and_the_links_it_made() {
        let release = Release::new("update", "x86_64");
        release.publish("0.0.9", "x86_64", None);
        release.publish(LATEST, "x86_64", None);
        let older = release.install(&[("NCU_VERSION", "0.0.9")]).await;
        assert!(older.status.success(), "{older:?}");
        let skill = release
            .home()
            .join(".local/share/niri-computer-use/skills/niri-computer-use");
        std::fs::write(skill.join("removed-upstream.md"), "stale").unwrap();

        let output = release.install(&[]).await;

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            std::fs::read_to_string(skill.join("SKILL.md")).unwrap(),
            LATEST
        );
        assert!(!skill.join("removed-upstream.md").exists());
        let link = release.home().join(".claude/skills/niri-computer-use");
        assert_eq!(std::fs::read_link(link).unwrap(), skill);
    }

    #[tokio::test]
    async fn a_tarball_that_does_not_match_its_checksum_installs_nothing() {
        let release = Release::new("mismatch", "x86_64");
        release.publish(LATEST, "x86_64", Some(&"0".repeat(64)));

        let output = release.install(&[]).await;

        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("doesn't match its .sha256; nothing was installed"),
            "{stderr}"
        );
        let home = release.home();
        assert!(!home.join(".local").exists());
        assert!(!home.join(".claude").exists());
        assert!(release.tmp_is_empty());
    }

    #[tokio::test]
    async fn a_machine_without_a_release_build_is_refused() {
        let release = Release::new("arch", "riscv64");
        release.publish(LATEST, "riscv64", None);

        let output = release.install(&[]).await;

        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("there is no release build for riscv64"),
            "{stderr}"
        );
        assert!(!release.home().join(".local").exists());
    }
}
