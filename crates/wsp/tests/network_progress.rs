//! Registry discovery must identify a quiet GitHub lookup before it completes.
#![cfg(unix)]

use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[test]
fn registry_discovery_names_the_owner_while_gh_is_blocked() {
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    let data = temp.path().join("data");
    let release = temp.path().join("release");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(data.join("wsp")).unwrap();
    let gh = bin.join("gh");
    std::fs::write(
        &gh,
        "#!/bin/sh\nwhile [ ! -f \"$WSP_TEST_RELEASE\" ]; do sleep 0.02; done\nprintf '[]\\n'\n",
    )
    .unwrap();
    std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
    wsp_core::config::Config {
        workspaces_dir: Some(temp.path().join("workspaces").display().to_string()),
        hints: Some(false),
        ..Default::default()
    }
    .save_to(&data.join("wsp/config.yaml"))
    .unwrap();
    let binary = std::env::var_os("WSP_PROGRESS_BASELINE")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_wsp").into());
    let mut child = Command::new(binary)
        .args([
            "registry",
            "add",
            "--from",
            "github.com/acme",
            "--all",
            "--json",
        ])
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", &data)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("WSP_TEST_RELEASE", &release)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stderr = child.stderr.take().unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            if send.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    // This generous deadline is a missing-event watchdog, not a timing assertion.
    // The fixture cannot complete until the parent sees feedback and releases it.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut lines = Vec::new();
    let mut seen = false;
    while Instant::now() < deadline {
        match receive.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                seen |= line.contains("Looking up GitHub repositories for acme");
                lines.push(line);
                if seen {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    let blocked = child.try_wait().unwrap().is_none();
    std::fs::write(&release, "release").unwrap();
    let output = child.wait_with_output().unwrap();
    drop(receive);
    reader.join().unwrap();
    assert!(
        seen && blocked,
        "registry lookup must identify its owner while gh is blocked: {lines:?}"
    );
    assert!(
        lines.iter().all(|line| !line.contains('\x1b')),
        "JSON lookup progress must be plain: {lines:?}"
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["error"], "no repos matched",
        "lookup must preserve the empty result: {result}"
    );
}
