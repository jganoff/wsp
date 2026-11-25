//! Exercise native Git HTTP and the real CLI through a controlling terminal.
#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};

// The renderer's clock cannot be injected into a separate product process.
// A server handshake holds real network work until observed frames release it.
// This deadline only bounds missing events; no assertion depends on speed.
const WATCHDOG: Duration = Duration::from_secs(30);
const USERNAME: &[u8] = b"fixture-user\r";
const PASSWORD: &[u8] = b"fixture-password\r";

fn isolated_command(program: impl AsRef<std::ffi::OsStr>, root: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .current_dir(root)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root)
        .env("XDG_DATA_HOME", root.join("data"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if root.join("gitconfig").is_file() {
                root.join("gitconfig")
            } else {
                PathBuf::from("/dev/null")
            },
        )
        .env("LC_ALL", "C")
        .env("TERM", "xterm-256color");
    command
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = isolated_command("git", root)
        .args(args)
        .stdin(Stdio::null())
        .output()?;
    ensure!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output.stdout)
}

fn fixture(root: &Path) -> Result<String> {
    git(
        root,
        &[
            "config",
            "--file",
            "gitconfig",
            "init.defaultBranch",
            "main",
        ],
    )?;
    git(
        root,
        &["config", "--file", "gitconfig", "fetch.unpackLimit", "1"],
    )?;
    git(
        root,
        &["config", "--file", "gitconfig", "pack.threads", "2"],
    )?;
    std::fs::create_dir_all(root.join("owner"))?;
    git(
        root,
        &[
            "init",
            "--bare",
            "--quiet",
            "--initial-branch=main",
            "owner/remote.git",
        ],
    )?;
    let remote = root.join("owner/remote.git");
    let tree = String::from_utf8(git(&remote, &["mktree"])?)?;
    let commit = String::from_utf8(git(
        &remote,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit-tree",
            tree.trim(),
            "-m",
            "fixture",
        ],
    )?)?;
    git(&remote, &["update-ref", "refs/heads/main", commit.trim()])?;
    let mirror = root.join("data/wsp/mirrors/127.0.0.1/owner/remote.git");
    std::fs::create_dir_all(mirror.parent().unwrap())?;
    git(
        root,
        &[
            "clone",
            "--mirror",
            "--quiet",
            "owner/remote.git",
            mirror.to_str().unwrap(),
        ],
    )?;
    let mut advanced = String::from_utf8(git(
        &remote,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit-tree",
            tree.trim(),
            "-p",
            commit.trim(),
            "-m",
            "advance real HTTP upstream",
        ],
    )?)?;
    for index in 0..15 {
        advanced = String::from_utf8(git(
            &remote,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit-tree",
                tree.trim(),
                "-p",
                advanced.trim(),
                "-m",
                &format!("fixture update {index}"),
            ],
        )?)?;
    }
    git(&remote, &["update-ref", "refs/heads/main", advanced.trim()])?;
    Ok(advanced.trim().to_owned())
}

fn register(root: &Path, url: &str) -> Result<()> {
    let mirror = root.join("data/wsp/mirrors/127.0.0.1/owner/remote.git");
    git(&mirror, &["remote", "set-url", "origin", url])?;
    wsp_core::config::Config {
        workspaces_dir: Some(root.join("workspaces").display().to_string()),
        hints: Some(false),
        repos: BTreeMap::from([(
            "127.0.0.1/owner/remote".to_owned(),
            wsp_core::config::RepoEntry {
                url: url.to_owned(),
                added: chrono::Utc::now(),
                setup_commands: None,
            },
        )]),
        ..Default::default()
    }
    .save_to(&root.join("data/wsp/config.yaml"))?;
    Ok(())
}

