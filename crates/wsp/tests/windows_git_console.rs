//! Real Windows console access, independently of redirected standard streams.
#![cfg(windows)]

use std::ffi::OsStr;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const ROLE: &str = "WSP_WINDOWS_GIT_CONSOLE_TEST_ROLE";
const FIXTURE: &str = "windows_git_console_fixture";
const RECORD: &str = "WSP_CONSOLE_ACCESS ";
const HOST_PID: &str = "WSP_WINDOWS_GIT_CONSOLE_HOST_PID";
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

fn console_contains_host(host_pid: u32) -> bool {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;
    let mut processes = vec![0; 16];
    loop {
        // SAFETY: the writable buffer contains exactly the advertised number
        // of u32 entries and remains alive throughout this synchronous call.
        let count = unsafe { GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) }
            as usize;
        if count == 0 {
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.raw_os_error(),
                Some(6),
                "console query failed: {error}"
            );
            return false; // ERROR_INVALID_HANDLE: this process has no console.
        }
        if count > processes.len() {
            processes.resize(count, 0);
            continue;
        }
        return processes[..count].contains(&host_pid);
    }
}

fn console_access(role: &str) -> String {
    let host_pid = std::env::var(HOST_PID)
        .map(|pid| pid.parse().unwrap())
        .unwrap_or_else(|_| std::process::id());
    let caller = console_contains_host(host_pid);
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
    format!("{RECORD}role={role} input={input} output={output} caller={caller}")
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
        "WSP_CONSOLE_ACCESS role=host input=true output=true caller=true"
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
            command.env(HOST_PID, std::process::id().to_string());
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
            let actual = records(&output.stdout);
            let no_console = format!("{RECORD}role={role} input=false output=false caller=false");
            let private_console =
                format!("{RECORD}role={role} input=true output=true caller=false");
            let native_console = format!("{RECORD}role={role} input=true output=true caller=true");
            // The selected Git launcher may allocate an independent invisible
            // console. It must never share the still-running host's console.
            // The direct child we control must have no console at all.
            let valid = if expects_isolation {
                actual == [no_console] || (role == "helper" && actual == [private_console])
            } else {
                actual == [native_console]
            };
            assert!(
                valid,
                "console isolation assertion: detached={detached}; negative_control={negative_control}; role={role}; records={actual:?}"
            );
        }
    }
    if !negative_control {
        cancel_git_helper(&helper_path);
    }
    println!("\nWSP_WINDOWS_CONSOLE_ISOLATION_VERIFIED");
}

fn cancel_git_helper(helper_path: &OsStr) {
    let mut command = wsp_core::git_process::command(OsStr::new("git"), true, None).unwrap();
    command.arg("wsp-console-fixture").env("PATH", helper_path);
    configure_fixture(&mut command, "waiting-helper");
    command.stdin(Stdio::piped());
    let child = wsp_core::git_process::spawn(&mut command, true).unwrap();
    let mut input = child.take_stdin().unwrap();
    input.write_all(&[1]).unwrap();
    input.flush().unwrap();
    let mut stdout = BufReader::new(child.take_stdout().unwrap());
    let mut stderr = child.take_stderr().unwrap();
    let (send, receive) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in stdout.by_ref().lines() {
            send.send(Some(line.unwrap())).unwrap();
        }
        send.send(None).unwrap();
    });
    let error_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).unwrap();
        bytes
    });
    loop {
        // Output establishes readiness. Time only bounds a broken fixture.
        match receive.recv_timeout(Duration::from_secs(60)) {
            Ok(Some(line)) if line == "WSP_GIT_HELPER_WAITING" => break,
            Ok(Some(_)) => {}
            event => {
                child.kill_tree().unwrap();
                child.wait().unwrap();
                panic!("Git helper did not reach its input barrier: {event:?}");
            }
        }
    }
    child.kill_tree().unwrap();
    // Keeping the input writer alive rules out EOF as the cause of helper exit.
    while receive
        .recv_timeout(Duration::from_secs(60))
        .unwrap()
        .is_some()
    {}
    assert!(!child.wait().unwrap().success(), "cancelled Git succeeded");
    reader.join().unwrap();
    error_reader.join().unwrap();
    drop(input);
}

#[test]
fn windows_git_console_fixture() {
    match std::env::var(ROLE).as_deref() {
        Ok("host") => host(false),
        Ok("negative-control") => host(true),
        Ok("waiting-helper") => {
            let mut handshake = [0];
            std::io::stdin().read_exact(&mut handshake).unwrap();
            assert_eq!(handshake, [1]);
            println!("\nWSP_GIT_HELPER_WAITING");
            std::io::stdout().flush().unwrap();
            // A readiness barrier models a stalled helper without requiring
            // one particular Git launcher's private-console allocation policy.
            std::io::stdin().read_exact(&mut [0]).unwrap();
            panic!("waiting helper received unexpected input");
        }
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

fn capture_host(role: &str) -> Output {
    use process_wrap::std::{CommandWrap, CreationFlags, JobObject};
    let mut host = CommandWrap::from(fixture_command(role));
    let mut flags = CreationFlags(Default::default());
    flags.0.0 = CREATE_NEW_CONSOLE;
    host.wrap(flags).wrap(JobObject);
    let mut child = host.spawn().unwrap();
    let (send, receive) = mpsc::channel();
    let streams: [Box<dyn Read + Send>; 2] = [
        Box::new(child.stdout().take().unwrap()),
        Box::new(child.stderr().take().unwrap()),
    ];
    let readers: Vec<_> = streams
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
    // Bounds every inner fixture wait, including regressions in cancellation.
    // The outer job owns the host and descendants even if inner cleanup fails.
    let deadline = Instant::now() + Duration::from_secs(300);
    let mut captured = [Vec::new(), Vec::new()];
    for _ in 0..2 {
        match receive.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((index, result)) => captured[index] = result.unwrap(),
            Err(error) => {
                let _ = child.start_kill();
                let _ = child.inner_mut().wait();
                panic!("console host {role} did not close its pipes: {error}");
            }
        }
    }
    let status = loop {
        if let Some(status) = child.inner_mut().try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.start_kill();
            let _ = child.inner_mut().wait();
            panic!("console host {role} did not exit");
        }
        std::thread::yield_now();
    };
    for reader in readers {
        reader.join().unwrap();
    }
    let [stdout, stderr] = captured;
    Output {
        status,
        stdout,
        stderr,
    }
}

#[test]
fn windows_git_and_its_external_helper_are_isolated_from_the_caller_console() {
    // A disposable console belongs only to this host. The runner's console is
    // never opened or modified, and inherited stdio is deliberately captured.
    let output = capture_host("host");
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
    let negative = capture_host("negative-control");
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
