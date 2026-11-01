//! Real subprocess handshakes cover quiet transports and observer failure.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn source(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet", "--initial-branch=main"]);
    git(dir, &["config", "user.email", "fixture@example.test"]);
    git(dir, &["config", "user.name", "Fixture"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(
        dir,
        &["commit", "--quiet", "--allow-empty", "-m", "initial"],
    );
}

#[test]
fn transport_progress_names_quiet_clone_and_fetch_before_completion() {
    for phase in ["clone", "fetch"] {
        let tmp = tempfile::tempdir().unwrap();
        let upstream = tmp.path().join("upstream");
        source(&upstream);
        let bin = tmp.path().join("bin");
        let data = tmp.path().join("data");
        let release = tmp.path().join("release");
        let entered = tmp.path().join("entered");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(data.join("wsp")).unwrap();
        wsp_core::config::Config {
            workspaces_dir: Some(tmp.path().join("workspaces").display().to_string()),
            hints: Some(false),
            ..Default::default()
        }
        .save_to(&data.join("wsp/config.yaml"))
        .unwrap();
        let config = tmp.path().join("gitconfig");
        std::fs::write(
            &config,
            format!(
                "[url \"file://{}\"]\n\tinsteadOf = https://github.com/test/sample\n",
                upstream.display()
            ),
        )
        .unwrap();
        let make_command = || {
            let binary = std::env::var_os("WSP_PROGRESS_BASELINE")
                .unwrap_or_else(|| env!("CARGO_BIN_EXE_wsp").into());
            let mut command = Command::new(binary);
            command
                .current_dir(tmp.path())
                .env("HOME", tmp.path())
                .env("XDG_DATA_HOME", &data)
                .env("GIT_CONFIG_GLOBAL", &config);
            command
        };
        if phase == "fetch" {
            let output = make_command()
                .args(["registry", "add", "https://github.com/test/sample"])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "fixture: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        // Resolve the executable before adding the wrapper to the child's PATH.
        let output = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let executable = String::from_utf8(output.stdout).unwrap().trim().to_owned();
        let wrapper = bin.join("git");
        std::fs::write(&wrapper, "#!/bin/sh\nif [ \"$1\" = \"$WSP_TEST_PHASE\" ]; then\n  touch \"$WSP_TEST_ENTERED\"\n  while [ ! -f \"$WSP_TEST_RELEASE\" ]; do sleep 0.02; done\nfi\nexec \"$WSP_TEST_GIT\" \"$@\"\n").unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let args = if phase == "clone" {
            vec![
                "registry",
                "add",
                "https://github.com/test/sample",
                "--json",
            ]
        } else {
            vec!["repo", "fetch", "--all", "--json"]
        };
        let mut child = make_command()
            .args(args)
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("WSP_TEST_PHASE", phase)
            .env("WSP_TEST_ENTERED", &entered)
            .env("WSP_TEST_RELEASE", &release)
            .env("WSP_TEST_GIT", &executable)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let (tx, rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        // The CLI reveal clock is not injectable across a process boundary.
        // This generous watchdog bounds a missing event, not execution speed;
        // the transport only completes after the feedback handshake releases it.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut lines = Vec::new();
        let seen = loop {
            match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(line) => {
                    let active = if phase == "clone" {
                        "Cloning github.com/test/sample"
                    } else {
                        "sample · Git fetch"
                    };
                    let matches = entered.exists() && line.contains(active);
                    lines.push(line);
                    if matches {
                        break true;
                    }
                }
                Err(_) => break false,
            }
        };
        let blocked = child.try_wait().unwrap().is_none();
        std::fs::write(&release, "release").unwrap();
        let output = child.wait_with_output().unwrap();
        drop(rx);
        reader.join().unwrap();
        assert!(
            seen && blocked,
            "{phase} missing active identity: {lines:?}"
        );
        assert!(output.status.success(), "{phase} failed: {lines:?}");
        assert!(
            lines.iter().all(|line| !line.contains('\x1b')),
            "JSON feedback must be plain: {lines:?}"
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(result.get("error").is_none(), "{result}");
        let mirror = data.join("wsp/mirrors/github.com/test/sample.git");
        assert_eq!(
            git(&mirror, &["rev-parse", "refs/heads/main"]),
            git(&upstream, &["rev-parse", "HEAD"])
        );
    }
}

#[test]
fn terminal_fetch_progress_renders_named_bar_rows_for_every_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let wsp_data = data.join("wsp");
    let bin = tmp.path().join("bin");
    let release = tmp.path().join("release");
    let entered = tmp.path().join("entered");
    std::fs::create_dir_all(&wsp_data).unwrap();
    std::fs::create_dir_all(&bin).unwrap();

    let mut config = wsp_core::config::Config {
        workspaces_dir: Some(tmp.path().join("workspaces").display().to_string()),
        hints: Some(false),
        ..Default::default()
    };
    let identities = ["github.com/demo/alpha", "github.com/demo/bravo"];
    for identity in identities {
        let upstream = tmp.path().join(identity.rsplit('/').next().unwrap());
        source(&upstream);
        let mirror = wsp_core::mirror::dir(
            &wsp_data.join("mirrors"),
            &wsp_core::giturl::Parsed::from_identity(identity).unwrap(),
        );
        std::fs::create_dir_all(mirror.parent().unwrap()).unwrap();
        let output = Command::new("git")
            .args(["clone", "--quiet", "--bare"])
            .arg(&upstream)
            .arg(&mirror)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "fixture mirror: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        config.repos.insert(
            identity.to_string(),
            wsp_core::config::RepoEntry {
                url: upstream.display().to_string(),
                added: chrono::Utc::now(),
                setup_commands: None,
            },
        );
    }
    config.save_to(&wsp_data.join("config.yaml")).unwrap();

    let git = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let git = String::from_utf8(git.stdout).unwrap().trim().to_owned();
    let wrapper = bin.join("git");
    std::fs::write(
        &wrapper,
        "#!/bin/sh\nif [ \"$1\" = fetch ]; then\n  printf '%s\\n' \"$*\" >> \"$WSP_TEST_ENTERED\"\n  while [ ! -f \"$WSP_TEST_RELEASE\" ]; do sleep 0.02; done\nfi\nexec \"$WSP_TEST_GIT\" \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let launcher = tmp.path().join("launch-wsp");
    std::fs::write(
        &launcher,
        "#!/bin/sh\nexec \"$WSP_TEST_BINARY\" repo fetch --all 2>&1\n",
    )
    .unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut script = Command::new("script");
    if cfg!(target_os = "macos") {
        script.args(["-q", "/dev/null"]).arg(&launcher);
    } else {
        script
            .args(["-q", "-e", "-c"])
            .arg(format!("'{}'", launcher.display()))
            .arg("/dev/null");
    }
    let binary = std::env::var_os("WSP_PROGRESS_BASELINE")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_wsp").into());
    let mut child = script
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("XDG_DATA_HOME", &data)
        .env("TERM", "xterm-256color")
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("WSP_TEST_BINARY", binary)
        .env("WSP_TEST_GIT", git)
        .env("WSP_TEST_ENTERED", &entered)
        .env("WSP_TEST_RELEASE", &release)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let stdout = child.stdout.take().unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut stdout = stdout;
        let mut buffer = [0; 4096];
        loop {
            let count = stdout.read(&mut buffer).unwrap();
            if count == 0 || send.send(buffer[..count].to_vec()).is_err() {
                break;
            }
        }
    });

    // The fetch wrappers hold both workers open. Each identity must appear in
    // an aligned bar row in the terminal output. The deadline is only a
    // watchdog for a missing frame or a broken PTY setup.
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut captured = Vec::new();
    let (snapshot, saw_rows) = loop {
        let snapshot = {
            let text = String::from_utf8_lossy(&captured);
            let entered_count =
                std::fs::read_to_string(&entered).map_or(0, |entries| entries.lines().count());
            let redraw = ["\r\x1b[2K", "\r\x1b[1A\x1b[J"]
                .iter()
                .filter_map(|sequence| text.rfind(sequence).map(|index| index + sequence.len()))
                .max()
                .unwrap_or(0);
            let frame = &text[redraw..];
            let rows: Vec<_> = frame
                .lines()
                .filter(|line| line.contains('[') && identities.iter().any(|id| line.contains(id)))
                .collect();
            if entered_count == identities.len()
                && identities
                    .iter()
                    .all(|identity| rows.iter().any(|line| line.contains(identity)))
            {
                Some(frame.to_owned())
            } else {
                None
            }
        };
        if let Some(snapshot) = snapshot {
            break (snapshot, true);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break (String::from_utf8_lossy(&captured).into_owned(), false);
        }
        match receive.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(bytes) => captured.extend(bytes),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                break (String::from_utf8_lossy(&captured).into_owned(), false);
            }
        }
    };
    let both_fetches_blocked = std::fs::read_to_string(&entered)
        .is_ok_and(|entries| entries.lines().count() == identities.len());
    let blocked = both_fetches_blocked && child.try_wait().unwrap().is_none();
    std::fs::write(&release, "release").unwrap();
    let status = child.wait().unwrap();
    drop(receive);
    reader.join().unwrap();

    assert!(
        blocked,
        "fetch did not reach the held transport: {captured:?}"
    );
    assert!(
        saw_rows,
        "no aligned repository bar rows were rendered: {snapshot:?}"
    );
    assert!(
        identities
            .iter()
            .all(|identity| snapshot.contains(identity)),
        "terminal output must name both repositories, snapshot was: {snapshot:?}"
    );
    let rows: Vec<_> = snapshot
        .lines()
        .filter(|line| identities.iter().any(|identity| line.contains(identity)))
        .collect();
    assert_eq!(
        rows.len(),
        identities.len(),
        "unexpected repeated rows: {snapshot:?}"
    );
    assert!(
        status.success(),
        "fetch failed: {}",
        String::from_utf8_lossy(&captured)
    );
}