#[test]
fn http_terminal_native_git_baseline_preserves_prompt_input() -> Result<()> {
    let root = tempfile::tempdir()?;
    let expected = fixture(root.path())?;
    let server = HttpServer::start(root.path())?;
    let url = format!("http://127.0.0.1:{}/owner/remote.git", server.port);
    let mut terminal = Terminal::start_with(root.path(), Scenario::Authenticate, Some(&url))?;
    server.entered.recv_timeout(WATCHDOG)?;
    server.release.send(false)?;
    terminal
        .until(|bytes| visible_native_prompt(bytes, b"Username for '"))
        .context("native Git username prompt")?;
    terminal.input.write_all(USERNAME)?;
    terminal.input.flush()?;
    terminal
        .until(|bytes| visible_native_prompt(bytes, b"Password for '"))
        .context("native Git password prompt")?;
    terminal.input.write_all(PASSWORD)?;
    terminal.input.flush()?;
    let (status, bytes) = terminal.finish()?;
    ensure!(
        status.success(),
        "native Git fixture failed: {}",
        String::from_utf8_lossy(&bytes)
    );
    ensure!(
        server.finish()?.authenticated,
        "native Git was not authenticated"
    );
    ensure!(
        String::from_utf8_lossy(&bytes).contains(&expected),
        "native Git refs missing"
    );
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum Scenario {
    Authenticate,
    PromptDisabled,
    InterruptPassword,
    ReadFirstHelper,
    Json,
    JsonPromptDisabled,
}

#[test]
fn http_terminal_progress_preserves_native_transport_and_interaction() -> Result<()> {
    for scenario in [
        Scenario::Authenticate,
        Scenario::PromptDisabled,
        Scenario::InterruptPassword,
        Scenario::ReadFirstHelper,
    ] {
        let root = tempfile::tempdir()?;
        let expected = fixture(root.path())?;
        let server = HttpServer::start(root.path())?;
        let url = format!("http://127.0.0.1:{}/owner/remote.git", server.port);
        register(root.path(), &url)?;
        let helper_ready = if matches!(scenario, Scenario::ReadFirstHelper) {
            Some(install_read_first_helper(root.path())?)
        } else {
            None
        };
        let mut terminal = Terminal::start(root.path(), scenario)?;
        server
            .entered
            .recv_timeout(WATCHDOG)
            .context("Git did not enter HTTP fixture")?;
        // The transport is gated by an event, so this proves context precedes
        // completion without depending on the machine's speed.
        terminal.until(|bytes| contains(bytes, b"Git fetch"))?;
        ensure!(
            terminal.child.try_wait()?.is_none(),
            "work finished before the server released it"
        );
        terminal.demo_pause();
        server.release.send(false)?;
        match scenario {
            Scenario::Authenticate | Scenario::InterruptPassword => {
                terminal.until(|bytes| visible_native_prompt(bytes, b"Username for '"))?;
                terminal.demo_pause();
                terminal.input.write_all(USERNAME)?;
                terminal.input.flush()?;
                terminal.until(|bytes| visible_native_prompt(bytes, b"Password for '"))?;
                terminal.demo_pause();
                if matches!(scenario, Scenario::InterruptPassword) {
                    terminal.input.write_all(b"partial-private-password")?;
                    terminal.input.write_all(&[3])?;
                } else {
                    terminal.input.write_all(PASSWORD)?;
                }
                terminal.input.flush()?;
            }
            Scenario::ReadFirstHelper => {
                if let Err(error) = helper_ready.as_ref().unwrap().recv_timeout(WATCHDOG) {
                    while let Ok(event) = terminal.chunks.try_recv() {
                        terminal.receive(event);
                    }
                    anyhow::bail!(
                        "helper handshake {error}: {}",
                        String::from_utf8_lossy(&terminal.bytes)
                    );
                }
                // There is deliberately no prompt or child output to detect.
                // Only the fixture knows when its private FIFO says it is ready.
                terminal.input.write_all(b"fixture-helper-answer\r")?;
                terminal.input.flush()?;
            }
            Scenario::PromptDisabled | Scenario::Json | Scenario::JsonPromptDisabled => {}
        }
        let (status, bytes) = terminal.finish().with_context(|| format!("{scenario:?}"))?;
        let report = server.finish()?;
        let text = String::from_utf8_lossy(&bytes);
        match scenario {
            Scenario::Authenticate | Scenario::ReadFirstHelper => {
                ensure!(status.success(), "HTTP fetch failed: {text}");
                ensure!(
                    report.authenticated,
                    "server never accepted fixture credentials"
                );
                ensure!(
                    !contains(&bytes, b"fixture-password"),
                    "password was echoed: {text}"
                );
                let native = if matches!(scenario, Scenario::ReadFirstHelper) {
                    ensure!(
                        text.contains("helper real terminal preserved"),
                        "helper terminal witness absent: {text}"
                    );
                    ensure!(
                        !text.contains("fixture-helper-answer"),
                        "helper no-echo input leaked: {text}"
                    );
                    "helper real terminal preserved"
                } else {
                    "Username for '"
                };
                let native_start = text.find(native).context("missing native output")?;
                let receiving = text
                    .find("Receiving objects:")
                    .with_context(|| format!("missing native Git progress: {text}"))?;
                ensure!(
                    text.find("Git fetch").unwrap() < native_start,
                    "context appeared after native output"
                );
                ensure!(
                    !text[native_start..receiving].contains("░"),
                    "wsp redrew over native interaction: {text}"
                );
                ensure!(
                    text[receiving..].contains("Fetched"),
                    "wsp completion did not resume: {text}"
                );
                let mirror = root
                    .path()
                    .join("data/wsp/mirrors/127.0.0.1/owner/remote.git");
                let actual = String::from_utf8(git(&mirror, &["rev-parse", "refs/heads/main"])?)?;
                ensure!(
                    actual.trim() == expected,
                    "fetch refs do not match upstream"
                );
            }
            Scenario::PromptDisabled => {
                ensure!(
                    !status.success(),
                    "disabled prompting unexpectedly succeeded"
                );
                ensure!(
                    !has_native_prompt_line(&bytes),
                    "disabled prompt reached terminal: {text}"
                );
                ensure!(
                    text.contains("terminal prompts disabled"),
                    "native error lost: {text}"
                );
                ensure!(
                    !report.authenticated,
                    "disabled prompt supplied credentials"
                );
            }
            Scenario::Json | Scenario::JsonPromptDisabled => {
                unreachable!("JSON has its own structured-output test")
            }
            Scenario::InterruptPassword => {
                ensure!(!status.success(), "Ctrl-C unexpectedly succeeded: {text}");
                ensure!(
                    !text.contains("partial-private-password"),
                    "unfinished password echoed: {text}"
                );
                ensure!(!report.authenticated, "cancelled password reached endpoint");
            }
        }
    }
    Ok(())
}

#[test]
fn http_terminal_json_output_remains_structured() -> Result<()> {
    for scenario in [Scenario::Json, Scenario::JsonPromptDisabled] {
        let root = tempfile::tempdir()?;
        fixture(root.path())?;
        let server = HttpServer::start(root.path())?;
        register(
            root.path(),
            &format!("http://127.0.0.1:{}/owner/remote.git", server.port),
        )?;
        let terminal = Terminal::start(root.path(), scenario)?;
        server.entered.recv_timeout(WATCHDOG)?;
        server.release.send(false)?;
        // JSON always captures and isolates Git, including native mode.
        let authenticate = false;
        let (status, bytes) = terminal.finish()?;
        ensure!(
            status.success() == authenticate,
            "unexpected JSON fetch status: {}",
            String::from_utf8_lossy(&bytes)
        );
        ensure!(
            server.finish()?.authenticated == authenticate,
            "unexpected JSON authentication"
        );
        let output: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.path().join("stdout.json"))?)?;
        ensure!(output.is_object(), "stdout was not one JSON object");
        if !authenticate {
            let error = output["repos"][0]["error"]
                .as_str()
                .context("missing JSON fetch error")?;
            ensure!(
                error.contains("terminal prompts disabled"),
                "JSON lost native error: {error}"
            );
            ensure!(
                !has_native_prompt_line(&bytes),
                "JSON failure unexpectedly prompted"
            );
        }
        ensure!(
            !contains(&bytes, b"Git fetch"),
            "human context leaked into JSON mode"
        );
    }
    Ok(())
}

