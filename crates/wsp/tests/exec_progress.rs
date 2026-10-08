//! Permanent command output must not share the terminal row with live progress.
#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn exec_header_clears_progress_before_printing() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let wsp_data = data.join("wsp");
    let workspaces = tmp.path().join("workspaces");
    let workspace = workspaces.join("demo");
    let repo = workspace.join("repo");
    fs::create_dir_all(&wsp_data).unwrap();
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&workspaces).unwrap();
    fs::write(
        workspace.join(".wsp.yaml"),
        "name: demo\nbranch: demo\ncreated: \"2026-10-08T00:00:00Z\"\nrepos:\n  github.com/test/repo: null\n",
    )
    .unwrap();

    let config = wsp_data.join("config.yaml");
    assert!(
        Command::new("mkfifo")
            .arg(&config)
            .status()
            .unwrap()
            .success()
    );
    // Keep one read/write handle so the writer thread can always unblock, even
    // if a regression makes wsp exit before opening the FIFO.
    let _fifo_guard = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&config)
        .unwrap();

    let contents = format!("workspaces_dir: {}\nhints: false\n", workspaces.display());
    let (release_tx, release_rx) = mpsc::channel();
    let writer_path = config.clone();
    let writer_contents = contents.clone();
    let writer = thread::spawn(move || {
        let mut pipe = OpenOptions::new().write(true).open(&writer_path).unwrap();
        // The test releases the config only after it sees the live progress
        // frame. The timeout only prevents a broken test from hanging CI.
        let _ = release_rx.recv_timeout(Duration::from_secs(30));
        let regular = writer_path.with_extension("regular");
        fs::write(&regular, &writer_contents).unwrap();
        fs::rename(&regular, &writer_path).unwrap();
        pipe.write_all(writer_contents.as_bytes()).unwrap();
    });

    let launcher = tmp.path().join("launch-wsp");
    fs::write(
        &launcher,
        "#!/bin/sh\nexec \"$WSP_TEST_BINARY\" exec demo -- echo CHILD\n",
    )
    .unwrap();
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o755)).unwrap();

    let mut script = Command::new("script");
    if cfg!(target_os = "macos") {
        script.args(["-q", "/dev/null"]).arg(&launcher);
    } else {
        // util-linux script takes a command string; the temporary path has no
        // shell metacharacters or spaces.
        script
            .args(["-q", "-e", "-c"])
            .arg(format!("'{}'", launcher.display()))
            .arg("/dev/null");
    }
    let mut child = script
        .current_dir(tmp.path())
        .env("HOME", tmp.path())
        .env("XDG_DATA_HOME", &data)
        .env("WSP_TEST_BINARY", env!("CARGO_BIN_EXE_wsp"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let stdout = child.stdout.take().unwrap();
    let (output_tx, output_rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let mut stdout = stdout;
        let mut buffer = [0; 4096];
        loop {
            let count = stdout.read(&mut buffer).unwrap();
            if count == 0 || output_tx.send(buffer[..count].to_vec()).is_err() {
                break;
            }
        }
    });

    // The FIFO and observed progress frame order the behavior deterministically.
    // This deadline only avoids hanging if the process or PTY setup regresses.
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut output = Vec::new();
    let saw_progress = loop {
        if output
            .windows(b"Reading ".len())
            .any(|window| window == b"Reading ")
        {
            break true;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break false;
        }
        match output_rx.recv_timeout(remaining) {
            Ok(chunk) => output.extend(chunk),
            Err(_) => break false,
        }
    };
    let _ = release_tx.send(());
    writer.join().unwrap();
    drop(_fifo_guard);

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if Instant::now() >= deadline {
            break None;
        }
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        match output_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => output.extend(chunk),
            Err(mpsc::RecvTimeoutError::Disconnected) => break child.try_wait().unwrap(),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
        }
    };
    if status.is_none() {
        let _ = child.kill();
    }
    let status = status.unwrap_or_else(|| child.wait().unwrap());
    reader.join().unwrap();
    while let Ok(chunk) = output_rx.try_recv() {
        output.extend(chunk);
    }
    assert!(saw_progress, "slow config read did not show progress");
    assert!(
        status.success(),
        "wsp exec PTY failed: {}",
        String::from_utf8_lossy(&output)
    );

    let header = output
        .windows(b"==> [repo]".len())
        .position(|window| window == b"==> [repo]")
        .expect("repo header is missing");
    let clear = output[..header]
        .windows(b"\x1b[2K".len())
        .rposition(|window| window == b"\x1b[2K")
        .expect("progress frame was not cleared before the header");
    let after_clear = clear + b"\x1b[2K".len();
    assert!(
        output[after_clear..header]
            .iter()
            .all(|byte| matches!(byte, b'\r' | b'\n' | b' ')),
        "a progress frame was written after the final clear and before the header: {}",
        String::from_utf8_lossy(&output[clear..header])
    );
    assert!(
        String::from_utf8_lossy(&output)
            .lines()
            .any(|line| line.trim() == "CHILD"),
        "child output was lost: {}",
        String::from_utf8_lossy(&output)
    );
}
