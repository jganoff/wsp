//! Real command wiring for an opt-in API read that stays quiet until released.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn status_names_a_blocked_pr_lookup_before_it_finishes() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let workspaces = tmp.path().join("workspaces");
    let bin = tmp.path().join("bin");
    let release = tmp.path().join("release");
    std::fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        "#!/bin/sh\nwhile [ ! -f \"$WSP_TEST_RELEASE\" ]; do sleep 0.02; done\nprintf '[]\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    let make_command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_wsp"));
        command
            .env("HOME", tmp.path())
            .env("XDG_DATA_HOME", &data)
            .current_dir(tmp.path());
        command
    };
    for args in [
        vec![
            "config",
            "set",
            "workspaces-dir",
            workspaces.to_str().unwrap(),
            "--global",
        ],
        vec!["config", "set", "pr.source", "github", "--global"],
        vec!["new", "lookup", "--empty"],
    ] {
        let output = make_command().args(&args).output().unwrap();
        assert!(
            output.status.success(),
            "fixture {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let workspace = workspaces.join("lookup");
    let metadata_path = workspace.join(".wsp.yaml");
    let metadata = std::fs::read_to_string(&metadata_path).unwrap();
    assert!(
        metadata.contains("repos: {}"),
        "fixture metadata: {metadata}"
    );
    std::fs::write(
        &metadata_path,
        metadata.replace("repos: {}", "repos:\n  github.com/test/lookup: null"),
    )
    .unwrap();
    let repo = workspace.join("lookup");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&repo)
            .status()
            .unwrap()
            .success()
    );

    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let mut child = make_command()
        .args(["st", "lookup", "--json"])
        .env("PATH", path)
        .env("WSP_TEST_RELEASE", &release)
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
    // The real CLI reveal clock cannot be injected across the process boundary.
    // The child remains blocked until feedback arrives; this deadline is only a
    // generous watchdog for a missing observation path, including on busy CI.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut lines = Vec::new();
    let mut seen = false;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                seen |= line.contains("pull request") && line.contains("lookup");
                lines.push(line);
                if seen {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    let still_running = child.try_wait().unwrap().is_none();
    std::fs::write(&release, "release").unwrap();
    let output = child.wait_with_output().unwrap();
    drop(rx);
    reader.join().unwrap();
    assert!(
        seen && still_running,
        "PR lookup must be named while gh is blocked; got {lines:?}"
    );
    assert!(output.status.success(), "status failed: {lines:?}");
    assert!(
        lines.iter().all(|line| !line.contains('\x1b')),
        "JSON progress contains ANSI: {lines:?}"
    );
    let status: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(status["workspace"], "lookup");
    assert_eq!(status["repos"][0]["identity"], "github.com/test/lookup");
}

#[test]
fn auxiliary_work_names_its_repo_and_hands_the_terminal_to_setup() {
    use std::sync::{Arc, Mutex};
    use wsp_core::lang::LanguageIntegration;
    use wsp_core::progress::{Event, Observer};

    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<Event>>);
    impl Observer for RecordingObserver {
        fn observe(&self, event: Event) -> bool {
            self.0.lock().unwrap().push(event);
            true
        }
    }
    let observer = Arc::new(RecordingObserver::default());
    let _installation = wsp_core::progress::install(observer.clone());
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("sample");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("go.mod"),
        "module example.com/sample\n\ngo 1.22\n",
    )
    .unwrap();
    let metadata: wsp_core::workspace::Metadata = serde_json::from_value(serde_json::json!({
        "name": "sample", "branch": "test/sample", "created": "2026-10-08T00:00:00Z",
        "repos": {"github.com/test/sample": null}
    }))
    .unwrap();
    assert!(wsp_core::lang::go::GoIntegration.detect(tmp.path(), &metadata));
    wsp_core::discovery::scan_repo_dir(
        &repo,
        "github.com/test/sample",
        &tmp.path().join("templates"),
    );
    let resolved = wsp_core::setup_commands::resolve(vec![wsp_core::setup_commands::SetupSource {
        label: "repo",
        commands: vec!["touch setup-ran".into()],
    }]);
    let data = tmp.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    wsp_core::approvals::record_always(
        &data,
        "github.com/test/sample",
        &wsp_core::approvals::commands_hash(&resolved.commands),
    )
    .unwrap();
    assert!(
        wsp_core::setup_runner::maybe_run_resolved(
            &data,
            &repo,
            "github.com/test/sample",
            &resolved
        )
        .unwrap()
    );
    assert!(
        repo.join("setup-ran").exists(),
        "approved setup must actually execute"
    );
    let events = observer.0.lock().unwrap();
    assert!(events.iter().any(|event| matches!(event, Event::Started {line, ..} if line == "Scanning Go modules in sample")), "Go scan must name its repo: {events:?}");
    assert!(events.iter().any(|event| matches!(event, Event::Started {line, ..} if line == "Discovering templates in github.com/test/sample")), "discovery must name its repo: {events:?}");
    let label = events.iter().position(|event| matches!(event, Event::Message(line) if line.contains("Running setup command 1/1 in sample"))).expect("setup command must be named before child owns terminal");
    let suspend = events[..label]
        .iter()
        .rposition(|event| matches!(event, Event::Suspended(true)))
        .expect("setup must suspend before its command label");
    assert!(
        !events[suspend..label]
            .iter()
            .any(|event| matches!(event, Event::Suspended(false))),
        "setup resumed before handing over terminal"
    );
    assert!(
        events[label + 1..]
            .iter()
            .any(|event| matches!(event, Event::Suspended(false))),
        "setup must resume after child finishes"
    );
}