#[test]
fn http_terminal_parallel_credentials_observe_access_without_prompting() -> Result<()> {
    // Cached credentials work in both human and JSON paths. A fresh terminal
    // sign-in and a helper requiring /dev/tty fail in parallel mode without
    // retrying. This proves actual fetch behavior, not a config classification.
    for (name, helper, json, succeeds) in [
        ("fresh", None, false, false),
        (
            "cached",
            Some("printf 'username=fixture-user\\npassword=fixture-password\\n\\n'"),
            false,
            true,
        ),
        (
            "cached-json",
            Some("printf 'username=fixture-user\\npassword=fixture-password\\n\\n'"),
            true,
            true,
        ),
        (
            "terminal-helper",
            Some(
                "if (exec 3<> /dev/tty) 2>/dev/null; then printf 'has-tty\\n' >> helper-events; printf 'username=fixture-user\\npassword=fixture-password\\n\\n'; else printf 'no-tty\\n' >> helper-events; fi",
            ),
            false,
            false,
        ),
    ] {
        let root = tempfile::tempdir()?;
        let expected = fixture(root.path())?;
        let server = HttpServer::start(root.path())?;
        register(
            root.path(),
            &format!("http://127.0.0.1:{}/owner/remote.git", server.port),
        )?;
        if let Some(body) = helper {
            use std::os::unix::fs::PermissionsExt;
            let helper_path = root.path().join("credential-helper");
            let root_quote = root.path().display().to_string().replace('\'', "'\\''");
            std::fs::write(
                &helper_path,
                format!(
                    "#!/bin/sh\ncd '{root_quote}' || exit 1\ncase \"$1\" in get) {body};; esac\n"
                ),
            )?;
            std::fs::set_permissions(&helper_path, std::fs::Permissions::from_mode(0o700))?;
            git(
                root.path(),
                &[
                    "config",
                    "--file",
                    "gitconfig",
                    "credential.helper",
                    helper_path.to_str().unwrap(),
                ],
            )?;
        }
        let scenario = if json {
            Scenario::Json
        } else {
            Scenario::Authenticate
        };
        // No flag tests the default policy. JSON explicitly requests native,
        // proving that its captured output contract still takes precedence.
        let terminal =
            Terminal::start_with_policy(root.path(), scenario, None, json.then_some("native"))?;
        server.entered.recv_timeout(WATCHDOG)?;
        server.release.send(false)?;
        let (status, bytes) = terminal.finish()?;
        let text = String::from_utf8_lossy(&bytes);
        ensure!(status.success() == succeeds, "{name}: wrong status: {text}");
        ensure!(
            server.finish()?.authenticated == succeeds,
            "{name}: wrong authentication result"
        );
        ensure!(
            !has_native_prompt_line(&bytes),
            "{name}: native prompt escaped into captured mode: {text}"
        );
        if succeeds {
            let mirror = root
                .path()
                .join("data/wsp/mirrors/127.0.0.1/owner/remote.git");
            let actual = git(&mirror, &["rev-parse", "refs/heads/main"])?;
            ensure!(
                String::from_utf8(actual)?.trim() == expected,
                "{name}: fetch did not update mirror"
            );
        } else {
            ensure!(
                text.contains("terminal prompts disabled"),
                "{name}: missing actionable Git failure: {text}"
            );
        }
        if name == "terminal-helper" {
            ensure!(
                std::fs::read_to_string(root.path().join("helper-events"))? == "no-tty\n",
                "helper accessed terminal or was retried"
            );
        }
        if json {
            let output: serde_json::Value =
                serde_json::from_slice(&std::fs::read(root.path().join("stdout.json"))?)?;
            ensure!(
                output["repos"][0]["error"].is_null(),
                "cached JSON fetch failed: {output}"
            );
            ensure!(
                !contains(&bytes, b"\x1b["),
                "JSON progress contains terminal control sequences: {text}"
            );
        }
    }
    Ok(())
}