#[test]
fn transport_progress_failed_observer_preserves_clone_fetch_and_refs() {
    struct Failed(std::sync::atomic::AtomicUsize);
    impl wsp_core::progress::Observer for Failed {
        fn observe(&self, _: wsp_core::progress::Event) -> bool {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            false
        }
    }
    let tmp = tempfile::tempdir().unwrap();
    let upstream = tmp.path().join("upstream");
    source(&upstream);
    let mirror = tmp.path().join("mirror.git");
    let failed = Arc::new(Failed(std::sync::atomic::AtomicUsize::new(0)));
    let _installation = wsp_core::progress::install(failed.clone());
    wsp_core::git::clone_bare(upstream.to_str().unwrap(), &mirror).unwrap();
    git(
        &upstream,
        &["commit", "--quiet", "--allow-empty", "-m", "advance"],
    );
    // Reinstall to exercise a failure during fetch independently of clone.
    let _fetch_installation = wsp_core::progress::install(failed.clone());
    wsp_core::git::fetch(&mirror, true).unwrap();
    assert_eq!(
        failed.0.load(std::sync::atomic::Ordering::Relaxed),
        2,
        "clone and fetch must each encounter and disable the failing observer"
    );
    assert_eq!(
        git(&mirror, &["rev-parse", "refs/heads/main"]),
        git(&upstream, &["rev-parse", "HEAD"])
    );
}

