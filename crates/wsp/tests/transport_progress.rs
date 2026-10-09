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
fn terminal_fetch_isolates_concurrent_git_and_animates_while_quiet() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let wsp_data = data.join("wsp");
    let bin = tmp.path().join("bin");
    let events = tmp.path().join("events");
    let controls = tmp.path().join("controls");
    std::fs::create_dir_all(&controls).unwrap();
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
        git(
            &mirror,
            &[
                "config",
                "remote.origin.fetch",
                "+refs/heads/*:refs/heads/*",
            ],
        );
        git(
            &upstream,
            &["commit", "--quiet", "--allow-empty", "-m", "advance"],
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

    let executable = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let executable = String::from_utf8(executable.stdout)
        .unwrap()
        .trim()
        .to_owned();
    // FIFO commands establish the exact output order without scheduling sleeps.
    // Both transports rendezvous before either writes its first carriage-return
    // frame. This fixture covers the default parallel renderer; native serial
    // mode needs a separate fixture because a rendezvous would deadlock it.
    let mut control_writers = Vec::new();
    for path in [
        events.clone(),
        controls.join("alpha.git"),
        controls.join("bravo.git"),
    ] {
        assert!(
            Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        let fifo = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        control_writers.push(fifo);
    }
    let mut event_writer = control_writers.remove(0);
    let event_reader = event_writer.try_clone().unwrap();
    let (event_send, event_receive) = mpsc::channel();
    let event_thread = std::thread::spawn(move || {
        for event in BufReader::new(event_reader).lines() {
            let event = event.unwrap();
            if event == "stop" || event_send.send(event).is_err() {
                break;
            }
        }
    });
    let wrapper = bin.join("git");
    std::fs::write(
        &wrapper,
        r#"#!/bin/sh
if [ "$1" != fetch ]; then
  exec "$WSP_TEST_GIT" "$@"
fi
repo=
for arg in "$@"; do
  case "$arg" in
    "$WSP_TEST_UPSTREAMS/alpha") repo=alpha.git ;;
    "$WSP_TEST_UPSTREAMS/bravo") repo=bravo.git ;;
  esac
done
if [ -z "$repo" ]; then exec "$WSP_TEST_GIT" "$@"; fi
terminal=detached
if [ -t 2 ]; then terminal=inherited; fi
controlling=no-tty
if (exec 3<>/dev/tty) 2>/dev/null; then controlling=has-tty; fi
printf 'entered %s %s %s\n' "$repo" "$terminal" "$controlling" > "$WSP_TEST_EVENTS"
IFS= read -r action < "$WSP_TEST_CONTROLS/$repo" || exit 1
[ "$action" = frame ] || exit 1
printf '\r\033[2KReceiving objects: %s 32%%' "$repo" >&2
printf 'frame %s\n' "$repo" > "$WSP_TEST_EVENTS"
IFS= read -r action < "$WSP_TEST_CONTROLS/$repo" || exit 1
[ "$action" = release ] || exit 1
printf '\r\033[2KReceiving objects: %s 100%%\n' "$repo" >&2
"$WSP_TEST_GIT" "$@"
result=$?
printf 'done %s\n' "$repo" > "$WSP_TEST_EVENTS"
exit "$result"
"#,
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let launcher = tmp.path().join("launch-wsp");
    std::fs::write(
        &launcher,
        "#!/bin/sh\nstty rows 24 cols 100\nexec \"$WSP_TEST_BINARY\" repo fetch --all 2>&1\n",
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
        .env("WSP_TEST_GIT", executable)
        .env("WSP_TEST_UPSTREAMS", tmp.path())
        .env("WSP_TEST_EVENTS", &events)
        .env("WSP_TEST_CONTROLS", &controls)
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

    // Watchdogs only bound a missing handshake. Success is established by
    // entered/frame events and the fetched refs, never by elapsed time.
    let next_event = || event_receive.recv_timeout(Duration::from_secs(30));
    let mut audit = Vec::new();
    for _ in identities {
        audit.push(next_event().expect("both Git transports must enter the rendezvous"));
    }
    // The real CLI tick is not injectable across a process boundary. Hold both
    // children at the FIFO rendezvous until two different cursor positions have
    // been observed for each named row. The deadline only bounds missing output.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut captured = Vec::new();
    let mut positions = [
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::new(),
    ];
    while positions.iter().any(|positions| positions.len() < 2) {
        let Ok(bytes) = receive.recv_timeout(deadline.saturating_duration_since(Instant::now()))
        else {
            break;
        };
        captured.extend(bytes);
        let text = String::from_utf8_lossy(&captured);
        for row in text.split(['\r', '\n']) {
            for segment in row.split('[').skip(1) {
                let Some((bar, label)) = segment.split_once(']') else {
                    continue;
                };
                if bar.chars().count() != 8 || !bar.chars().all(|cell| matches!(cell, '█' | '░'))
                {
                    continue;
                }
                for (name, positions) in ["alpha", "bravo"].into_iter().zip(&mut positions) {
                    if label.contains(name) {
                        positions.insert(bar.to_owned());
                    }
                }
            }
        }
    }
    let animated = positions.iter().all(|positions| positions.len() >= 2);
    for (name, writer) in ["alpha.git", "bravo.git"]
        .into_iter()
        .zip(&mut control_writers)
    {
        writeln!(writer, "frame").unwrap();
        let event = next_event().unwrap_or_else(|error| {
            panic!("Git must acknowledge its carriage-return frame: {error}; audit: {audit:?}")
        });
        assert_eq!(event, format!("frame {name}"));
        audit.push(event);
    }
    let blocked = child.try_wait().unwrap().is_none();
    for writer in &mut control_writers {
        writeln!(writer, "release").unwrap();
    }
    for _ in identities {
        audit.push(next_event().expect("released Git transports must finish"));
    }
    writeln!(event_writer, "stop").unwrap();
    event_thread.join().unwrap();
    let status = child.wait().unwrap();
    reader.join().unwrap();
    captured.extend(receive.into_iter().flatten());
    let text = String::from_utf8_lossy(&captured);

    assert!(
        blocked,
        "fetch must wait for both release handshakes: {text}"
    );
    assert!(
        audit
            .iter()
            .take(2)
            .all(|event| event.starts_with("entered ")),
        "both wrappers must rendezvous before their frames: {audit:?}"
    );
    let inherited = audit
        .iter()
        .filter(|event| event.contains(" inherited "))
        .count();
    assert!(status.success(), "fetch failed: {text}");
    for identity in identities {
        let name = identity.rsplit('/').next().unwrap();
        assert!(
            text.contains(&format!("ok    {name}")),
            "missing successful result: {text}"
        );
        let mirror = wsp_core::mirror::dir(
            &wsp_data.join("mirrors"),
            &wsp_core::giturl::Parsed::from_identity(identity).unwrap(),
        );
        assert_eq!(
            git(&mirror, &["rev-parse", "refs/heads/main"]),
            git(&tmp.path().join(name), &["rev-parse", "HEAD"]),
            "{identity} must receive its new upstream commit"
        );
    }
    assert_eq!(
        inherited, 0,
        "concurrent Git must never inherit terminal output: {audit:?}; terminal: {text:?}"
    );
    assert!(
        audit
            .iter()
            .take(2)
            .all(|event| event.ends_with(" detached no-tty")),
        "captured Git must also lose its controlling terminal: {audit:?}"
    );
    assert!(
        animated,
        "each silent Git child needs a moving bar before release: {positions:?}; terminal: {text:?}"
    );
    for name in ["alpha.git", "bravo.git"] {
        assert!(
            !text.contains(&format!("\r\x1b[2KReceiving objects: {name}")),
            "Git wrote its raw erase-line frame into wsp's live region: {text:?}"
        );
    }
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
        "#!/bin/sh\nexec \"$WSP_TEST_BINARY\" registry add git@github.com:test/sample.git --git-progress native\n",
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
        let text = String::from_utf8_lossy(&captured);
        if count > prompts {
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
    let prompt = text.find("Fixture passphrase: ").unwrap();
    assert!(
        text[..prompt].contains("Cloning github.com/test/sample"),
        "clone context must precede the authentication prompt: {text}"
    );
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
            "--git-progress",
            "native",
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

#[test]
fn transport_progress_cancellation_closes_detached_descendant_descriptors() {
    let tmp = tempfile::tempdir().unwrap();
    let events = tmp.path().join("events");
    let alive = tmp.path().join("alive");
    let release = tmp.path().join("release");
    for fifo in [&events, &alive, &release] {
        assert!(Command::new("mkfifo").arg(fifo).status().unwrap().success());
    }
    // Keep the event FIFO open while readers and writers rendezvous. The liveness
    // FIFO is read-only, so EOF proves every inherited writer has been closed.
    let mut event_control = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&events)
        .unwrap();
    let event_reader = event_control.try_clone().unwrap();
    let (event_send, event_receive) = mpsc::channel();
    let event_thread = std::thread::spawn(move || {
        for line in BufReader::new(event_reader).lines() {
            let line = line.unwrap();
            if line == "stop" || event_send.send(line).is_err() {
                break;
            }
        }
    });
    let (closed_send, closed_receive) = mpsc::channel();
    let alive_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = std::fs::File::open(alive).and_then(|mut file| file.read_to_end(&mut bytes));
        let _ = closed_send.send(result.map(|_| bytes));
    });
    let descendant = tmp.path().join("descendant");
    std::fs::write(&descendant, "#!/bin/sh\nprintf 'descendant %s\\n' \"$$\" > \"$WSP_TEST_EVENTS\"\nIFS= read -r action < \"$WSP_TEST_RELEASE\"\n").unwrap();
    let ssh = tmp.path().join("ssh-fixture");
    std::fs::write(&ssh, "#!/bin/sh\nexec 3>\"$WSP_TEST_ALIVE\"\nprintf 'transport %s\\n' \"$$\" > \"$WSP_TEST_EVENTS\"\n\"$WSP_TEST_DESCENDANT\" &\nwait \"$!\"\n").unwrap();
    for executable in [&descendant, &ssh] {
        std::fs::set_permissions(executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let config = tmp.path().join("gitconfig");
    std::fs::write(
        &config,
        format!("[core]\n\tsshCommand = {}\n", ssh.display()),
    )
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_wsp"))
        .args([
            "registry",
            "add",
            "git@github.com:test/sample.git",
            "--json",
        ])
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("XDG_DATA_HOME", tmp.path().join("data"))
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_SSH_VARIANT", "ssh")
        .env("WSP_TEST_EVENTS", &events)
        .env("WSP_TEST_ALIVE", tmp.path().join("alive"))
        .env("WSP_TEST_RELEASE", &release)
        .env("WSP_TEST_DESCENDANT", &descendant)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut audit = Vec::new();
    // FIFO readiness, not elapsed time, establishes that a grandchild exists
    // before cancellation. These timeouts only bound missing fixture handshakes.
    for _ in 0..2 {
        if let Ok(event) = event_receive.recv_timeout(Duration::from_secs(30)) {
            audit.push(event);
        } else {
            break;
        }
    }
    let ready = audit.iter().any(|event| event.starts_with("transport "))
        && audit.iter().any(|event| event.starts_with("descendant "));
    let interrupted = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap()
        .success();
    let closed = closed_receive.recv_timeout(Duration::from_secs(30));
    // Failure cleanup uses only PIDs published by this fixture; cleanup cannot
    // turn a failed EOF observation into a pass.
    if !ready || closed.is_err() {
        for event in &audit {
            if let Some((_, pid)) = event.split_once(' ') {
                let _ = Command::new("kill").args(["-KILL", pid]).status();
            }
        }
        let _ = child.kill();
    }
    let output = child.wait_with_output().unwrap();
    writeln!(event_control, "stop").unwrap();
    event_thread.join().unwrap();
    if ready {
        alive_reader.join().unwrap();
    }
    assert!(
        ready,
        "transport and descendant must reach the readiness barrier: {audit:?}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        interrupted,
        "could not deliver SIGINT to the real wsp process"
    );
    assert!(
        matches!(closed, Ok(Ok(ref bytes)) if bytes.is_empty()),
        "SIGINT must close every detached descendant's liveness descriptor: {closed:?}; audit={audit:?}"
    );
    assert_eq!(
        output.status.code(),
        Some(130),
        "cancellation status: {output:?}"
    );
    assert!(
        !tmp.path()
            .join("data/wsp/mirrors/github.com/test/sample.git")
            .exists(),
        "cancelled clone published an incomplete mirror"
    );
}