fn install_read_first_helper(root: &Path) -> Result<mpsc::Receiver<()>> {
    let fifo = root.join("helper-ready");
    ensure!(
        Command::new("mkfifo").arg(&fifo).status()?.success(),
        "FIFO creation failed"
    );
    let mut reader = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fifo)?;
    let (ready, receive) = mpsc::channel();
    thread::spawn(move || {
        let mut byte = [0];
        if reader.read_exact(&mut byte).is_ok() {
            let _ = ready.send(());
        }
    });
    // Git owns helper stdin/stdout for its credential protocol. The helper
    // legitimately uses the controlling terminal, changes its mode, and reads
    // without printing a prompt. wsp must not predict any of this from config.
    let helper = root.join("credential-helper");
    std::fs::write(
        &helper,
        r#"#!/bin/sh
[ "$1" = get ] || exit 0
[ -t 2 ] || { printf "helper stderr not terminal\n" >&2; exit 21; }
mode=$(stty -g < /dev/tty)
trap 'stty "$mode" < /dev/tty' EXIT
stty -echo < /dev/tty
printf R > "$HOME/helper-ready"
IFS= read -r answer < /dev/tty
[ "$answer" = fixture-helper-answer ] || exit 23
printf 'helper real terminal preserved\n' >&2
printf 'username=fixture-user\npassword=fixture-password\n'
"#,
    )?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700))?;
    git(
        root,
        &[
            "config",
            "--file",
            "gitconfig",
            "credential.helper",
            helper.to_str().unwrap(),
        ],
    )?;
    Ok(receive)
}