#[test]
fn transport_progress_controlling_terminal_authentication_accepts_input() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = tmp.path().join("upstream");
    source(&upstream);
    let ssh = tmp.path().join("ssh-fixture");
    std::fs::write(&ssh, "#!/bin/sh\nprintf 'Fixture passphrase: ' > /dev/tty\nIFS= read -r answer < /dev/tty\n[ \"$answer\" = \"fixture-answer\" ] || exit 1\nexec git upload-pack \"$WSP_TEST_UPSTREAM\"\n").unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = tmp.path().join("gitconfig");
    std::fs::write(
        &config,
        format!("[core]\n\tsshCommand = {}\n", ssh.display()),
    )
    .unwrap();
    let launcher = tmp.path().join("launch");
    std::fs::write(
        &launcher,
        "#!/bin/sh\nexec \"$WSP_TEST_BINARY\" registry add git@github.com:test/sample.git\n",
    )
    .unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut command = Command::new("script");
    if cfg!(target_os = "macos") {
        command.args(["-q", "/dev/null"]).arg(&launcher);
    } else {
        // util-linux script accepts a command string rather than argv.
        command
            .args(["-q", "-e", "-c"])
            .arg(format!("'{}'", launcher.display()))
            .arg("/dev/null");
    }
    let mut child = command
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_SSH_VARIANT", "ssh")
        .env("WSP_TEST_UPSTREAM", &upstream)
        .env("WSP_TEST_BINARY", env!("CARGO_BIN_EXE_wsp"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut bytes = [0; 4096];
        loop {
            let count = stdout.read(&mut bytes).unwrap();
            if count == 0 || tx.send(bytes[..count].to_vec()).is_err() {
                break;
            }
        }
    });
    // The operating system PTY and CLI clock cannot be injected. The deadline
    // is only a watchdog; input is sent after the actual prompt arrives.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut captured = Vec::new();
    let mut prompts = 0;
    let mut erased_prompt = false;
    while let Ok(bytes) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        captured.extend(bytes);
        let count = String::from_utf8_lossy(&captured)
            .matches("Fixture passphrase: ")
            .count();
        // Hold the first prompt until slow-operation feedback is visible.
        // This tests handoff while authentication actually owns the terminal.
        let text = String::from_utf8_lossy(&captured);
        let feedback_after_prompt = text.find("Fixture passphrase: ").is_some_and(|start| {
            let active = &text[start..];
            active.contains("Cloning github.com/test/sample")
        });
        if count > prompts && (prompts > 0 || feedback_after_prompt) {
            erased_prompt |=
                text[text.rfind("Fixture passphrase: ").unwrap()..].contains("\x1b[2K");
            child
                .stdin
                .as_mut()
                .unwrap()
                .write_all(b"fixture-answer\n")
                .unwrap();
            prompts = count;
        }
    }
    if child.try_wait().unwrap().is_none() {
        child.kill().unwrap();
    }
    let output = child.wait_with_output().unwrap();
    drop(rx);
    reader.join().unwrap();
    let text = String::from_utf8_lossy(&captured);
    assert!(prompts > 0, "controlling-terminal prompt missing: {text}");
    assert!(
        !erased_prompt,
        "progress erased an active authentication prompt: {text}"
    );
    assert!(
        output.status.success(),
        "PTY authentication failed: {text}; {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mirror = tmp
        .path()
        .join("data/wsp/mirrors/github.com/test/sample.git");
    assert_eq!(
        git(&mirror, &["rev-parse", "refs/heads/main"]),
        git(&upstream, &["rev-parse", "HEAD"])
    );
}

