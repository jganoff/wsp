//! Real Windows console access, independently of redirected standard streams.
#![cfg(windows)]

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const ROLE: &str = "WSP_WINDOWS_GIT_CONSOLE_TEST_ROLE";
const FIXTURE: &str = "windows_git_console_fixture";
const RECORD: &str = "WSP_CONSOLE_ACCESS ";
const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;

fn fixture_command(role: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    configure_fixture(&mut command, role);
    command
}

fn configure_fixture(command: &mut Command, role: &str) {
    command
        .args(["--exact", FIXTURE, "--nocapture", "--test-threads=1"])
        .env(ROLE, role)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}

fn console_access(role: &str) -> String {
    let input = OpenOptions::new().read(true).open("CONIN$").is_ok();
    let output = match OpenOptions::new().write(true).open("CONOUT$") {
        Ok(mut console) => {
            console
                .write_all(format!("wsp-private-console-{role}\r\n").as_bytes())
                .unwrap();
            true
        }
        Err(_) => false,
    };
    format!("{RECORD}role={role} input={input} output={output}")
}

fn records(output: &[u8]) -> Vec<String> {
    String::from_utf8(output.to_vec())
        .unwrap()
        .lines()
        .filter(|line| line.starts_with(RECORD))
        .map(str::to_owned)
        .collect()
}

fn capture(child: wsp_core::git_process::TrackedChild) -> Output {
    let stdout = child.take_stdout().unwrap();
    let stderr = child.take_stderr().unwrap();
    let (send, receive) = mpsc::channel();
    let readers: Vec<_> = [
        Box::new(stdout) as Box<dyn Read + Send>,
        Box::new(stderr) as Box<dyn Read + Send>,
    ]
    .into_iter()
    .enumerate()
    .map(|(index, mut stream)| {
        let send = send.clone();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = stream.read_to_end(&mut bytes).map(|_| bytes);
            let _ = send.send((index, result));
        })
    })
    .collect();
    drop(send);
    let mut captured = [Vec::new(), Vec::new()];
    for _ in 0..2 {
        // Pipe EOF is the completion event. This generous timeout only
        // bounds a stuck fixture, never establishes console isolation.
        match receive.recv_timeout(Duration::from_secs(60)) {
            Ok((index, result)) => captured[index] = result.unwrap(),
            Err(error) => {
                child.kill_tree().unwrap();
                child.wait().unwrap();
                panic!("console fixture did not close its pipes: {error}");
            }
        }
    }
    for reader in readers {
        reader.join().unwrap();
    }
    let status = child.wait().unwrap();
    let [stdout, stderr] = captured;
    Output {
        status,
        stdout,
        stderr,
    }
}

fn host(negative_control: bool) {
    // This positive control prevents headless CI from passing because no
    // console exists. Every probe has redirected stdin, stdout, and stderr.
    assert_eq!(
        console_access("host"),
        "WSP_CONSOLE_ACCESS role=host input=true output=true"
    );
    let executable = std::env::current_exe().unwrap();
    let helper_dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        &executable,
        helper_dir.path().join("git-wsp-console-fixture.exe"),
    )
    .unwrap();
    let inherited_path = std::env::var_os("PATH").unwrap_or_default();
    let helper_path = std::env::join_paths(
        std::iter::once(helper_dir.path().to_path_buf())
            .chain(std::env::split_paths(&inherited_path)),
    )
    .unwrap();
    for detached in [false, true] {
        for role in ["probe", "helper"] {
            let program = if role == "probe" {
                executable.as_os_str()
            } else {
                OsStr::new("git")
            };
            let mut command = wsp_core::git_process::command(program, detached, None).unwrap();
            if role == "helper" {
                // Git resolves this external command from PATH and owns the
                // helper's launch policy, with no synthetic descendant flags.
                command.arg("wsp-console-fixture").env("PATH", &helper_path);
            }
            configure_fixture(&mut command, role);
            let child = wsp_core::git_process::spawn(&mut command, detached).unwrap();
            let output = capture(child);
            assert!(
                output.status.success(),
                "role={role}; detached={detached}: {}; stdout={}; stderr={}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let expects_isolation = detached || negative_control;
            let expected = format!(
                "{RECORD}role={role} input={} output={}",
                !expects_isolation, !expects_isolation
            );
            assert_eq!(
                records(&output.stdout),
                [expected],
                "console isolation assertion: detached={detached}; negative_control={negative_control}; role={role}"
            );
        }
    }
    println!("\nWSP_WINDOWS_CONSOLE_ISOLATION_VERIFIED");
}

#[test]
fn windows_git_console_fixture() {
    match std::env::var(ROLE).as_deref() {
        Ok("host") => host(false),
        Ok("negative-control") => host(true),
        Ok(role @ ("probe" | "helper")) => {
            // Start on a fresh line because libtest may have printed the test
            // name without a newline before invoking this fixture.
            println!("\n{}", console_access(role));
        }
        Ok(role) => panic!("unknown console fixture role: {role}"),
        Err(_) => return,
    }
    std::io::stdout().flush().unwrap();
    std::process::exit(0);
}

#[test]
fn windows_git_and_its_external_helper_are_isolated_from_the_console() {
    // A disposable console belongs only to this host. The runner's console is
    // never opened or modified, and inherited stdio is deliberately captured.
    let output = fixture_command("host")
        .creation_flags(CREATE_NEW_CONSOLE)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "console host failed: {}; stdout={}; stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line == "WSP_WINDOWS_CONSOLE_ISOLATION_VERIFIED"),
        "console fixture did not verify both policies"
    );
    // Deliberately require isolation from a native child with a real console.
    // The same assertions must reject it, proving their runtime sensitivity.
    let negative = fixture_command("negative-control")
        .creation_flags(CREATE_NEW_CONSOLE)
        .output()
        .unwrap();
    assert!(
        !negative.status.success()
            && String::from_utf8_lossy(&negative.stderr)
                .contains("console isolation assertion: detached=false; negative_control=true"),
        "native negative control failed for the wrong reason: {}; stdout={}; stderr={}",
        negative.status,
        String::from_utf8_lossy(&negative.stdout),
        String::from_utf8_lossy(&negative.stderr)
    );
}