fn terminal_line(mut line: &[u8]) -> &[u8] {
    while line.starts_with(b"\x1b[") {
        let Some(end) = line[2..]
            .iter()
            .position(|byte| (0x40..=0x7e).contains(byte))
        else {
            return &[];
        };
        line = &line[end + 3..];
    }
    line
}

fn visible_native_prompt(bytes: &[u8], prefix: &[u8]) -> bool {
    let line = bytes
        .rsplit(|byte| matches!(byte, b'\r' | b'\n'))
        .next()
        .unwrap_or_default();
    let line = terminal_line(line);
    line.starts_with(prefix) && line.ends_with(b"': ")
}

fn has_native_prompt_line(bytes: &[u8]) -> bool {
    bytes
        .split(|byte| matches!(byte, b'\r' | b'\n'))
        .any(|line| {
            let line = terminal_line(line);
            line.starts_with(b"Username for '") || line.starts_with(b"Password for '")
        })
}

#[test]
fn native_prompt_handshake_rejects_diagnostic_substrings() {
    for (bytes, expected) in [
        (b"\r\x1b[2KUsername for 'http://fixture': ".as_slice(), true),
        (
            b"fatal: could not read Username for 'http://fixture': ".as_slice(),
            false,
        ),
        (
            b"fatal: could not read Username for 'http://fixture': Device not configured"
                .as_slice(),
            false,
        ),
        (b"\r\x1b[2KUsername for 'http://fixture'".as_slice(), false),
    ] {
        assert_eq!(visible_native_prompt(bytes, b"Username for '"), expected);
    }
}

fn contains(bytes: &[u8], pattern: &[u8]) -> bool {
    bytes
        .windows(pattern.len())
        .any(|candidate| candidate == pattern)
}

// `script` is also the existing pager integration test's portable PTY fixture.
// It starts the actual product as terminal foreground owner; no fake git runs.
struct Terminal {
    child: Child,
    input: ChildStdin,
    chunks: mpsc::Receiver<(Duration, Vec<u8>)>,
    recording: Vec<(Duration, Vec<u8>)>,
    cast_path: Option<PathBuf>,
    root: PathBuf,
    reader: Option<JoinHandle<std::io::Result<()>>>,
    bytes: Vec<u8>,
}

impl Terminal {
    fn start(root: &Path, scenario: Scenario) -> Result<Self> {
        Self::start_with(root, scenario, None)
    }

    fn start_with(root: &Path, scenario: Scenario, native_url: Option<&str>) -> Result<Self> {
        Self::start_with_policy(root, scenario, native_url, Some("native"))
    }