#[test]
fn transport_progress_askpass_environment_reaches_the_authentication_child() {
    let tmp = tempfile::tempdir().unwrap();
    let upstream = tmp.path().join("upstream");
    source(&upstream);
    let askpass = tmp.path().join("askpass-fixture");
    let invoked = tmp.path().join("askpass-invoked");
    std::fs::write(&askpass, "#!/bin/sh\nprintf '%s' \"$1\" > \"$WSP_TEST_ASKPASS_INVOKED\"\nprintf 'fixture-answer\\n'\n").unwrap();
    std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o755)).unwrap();
    let ssh = tmp.path().join("ssh-fixture");
    // This transport chooses askpass explicitly, like SSH with forced askpass.
    // No real SSH server or display is needed to verify inherited helper env.
    std::fs::write(&ssh, "#!/bin/sh\nanswer=$(\"$SSH_ASKPASS\" 'Fixture passphrase: ') || exit 1\n[ \"$answer\" = fixture-answer ] || exit 1\nexec git upload-pack \"$WSP_TEST_UPSTREAM\"\n").unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let config = tmp.path().join("gitconfig");
    std::fs::write(
        &config,
        format!("[core]\n\tsshCommand = {}\n", ssh.display()),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_wsp"))
        .args([
            "registry",
            "add",
            "git@github.com:test/sample.git",
            "--json",
        ])
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("GIT_CONFIG_GLOBAL", config)
        .env("GIT_SSH_VARIANT", "ssh")
        .env("SSH_ASKPASS", askpass)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("WSP_TEST_ASKPASS_INVOKED", &invoked)
        .env("WSP_TEST_UPSTREAM", &upstream)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "askpass transport failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(invoked).unwrap(),
        "Fixture passphrase: "
    );
    assert_eq!(
        git(
            &tmp.path()
                .join("data/wsp/mirrors/github.com/test/sample.git"),
            &["rev-parse", "refs/heads/main"]
        ),
        git(&upstream, &["rev-parse", "HEAD"])
    );
}
