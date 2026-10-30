//! `install.sh` against fake releases served over HTTP from a directory of the test's own.
//! The script runs with a home inside that directory, whose name has a space, as `HOME`, the
//! directory's `tmp` as `TMPDIR`, and a fake `uname` that names the machine, so nothing here
//! reaches the real home directory or the network.
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
    use tokio::io::AsyncWriteExt as _;

    const LATEST: &str = "0.1.0";
    /// A release from before the Noctalia plugin, whose tarball has none.
    const BEFORE_PLUGIN: &str = "0.0.1";
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
            for sub in ["home dir", "tmp", "fakes", "releases", "stage"] {
                std::fs::create_dir_all(dir.join(sub)).unwrap();
            }
            let release = Self { dir };
            release.fake(
                "uname",
                &format!("case $1 in -s) echo Linux ;; -m) echo {arch} ;; esac\n"),
            );
            release
        }

        fn home(&self) -> PathBuf {
            self.dir.join("home dir")
        }

        /// A program the script finds on `PATH` before the system's.
        fn fake(&self, program: &str, body: &str) {
            write_program(&self.dir.join("fakes").join(program), body);
        }

        /// Publishes `version`'s tarball for `arch` and its `.sha256`, which is `sha256` of
        /// the tarball's own bytes when `None`.
        fn publish(&self, version: &str, arch: &str, sha256: Option<&str>) {
            let binary = format!("echo 'niri-computer-use {version}'\n");
            self.publish_binary(version, arch, &binary, sha256);
        }

        /// Publishes `version` for `arch` with `binary` as the program's body.
        fn publish_binary(&self, version: &str, arch: &str, binary: &str, sha256: Option<&str>) {
            let name = format!("niri-computer-use-{version}-{arch}-linux");
            let tree = self.dir.join("stage").join(&name);
            std::fs::create_dir_all(tree.join("skills/niri-computer-use/references")).unwrap();
            write_program(&tree.join("niri-computer-use"), binary);
            std::fs::write(tree.join("skills/niri-computer-use/SKILL.md"), version).unwrap();
            std::fs::write(
                tree.join("skills/niri-computer-use/references/tools.md"),
                "tools",
            )
            .unwrap();
            if version != BEFORE_PLUGIN {
                let plugin = tree.join("noctalia/niri-computer-use");
                std::fs::create_dir_all(&plugin).unwrap();
                std::fs::write(plugin.join("plugin.toml"), version).unwrap();
            }
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
            self.run(script(), env, None).await
        }

        /// Runs the script as `curl … | sh` does: `sh` reads it from stdin, which is the
        /// first `length` bytes of it when the download was cut short.
        async fn install_piped(&self, env: &[(&str, &str)], length: Option<usize>) -> Output {
            let mut text = std::fs::read(script_path()).unwrap();
            if let Some(length) = length {
                text.truncate(length);
            }
            self.run(shell(), env, Some(text)).await
        }

        /// Runs `command`, a shell command line in which `SCRIPT` names the script, on a
        /// pseudo-terminal of its own through util-linux `script`, typing `answers` into it.
        async fn install_on_terminal(&self, command: &str, answers: &[u8]) -> Output {
            let line = command.replace("SCRIPT", &format!("'{}'", script_path().display()));
            let mut terminal = terminal();
            terminal.args(["-qec", &line, "/dev/null"]);
            self.run(terminal, &[], Some(answers.to_vec())).await
        }

        async fn run(
            &self,
            mut command: tokio::process::Command,
            env: &[(&str, &str)],
            stdin: Option<Vec<u8>>,
        ) -> Output {
            let releases = serve(self.dir.join("releases"));
            let fakes = self.dir.join("fakes");
            let path = format!("{}:/usr/local/bin:/usr/bin:/bin", fakes.display());
            command
                .env_clear()
                .env("PATH", path)
                .env("HOME", self.home())
                .env("TMPDIR", self.dir.join("tmp"))
                .env("NCU_RELEASES_URL", releases)
                .current_dir(&self.dir)
                .envs(env.iter().copied())
                .stdin(if stdin.is_some() {
                    Stdio::piped()
                } else {
                    Stdio::null()
                })
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            let mut child = command.spawn().unwrap();
            if let Some(bytes) = stdin {
                feed(child.stdin.take().unwrap(), bytes);
            }
            tokio::time::timeout(DEADLINE, child.wait_with_output())
                .await
                .unwrap()
                .unwrap()
        }

        /// A `PATH` with the fakes and every system program but `hidden`, such as one the
        /// host has installed and the test needs to be missing.
        fn path_without(&self, hidden: &str) -> String {
            let system = self.dir.join("system");
            std::fs::create_dir_all(&system).unwrap();
            let programs = ["/usr/local/bin", "/usr/bin", "/bin"]
                .into_iter()
                .filter_map(|dir| std::fs::read_dir(dir).ok())
                .flatten()
                .map(Result::unwrap)
                .filter(|program| program.file_name() != hidden);
            for program in programs {
                // The first directory's copy of a program wins, as on `PATH`.
                let linked =
                    std::os::unix::fs::symlink(program.path(), system.join(program.file_name()));
                let taken = linked
                    .as_ref()
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists);
                assert!(linked.is_ok() || taken, "{linked:?}");
            }
            format!("{}:{}", self.dir.join("fakes").display(), system.display())
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

    /// Writes `bytes` to a child's stdin and closes it. The shell may exit before it has read
    /// everything, which closes the pipe early.
    fn feed(mut pipe: tokio::process::ChildStdin, bytes: Vec<u8>) {
        tokio::spawn(async move { pipe.write_all(&bytes).await.ok() });
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the test runs the install script itself; it isn't part of the server"
    )]
    fn script() -> tokio::process::Command {
        tokio::process::Command::new(script_path())
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the test pipes the install script into sh, as the install page says to"
    )]
    fn shell() -> tokio::process::Command {
        tokio::process::Command::new("sh")
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the test gives the install script a terminal with util-linux script"
    )]
    fn terminal() -> tokio::process::Command {
        tokio::process::Command::new("script")
    }

    fn script_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh")
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
            "claude mcp add --scope user niri-computer-use -- \"{}\" serve",
            binary.display()
        );
        assert!(stdout.contains(&register), "{stdout}");
        assert!(stdout.contains(&format!("spawn \"{}\" \"stop\"", binary.display())));
        assert!(!stdout.contains("Replaced"), "{stdout}");
        assert!(release.tmp_is_empty());
    }

    /// A fake agent CLI that appends each call's arguments to `<HOME>/<program>-calls`, one
    /// per line and an empty line after the call, so a split or joined argument shows.
    /// `mcp get niri-computer-use` succeeds once `<HOME>/<program>-registered` exists, and
    /// `mcp add` creates it, or fails when `<HOME>/<program>-refuses` exists.
    fn fake_agent(release: &Release, program: &str) {
        release.fake(
            program,
            &format!(
                "printf '%s\\n' \"$@\" '' >> \"$HOME/{program}-calls\"\n\
                 case \"$1 $2\" in\n\
                 'mcp get') test -e \"$HOME/{program}-registered\" ;;\n\
                 'mcp add') if [ -e \"$HOME/{program}-refuses\" ]; then echo 'error: no'; exit 1; fi; : > \"$HOME/{program}-registered\" ;;\n\
                 *) exit 2 ;;\n\
                 esac\n"
            ),
        );
    }

    /// Each call's arguments, in order.
    fn calls(release: &Release, program: &str) -> Vec<Vec<String>> {
        let log = std::fs::read_to_string(release.home().join(format!("{program}-calls")))
            .unwrap_or_default();
        let mut calls = Vec::new();
        let mut call = Vec::new();
        for line in log.lines() {
            if line.is_empty() {
                calls.push(std::mem::take(&mut call));
            } else {
                call.push(line.to_owned());
            }
        }
        calls
    }

    /// The arguments of `mcp add`: `words` split at spaces, then the binary and `serve`.
    fn add_call(words: &str, binary: &Path) -> Vec<String> {
        let binary = binary.display().to_string();
        words
            .split(' ')
            .map(str::to_owned)
            .chain([binary, "serve".to_owned()])
            .collect()
    }

    #[tokio::test]
    async fn ncu_register_registers_in_every_agent_found_through_its_own_mcp_add() {
        let release = Release::new("register", "x86_64");
        release.publish(LATEST, "x86_64", None);
        fake_agent(&release, "claude");
        fake_agent(&release, "codex");

        let output = release.install(&[("NCU_REGISTER", "1")]).await;

        assert!(output.status.success(), "{output:?}");
        let binary = release.home().join(".local/bin/niri-computer-use");
        assert_eq!(
            calls(&release, "claude"),
            [
                vec![
                    "mcp".to_owned(),
                    "get".to_owned(),
                    "niri-computer-use".to_owned()
                ],
                add_call("mcp add --scope user niri-computer-use --", &binary),
            ]
        );
        assert_eq!(
            calls(&release, "codex"),
            [add_call("mcp add niri-computer-use --", &binary)]
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("Registered in Claude Code."), "{stdout}");
        assert!(stdout.contains("Registered in Codex."), "{stdout}");
        assert!(!stdout.contains("claude mcp add"), "{stdout}");
        // It suggests the policy key and leaves the user's config alone.
        let policy = release.home().join(".config/niri-computer-use/policy.toml");
        assert!(
            stdout.contains(&format!(
                "add this line to {}:\n       launch_desktop_entries = true",
                policy.display()
            )),
            "{stdout}"
        );
        assert!(!policy.parent().unwrap().exists());
        assert!(!stdout.contains("pi mcp add"), "{stdout}");
    }

    #[tokio::test]
    async fn piped_into_sh_it_installs_and_registers_even_when_an_agent_reads_stdin() {
        let release = Release::new("piped", "x86_64");
        release.publish(LATEST, "x86_64", None);
        fake_agent(&release, "claude");
        // An agent's `mcp add` that reads stdin must not get the rest of the script.
        release.fake("pi", "cat > \"$HOME/pi-stdin\"\n");
        fake_agent(&release, "codex");

        let output = release.install_piped(&[("NCU_REGISTER", "1")], None).await;

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            std::fs::read_to_string(release.home().join("pi-stdin")).unwrap(),
            ""
        );
        assert!(
            calls(&release, "codex")
                .iter()
                .any(|call| call.starts_with(&["mcp".to_owned(), "add".to_owned()]))
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains("Registered in Codex."), "{stdout}");
        assert!(stdout.contains("Bind the stop key"), "{stdout}");
    }

    #[tokio::test]
    async fn piped_on_a_terminal_it_asks_before_registering() {
        let release = Release::new("asks", "x86_64");
        release.publish(LATEST, "x86_64", None);
        fake_agent(&release, "claude");

        let output = release.install_on_terminal("cat SCRIPT | sh", b"y\n").await;

        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Register it in Claude Code now"),
            "{stdout}"
        );
        assert!(stdout.contains("Registered in Claude Code."), "{stdout}");
    }

    #[tokio::test]
    async fn on_a_terminal_it_cannot_open_it_registers_in_none_and_finishes() {
        let release = Release::new("detached", "x86_64");
        release.publish(LATEST, "x86_64", None);
        fake_agent(&release, "claude");

        // Output goes to a terminal, but the script's new session has no controlling one.
        let output = release
            .install_on_terminal("setsid -w sh SCRIPT", b"")
            .await;

        assert!(output.status.success(), "{output:?}");
        assert_eq!(calls(&release, "claude"), Vec::<Vec<String>>::new());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("claude mcp add --scope user"), "{stdout}");
        assert!(stdout.contains("Bind the stop key"), "{stdout}");
    }

    #[tokio::test]
    async fn a_download_cut_short_and_piped_into_sh_installs_nothing() {
        let release = Release::new("cut", "x86_64");
        release.publish(LATEST, "x86_64", None);
        let length = std::fs::read(script_path()).unwrap().len() * 2 / 3;

        let output = release.install_piped(&[], Some(length)).await;

        assert!(!output.status.success(), "{output:?}");
        assert!(!release.home().join(".local").exists());
    }

    #[tokio::test]
    async fn an_agent_already_registered_is_left_as_it_is() {
        let release = Release::new("registered", "x86_64");
        release.publish(LATEST, "x86_64", None);
        fake_agent(&release, "claude");
        std::fs::write(release.home().join("claude-registered"), "").unwrap();

        let output = release.install(&[("NCU_REGISTER", "1")]).await;

        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            calls(&release, "claude"),
            [["mcp", "get", "niri-computer-use"]]
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains("Already registered in Claude Code"),
            "{stdout}"
        );
    }

    #[tokio::test]
    async fn an_agent_that_refuses_does_not_fail_the_install_and_its_command_is_printed() {
        let release = Release::new("refuses", "x86_64");
        release.publish(LATEST, "x86_64", None);
        fake_agent(&release, "pi");
        std::fs::write(release.home().join("pi-refuses"), "").unwrap();

        let output = release.install(&[("NCU_REGISTER", "1")]).await;

        assert!(output.status.success(), "{output:?}");
        let binary = release.home().join(".local/bin/niri-computer-use");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("Registering in Pi failed: error: no"),
            "{stderr}"
        );
        let stdout = String::from_utf8(output.stdout).unwrap();
        let command = format!(
            "pi mcp add niri-computer-use --exposure direct -- \"{}\" serve",
            binary.display()
        );
        assert!(stdout.contains(&command), "{stdout}");
        assert!(!stdout.contains("Registered in"), "{stdout}");
    }

    #[tokio::test]
    async fn without_a_terminal_or_ncu_register_no_agent_is_touched_and_its_command_is_printed() {
        let release = Release::new("quiet", "x86_64");
        release.publish(LATEST, "x86_64", None);
        fake_agent(&release, "claude");
        fake_agent(&release, "pi");

        let unset = release.install(&[]).await;
        let off = release.install(&[("NCU_REGISTER", "0")]).await;

        assert!(unset.status.success(), "{unset:?}");
        assert!(off.status.success(), "{off:?}");
        assert_eq!(calls(&release, "claude"), Vec::<Vec<String>>::new());
        assert_eq!(calls(&release, "pi"), Vec::<Vec<String>>::new());
        let binary = release.home().join(".local/bin/niri-computer-use");
        for output in [unset, off] {
            let stdout = String::from_utf8(output.stdout).unwrap();
            assert!(
                stdout.contains(&format!(
                    "claude mcp add --scope user niri-computer-use -- \"{}\" serve",
                    binary.display()
                )),
                "{stdout}"
            );
            assert!(
                stdout.contains("pi mcp add niri-computer-use --exposure direct"),
                "{stdout}"
            );
            assert!(!stdout.contains("codex mcp add"), "{stdout}");
        }
    }

    /// The agents after Claude Code, Pi and Codex, each with the arguments its `mcp add`
    /// takes for a stdio server in the user's config, whatever the working directory.
    const MORE_AGENTS: [(&str, &str, &str); 4] = [
        ("amp", "Amp", "mcp add niri-computer-use --"),
        (
            "copilot",
            "GitHub Copilot CLI",
            "mcp add niri-computer-use --",
        ),
        (
            "gemini",
            "Gemini CLI",
            "mcp add --scope user niri-computer-use",
        ),
        (
            "qwen",
            "Qwen Code",
            "mcp add --scope user niri-computer-use",
        ),
    ];

    #[tokio::test]
    async fn amp_copilot_gemini_and_qwen_are_registered_at_user_scope() {
        let release = Release::new("more-agents", "x86_64");
        release.publish(LATEST, "x86_64", None);
        for (program, _, _) in MORE_AGENTS {
            fake_agent(&release, program);
        }

        let output = release.install(&[("NCU_REGISTER", "1")]).await;

        assert!(output.status.success(), "{output:?}");
        let binary = release.home().join(".local/bin/niri-computer-use");
        let stdout = String::from_utf8(output.stdout).unwrap();
        for (program, agent, add) in MORE_AGENTS {
            assert_eq!(
                calls(&release, program),
                [add_call(add, &binary)],
                "{program}"
            );
            assert!(
                stdout.contains(&format!("Registered in {agent}.")),
                "{stdout}"
            );
            assert!(!stdout.contains(&format!("{program} mcp add")), "{stdout}");
        }
    }

    #[tokio::test]
    async fn amp_copilot_gemini_and_qwen_are_left_alone_when_declined() {
        let release = Release::new("more-declined", "x86_64");
        release.publish(LATEST, "x86_64", None);
        for (program, _, _) in MORE_AGENTS {
            fake_agent(&release, program);
        }

        let output = release.install(&[("NCU_REGISTER", "0")]).await;

        assert!(output.status.success(), "{output:?}");
        let binary = release.home().join(".local/bin/niri-computer-use");
        let stdout = String::from_utf8(output.stdout).unwrap();
        for (program, _, add) in MORE_AGENTS {
            assert_eq!(
                calls(&release, program),
                Vec::<Vec<String>>::new(),
                "{program}"
            );
            let command = format!("{program} {add} \"{}\" serve", binary.display());
            assert!(stdout.contains(&command), "{stdout}");
        }
        assert!(!stdout.contains("Registered in"), "{stdout}");
    }

    #[tokio::test]
    async fn an_unknown_ncu_register_value_is_refused_before_anything_is_registered() {
        let release = Release::new("bad-register", "x86_64");
        release.publish(LATEST, "x86_64", None);
        fake_agent(&release, "claude");

        let output = release.install(&[("NCU_REGISTER", "maybe")]).await;

        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("NCU_REGISTER must be 1 or 0"), "{stderr}");
        assert_eq!(calls(&release, "claude"), Vec::<Vec<String>>::new());
    }

    #[tokio::test]
    async fn installs_the_named_version_for_aarch64_into_a_relative_directory() {
        let release = Release::new("named", "aarch64");
        release.publish("0.0.9", "aarch64", None);
        release.publish(LATEST, "aarch64", None);

        let output = release
            .install(&[("NCU_VERSION", "v0.0.9"), ("NCU_BIN_DIR", "tools")])
            .await;

        assert!(output.status.success(), "{output:?}");
        let binary = release.dir.join("tools/niri-computer-use");
        assert_eq!(version_of(&binary), "niri-computer-use 0.0.9\n");
        assert!(!release.home().join(".local/bin").exists());
        // The printed steps name the binary by its absolute path, which niri's `spawn` needs.
        let stdout = String::from_utf8(output.stdout).unwrap();
        let status = format!("\"{}\" status", binary.display());
        assert!(stdout.contains(&status), "{stdout}");
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

    #[tokio::test]
    async fn a_binary_that_does_not_run_here_installs_nothing() {
        let release = Release::new("broken", "x86_64");
        let binary = "echo 'version GLIBC_2.39 not found' >&2\nexit 1\n";
        release.publish_binary(LATEST, "x86_64", binary, None);

        let output = release.install(&[]).await;

        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("version GLIBC_2.39 not found"), "{stderr}");
        assert!(stderr.contains("exited with status 1"), "{stderr}");
        assert!(stderr.contains("nothing was installed"), "{stderr}");
        assert!(!release.home().join(".local").exists());
        assert!(release.tmp_is_empty());
    }

    #[tokio::test]
    async fn a_release_without_its_checksum_installs_nothing() {
        let release = Release::new("unsummed", "x86_64");
        release.publish(LATEST, "x86_64", None);
        let checksum = format!("niri-computer-use-{LATEST}-x86_64-linux.tar.gz.sha256");
        std::fs::remove_file(release.dir.join("releases/download/v0.1.0").join(checksum)).unwrap();

        let output = release.install(&[]).await;

        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains("404"), "{stderr}");
        assert!(!release.home().join(".local").exists());
        assert!(release.tmp_is_empty());
    }

    #[tokio::test]
    async fn a_copy_that_fails_keeps_the_previous_install_whole() {
        let release = Release::new("full", "x86_64");
        release.publish("0.0.9", "x86_64", None);
        release.publish(LATEST, "x86_64", None);
        let older = release.install(&[("NCU_VERSION", "0.0.9")]).await;
        assert!(older.status.success(), "{older:?}");
        // The skill's copy fails, as on a full disk, after the binary's copy succeeded.
        release.fake(
            "cp",
            "case $1 in -R) echo 'cp: No space left on device' >&2; exit 1 ;; esac\nexec /bin/cp \"$@\"\n",
        );

        let output = release.install(&[]).await;

        assert!(!output.status.success());
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(
            stderr.contains("what was installed before is unchanged"),
            "{stderr}"
        );
        let home = release.home();
        let binary = home.join(".local/bin/niri-computer-use");
        assert_eq!(version_of(&binary), "niri-computer-use 0.0.9\n");
        let skills = home.join(".local/share/niri-computer-use/skills");
        let link = home.join(".claude/skills/niri-computer-use");
        assert_eq!(
            std::fs::read_to_string(link.join("SKILL.md")).unwrap(),
            "0.0.9"
        );
        assert!(!home.join(".local/bin/.niri-computer-use.new").exists());
        assert!(!skills.join(".niri-computer-use.new").exists());
        assert!(release.tmp_is_empty());
    }

    #[tokio::test]
    async fn a_leftover_directory_at_the_staging_name_is_not_installed_as_the_binary() {
        let release = Release::new("leftover", "x86_64");
        release.publish(LATEST, "x86_64", None);
        let bin = release.home().join(".local/bin");
        std::fs::create_dir_all(bin.join(".niri-computer-use.new")).unwrap();

        let output = release.install(&[]).await;

        assert!(output.status.success(), "{output:?}");
        let binary = bin.join("niri-computer-use");
        assert!(binary.is_file());
        assert_eq!(version_of(&binary), "niri-computer-use 0.1.0\n");
        assert!(!bin.join(".niri-computer-use.new").exists());
    }

    #[tokio::test]
    async fn a_link_the_user_made_is_replaced_with_a_notice_and_a_directory_left_alone() {
        let release = Release::new("theirs", "x86_64");
        release.publish(LATEST, "x86_64", None);
        let home = release.home();
        let clone = release.dir.join("clone/skills/niri-computer-use");
        std::fs::create_dir_all(&clone).unwrap();
        std::fs::write(clone.join("SKILL.md"), "from the clone").unwrap();
        let claude = home.join(".claude/skills/niri-computer-use");
        std::fs::create_dir_all(claude.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&clone, &claude).unwrap();
        let agents = home.join(".agents/skills/niri-computer-use");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(agents.join("SKILL.md"), "their own").unwrap();

        let output = release.install(&[]).await;

        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        let skill = home.join(".local/share/niri-computer-use/skills/niri-computer-use");
        assert_eq!(std::fs::read_link(&claude).unwrap(), skill);
        let replaced = format!(
            "Replaced {}, which pointed to {}.",
            claude.display(),
            clone.display()
        );
        assert!(stdout.contains(&replaced), "{stdout}");
        assert_eq!(
            std::fs::read_to_string(clone.join("SKILL.md")).unwrap(),
            "from the clone"
        );
        assert!(!agents.is_symlink());
        assert_eq!(
            std::fs::read_to_string(agents.join("SKILL.md")).unwrap(),
            "their own"
        );
        assert!(
            stdout.contains(&format!("Left {} as it is", agents.display())),
            "{stdout}"
        );
    }

    #[tokio::test]
    async fn the_noctalia_plugin_is_linked_into_noctalia_only_when_noctalia_is_on_path() {
        let release = Release::new("plugin", "x86_64");
        release.publish(LATEST, "x86_64", None);
        let home = release.home();
        let plugin = home.join(".local/share/niri-computer-use/noctalia/niri-computer-use");
        let link = home.join(".local/share/noctalia/plugins/niri-computer-use");
        let enable = "noctalia msg plugins enable ayagmar/niri-computer-use";

        let without = release
            .install(&[("PATH", &release.path_without("noctalia"))])
            .await;

        assert!(without.status.success(), "{without:?}");
        assert_eq!(
            std::fs::read_to_string(plugin.join("plugin.toml")).unwrap(),
            LATEST
        );
        assert!(!link.exists() && !link.is_symlink());
        assert!(!String::from_utf8(without.stdout).unwrap().contains(enable));

        release.fake("noctalia", "exit 0\n");
        let with = release.install(&[]).await;

        assert!(with.status.success(), "{with:?}");
        assert_eq!(std::fs::read_link(&link).unwrap(), plugin);
        let stdout = String::from_utf8(with.stdout).unwrap();
        assert!(stdout.contains(enable), "{stdout}");
        assert!(!stdout.contains(INCOMPATIBLE), "{stdout}");
    }

    const INCOMPATIBLE: &str = "it needs Noctalia 5.4 or newer";

    // The bug: an install on Noctalia 5.3, which refuses the plugin's API, that said to
    // enable a plugin Noctalia won't load.
    #[tokio::test]
    async fn an_install_says_when_noctalia_lists_the_plugin_as_incompatible() {
        let release = Release::new("incompatible", "x86_64");
        release.publish(LATEST, "x86_64", None);
        release.fake(
            "noctalia",
            "[ \"$*\" = 'msg plugins list' ] || exit 0\n\
             echo 'other/plugin [local] 1.0 enabled'\n\
             echo 'ayagmar/niri-computer-use [local] 0.1.0 disabled incompatible'\n",
        );

        let output = release.install(&[]).await;

        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(stdout.contains(INCOMPATIBLE), "{stdout}");
    }

    #[tokio::test]
    async fn a_release_without_the_plugin_leaves_the_installed_one_in_place() {
        let release = Release::new("noplugin", "x86_64");
        release.publish(LATEST, "x86_64", None);
        release.publish(BEFORE_PLUGIN, "x86_64", None);
        let newer = release.install(&[]).await;
        assert!(newer.status.success(), "{newer:?}");

        let output = release.install(&[("NCU_VERSION", BEFORE_PLUGIN)]).await;

        assert!(output.status.success(), "{output:?}");
        let plugin = release
            .home()
            .join(".local/share/niri-computer-use/noctalia/niri-computer-use");
        assert_eq!(
            std::fs::read_to_string(plugin.join("plugin.toml")).unwrap(),
            LATEST
        );
    }

    /// Runs `binary serve` from the release's directory, where `serve` is a FIFO, so a
    /// copy of `cat` as the binary waits on it like a server waits on its client.
    #[expect(
        clippy::disallowed_methods,
        reason = "the test stands a server in for an agent's; it isn't part of the server"
    )]
    fn waiting_server(release: &Release, binary: &Path) -> std::process::Child {
        run_tool("mkfifo", &[release.dir.join("serve").as_os_str()]);
        std::process::Command::new(binary)
            .arg("serve")
            .current_dir(&release.dir)
            .spawn()
            .unwrap()
    }

    #[tokio::test]
    async fn a_server_left_on_the_replaced_build_is_named_with_its_agent() {
        let release = Release::new("leftover", "x86_64");
        release.publish(LATEST, "x86_64", None);
        let binary = release.home().join(".local/bin/niri-computer-use");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::copy("/bin/cat", &binary).unwrap();
        let mut server = waiting_server(&release, &binary);

        let output = release.install(&[]).await;

        server.kill().unwrap();
        server.wait().unwrap();
        assert!(output.status.success(), "{output:?}");
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert!(
            stdout.contains("These servers still run the build this install replaced"),
            "{stdout}"
        );
        let agent = std::fs::read_to_string("/proc/self/comm").unwrap();
        let named = format!("PID {}, started by {}", server.id(), agent.trim_end());
        assert!(stdout.contains(&named), "{stdout}");
    }

    #[tokio::test]
    async fn a_linked_binary_is_replaced_by_a_file_and_its_target_kept() {
        let release = Release::new("linked", "x86_64");
        release.publish(LATEST, "x86_64", None);
        let elsewhere = release.dir.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        write_program(&elsewhere.join("niri-computer-use"), "echo old\n");
        let binary = release.home().join(".local/bin/niri-computer-use");
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("niri-computer-use"), &binary).unwrap();

        let output = release.install(&[]).await;

        assert!(output.status.success(), "{output:?}");
        assert!(!binary.is_symlink());
        assert_eq!(version_of(&binary), "niri-computer-use 0.1.0\n");
        assert_eq!(version_of(&elsewhere.join("niri-computer-use")), "old\n");
    }
}