    fn start_with_policy(
        root: &Path,
        scenario: Scenario,
        native_url: Option<&str>,
        mode: Option<&str>,
    ) -> Result<Self> {
        let binary = std::env::var_os("WSP_PROGRESS_BASELINE")
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_wsp").into());
        let binary = if native_url.is_some() {
            "git".into()
        } else {
            binary
        };
        let mut args = if let Some(url) = native_url {
            vec!["ls-remote", url]
        } else if matches!(scenario, Scenario::Json | Scenario::JsonPromptDisabled) {
            vec!["repo", "fetch", "--all", "--json"]
        } else {
            vec!["repo", "fetch", "--all"]
        };
        if native_url.is_none()
            && let Some(mode) = mode
        {
            args.extend(["--git-progress", mode]);
        }
        let quote = |value: &str| format!("'{}'", value.replace('\'', "'\\''"));
        let invocation = format!(
            "{} {}",
            quote(&binary.to_string_lossy()),
            args.iter()
                .map(|value| quote(value))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let invocation = if matches!(scenario, Scenario::Json | Scenario::JsonPromptDisabled) {
            format!("{invocation} > stdout.json")
        } else {
            invocation
        };
        // The shell records terminal modes around the actual product process.
        let invocation = format!(
            "trap ':' INT; tty > tty-path; stty -g > tty-before; {invocation}; result=$?; stty -g > tty-after; exit \"$result\""
        );
        let mut command = isolated_command("script", root);
        #[cfg(target_os = "macos")]
        command.args(["-q", "/dev/null", "/bin/sh", "-c", &invocation]);
        #[cfg(target_os = "linux")]
        command.args(["-q", "-e", "-c", &invocation, "/dev/null"]);
        if matches!(
            scenario,
            Scenario::PromptDisabled | Scenario::JsonPromptDisabled
        ) {
            command.env("GIT_TERMINAL_PROMPT", "0");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = child.stdin.take().context("missing script stdin")?;
        let mut output = child.stdout.take().context("missing script stdout")?;
        let (send, chunks) = mpsc::channel();
        let started = Instant::now();
        let reader = thread::spawn(move || {
            let mut buffer = [0; 4096];
            loop {
                let count = output.read(&mut buffer)?;
                if count == 0
                    || send
                        .send((started.elapsed(), buffer[..count].to_vec()))
                        .is_err()
                {
                    return Ok(());
                }
            }
        });
        Ok(Self {
            child,
            input,
            chunks,
            recording: Vec::new(),
            root: root.to_owned(),
            cast_path: if matches!(scenario, Scenario::Authenticate) && native_url.is_none() {
                std::env::var_os("WSP_HTTP_PROGRESS_CAST").map(PathBuf::from)
            } else {
                None
            },
            reader: Some(reader),
            bytes: Vec::new(),
        })
    }

    fn demo_pause(&self) {
        if self.cast_path.is_some() {
            // Visual pacing only when explicitly recording a demo. Regression
            // tests synchronize using events and never wait for elapsed time.
            thread::sleep(Duration::from_millis(900));
        }
    }

    fn receive(&mut self, event: (Duration, Vec<u8>)) {
        self.bytes.extend_from_slice(&event.1);
        if self.cast_path.is_some() {
            self.recording.push(event);
        }
    }

    fn until(&mut self, condition: impl Fn(&[u8]) -> bool) -> Result<()> {
        let deadline = Instant::now() + WATCHDOG;
        while !condition(&self.bytes) {
            let next = self
                .chunks
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .with_context(|| {
                    format!(
                        "missing terminal handshake: {}",
                        String::from_utf8_lossy(&self.bytes)
                    )
                })?;
            self.receive(next);
            ensure!(
                self.bytes.len() < 1_048_576,
                "terminal output exceeded fixture bound"
            );
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(ExitStatus, Vec<u8>)> {
        let deadline = Instant::now() + WATCHDOG;
        loop {
            match self
                .chunks
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(bytes) => {
                    self.receive(bytes);
                    ensure!(
                        self.bytes.len() < 1_048_576,
                        "terminal output exceeded fixture bound"
                    );
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "terminal did not exit: {}",
                            String::from_utf8_lossy(&self.bytes)
                        )
                    });
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            reader.join().expect("terminal reader panicked")?;
        }
        let status = self.child.wait()?;
        let before = std::fs::read_to_string(self.root.join("tty-before"))?;
        let after = std::fs::read_to_string(self.root.join("tty-after"))?;
        ensure!(
            configured_modes(&before)? == configured_modes(&after)?,
            "terminal modes were not restored: before={before:?} after={after:?} transcript={}",
            String::from_utf8_lossy(&self.bytes)
        );
        if status.success()
            && let Some(path) = &self.cast_path
        {
            write_cast(path, &self.recording)?;
        }
        Ok((status, std::mem::take(&mut self.bytes)))
    }
}

fn configured_modes(modes: &str) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        // macOS's PENDIN is transient line-discipline state, not a configured
        // mode. A canonical-mode transition sets it until the next input read.
        // Compare every configured flag, control character and speed exactly.
        modes
            .split(':')
            .map(|field| {
                if let Some(flags) = field.strip_prefix("lflag=") {
                    Ok(format!(
                        "lflag={:x}",
                        u64::from_str_radix(flags, 16)? & !0x2000_0000
                    ))
                } else {
                    Ok(field.to_owned())
                }
            })
            .collect::<Result<Vec<_>>>()
            .map(|fields| fields.join(":"))
    }
    #[cfg(not(target_os = "macos"))]
    Ok(modes.to_owned())
}

fn write_cast(path: &Path, events: &[(Duration, Vec<u8>)]) -> Result<()> {
    let mut file = std::fs::File::create(path)?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({
            "version": 2, "width": 80, "height": 16,
            "title": "wsp yields to native Git, then resumes",
            "env": { "TERM": "xterm-256color" }
        }),
    )?;
    file.write_all(b"\n")?;
    // A PTY read may divide a UTF-8 character. Keep that partial character for
    // the next real chunk instead of replacing or inventing terminal bytes.
    let mut pending = Vec::new();
    for (elapsed, bytes) in events {
        pending.extend_from_slice(bytes);
        let valid = match std::str::from_utf8(&pending) {
            Ok(_) => pending.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(error) => return Err(error.into()),
        };
        if valid > 0 {
            let text = std::str::from_utf8(&pending[..valid])?;
            serde_json::to_writer(
                &mut file,
                &serde_json::json!([elapsed.as_secs_f64(), "o", text]),
            )?;
            file.write_all(b"\n")?;
            pending.drain(..valid);
        }
    }
    ensure!(
        pending.is_empty(),
        "terminal recording ends in partial UTF-8"
    );
    Ok(())
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Closing the disposable PTY gives its foreground group SIGHUP. Send
        // Ctrl-C first so the product can also cancel privately owned children.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.input.write_all(&[3]);
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[derive(Default)]
struct HttpReport {
    authenticated: bool,
    authenticated_paths: BTreeSet<String>,
    interrupted_connection_closed: bool,
}

struct HttpServer {
    port: u16,
    entered: mpsc::Receiver<()>,
    release: mpsc::Sender<bool>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<HttpReport>>>,
}

impl HttpServer {
    fn start(root: &Path) -> Result<Self> {
        Self::start_expected(root, 1)
    }

    fn start_expected(root: &Path, initial_requests: usize) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let (enter, entered) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let root = root.to_owned();
        let worker = thread::spawn(move || {
            let mut report = HttpReport::default();
            let mut initial = VecDeque::new();
            for _ in 0..initial_requests {
                let (mut stream, _) = listener.accept()?;
                if stopped.load(Ordering::Acquire) {
                    return Ok(report);
                }
                let header = read_http_header(&mut stream)?;
                initial.push_back((stream, header));
            }
            enter.send(())?;
            if released.recv_timeout(WATCHDOG)? {
                let mut byte = [0];
                for (mut stream, _) in initial {
                    report.interrupted_connection_closed = match stream.read(&mut byte) {
                        Ok(0) => true,
                        Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => true,
                        other => {
                            anyhow::bail!("native HTTP child did not close connection: {other:?}")
                        }
                    };
                }
                return Ok(report);
            }
            loop {
                let (mut stream, header) = if let Some(request) = initial.pop_front() {
                    request
                } else {
                    let (mut stream, _) = listener.accept()?;
                    if stopped.load(Ordering::Acquire) {
                        return Ok(report);
                    }
                    let header = read_http_header(&mut stream)?;
                    (stream, header)
                };
                let path = header
                    .lines()
                    .next()
                    .context("HTTP request missing")?
                    .split_whitespace()
                    .nth(1)
                    .context("HTTP path missing")?;
                let authorization = if path.starts_with("/owner/second.git/") {
                    "Authorization: Basic Zml4dHVyZS1zZWNvbmQ6Zml4dHVyZS1zZWNvbmQtcGFzc3dvcmQ="
                } else {
                    "Authorization: Basic Zml4dHVyZS11c2VyOmZpeHR1cmUtcGFzc3dvcmQ="
                };
                if !header
                    .lines()
                    .any(|line| line.eq_ignore_ascii_case(authorization))
                {
                    stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"wsp-progress-fixture\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
                    continue;
                }
                report.authenticated = true;
                report.authenticated_paths.insert(path.to_owned());
                serve_git(&root, &header, &mut stream)?;
            }
        });
        Ok(Self {
            port,
            entered,
            release,
            stop,
            worker: Some(worker),
        })
    }

    fn finish(mut self) -> Result<HttpReport> {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        self.worker
            .take()
            .context("HTTP worker missing")?
            .join()
            .expect("HTTP fixture panicked")
    }
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.release.send(true);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn read_http_header(stream: &mut TcpStream) -> Result<String> {
    stream.set_read_timeout(Some(WATCHDOG))?;
    stream.set_write_timeout(Some(WATCHDOG))?;
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        header.push(byte[0]);
        ensure!(header.len() <= 16_384, "HTTP headers exceed bound");
    }
    Ok(String::from_utf8(header)?)
}

fn serve_git(root: &Path, header: &str, stream: &mut TcpStream) -> Result<()> {
    let field = |name: &str| {
        header
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
            .unwrap_or("")
    };
    let mut request = header
        .lines()
        .next()
        .context("empty HTTP request")?
        .split_whitespace();
    let method = request.next().context("HTTP method missing")?;
    let path = request.next().context("HTTP path missing")?;
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    ensure!(
        matches!(
            path,
            "/owner/remote.git/info/refs"
                | "/owner/remote.git/git-upload-pack"
                | "/owner/remote.git/HEAD"
                | "/owner/second.git/info/refs"
                | "/owner/second.git/git-upload-pack"
                | "/owner/second.git/HEAD"
        ),
        "unexpected Git HTTP path {path}"
    );
    ensure!(
        field("Transfer-Encoding").is_empty(),
        "unexpected chunked request"
    );
    let content_length = if field("Content-Length").is_empty() {
        0
    } else {
        field("Content-Length").parse::<usize>()?
    };
    ensure!(content_length < 65_536, "HTTP body exceeds bound");
    let mut body = vec![0; content_length];
    stream.read_exact(&mut body)?;
    let mut backend = isolated_command("git", root)
        .arg("http-backend")
        .env("GIT_PROJECT_ROOT", root)
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", path)
        .env("QUERY_STRING", query)
        .env("REQUEST_METHOD", method)
        .env("CONTENT_TYPE", field("Content-Type"))
        .env("CONTENT_LENGTH", content_length.to_string())
        .env("HTTP_GIT_PROTOCOL", field("Git-Protocol"))
        .env("REMOTE_USER", "fixture-user")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    backend
        .stdin
        .take()
        .context("CGI stdin missing")?
        .write_all(&body)?;
    let output = backend.wait_with_output()?;
    ensure!(
        output.status.success(),
        "Git HTTP backend failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let split = output
        .stdout
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .context("CGI headers missing")?;
    stream.write_all(b"HTTP/1.1 200 OK\r\n")?;
    stream.write_all(&output.stdout[..split])?;
    let body = &output.stdout[split + 4..];
    write!(
        stream,
        "\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    if std::env::var_os("WSP_HTTP_PROGRESS_CAST").is_some() && method == "POST" {
        // Controlled endpoint pacing makes Git's actual measured progress
        // visible in the requested recording. Test synchronization uses events.
        for chunk in body.chunks(48) {
            stream.write_all(chunk)?;
            stream.flush()?;
            thread::sleep(Duration::from_millis(25));
        }
    } else {
        stream.write_all(body)?;
    }
    Ok(())
}
