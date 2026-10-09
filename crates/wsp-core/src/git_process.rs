//! Git child lifetime and terminal isolation.
//!
//! Build commands with [`command`] before applying arguments, environment, and
//! stdio. Drain captured streams before [`TrackedChild::wait`]. Detached Unix
//! children retain their process-group identity until their final reap, so
//! cancellation cannot signal an unrelated process after PID reuse.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::Path;
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};

#[cfg(unix)]
type ProcessChild = std::process::Child;
#[cfg(windows)]
type ProcessChild = Box<dyn process_wrap::std::ChildWrapper>;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

pub const TRAMPOLINE_MARKER: &str = "--wsp-internal-git-exec";

#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

#[derive(Default)]
struct Registry {
    cancelled: bool,
    children: BTreeMap<u32, Arc<Mutex<ManagedChild>>>,
}

struct ManagedChild {
    child: ProcessChild,
    detached: bool,
    status: Option<ExitStatus>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Construct the child before configuring args, environment, cwd, or stdio.
///
/// An existing `Command` cannot be reconstructed faithfully: Rust exposes
/// neither its stdio configuration nor whether `env_clear()` was called.
/// A detached Unix command therefore needs the CLI's executable up front.
pub fn command(program: &OsStr, detached: bool, launcher: Option<&Path>) -> io::Result<Command> {
    #[cfg(unix)]
    if detached {
        let launcher = launcher.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "detached Git requires the wsp executable as its launcher",
            )
        })?;
        let mut command = Command::new(launcher);
        command.arg(TRAMPOLINE_MARKER).arg(program);
        return Ok(command);
    }
    let command = Command::new(program);
    #[cfg(windows)]
    let command = {
        use std::os::windows::process::CommandExt;
        let _ = launcher;
        let mut command = command;
        if detached {
            // Spawn applies the same console flag through the Job Object
            // wrapper, preserving it while the child is briefly suspended.
            command.creation_flags(DETACHED_PROCESS);
        }
        command
    };
    Ok(command)
}

/// Enter the detached Unix session before any CLI threads or handlers exist.
/// `args` starts with the original program and contains literal OS arguments.
pub fn trampoline(args: &[OsString]) -> io::Result<Infallible> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let (program, args) = args.split_first().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing Git subprocess program",
            )
        })?;
        rustix::process::setsid()?;
        Err(Command::new(program).args(args).exec())
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the Git session trampoline is only used on Unix",
        ))
    }
}

/// Spawn and register under one lock so cancellation cannot miss a new child.
/// `detached` must match the value passed to [`command`].
pub fn spawn(command: &mut Command, detached: bool) -> io::Result<TrackedChild> {
    let mut registry = lock(registry());
    if registry.cancelled {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Git execution cancelled",
        ));
    }
    let child = spawn_process(command, detached)?;
    let id = child.id();
    let child = Arc::new(Mutex::new(ManagedChild {
        child,
        detached,
        status: None,
    }));
    registry.children.insert(id, Arc::clone(&child));
    Ok(TrackedChild {
        id,
        child,
        registered: true,
    })
}

#[cfg(unix)]
fn spawn_process(command: &mut Command, _: bool) -> io::Result<ProcessChild> {
    command.spawn()
}

#[cfg(windows)]
fn spawn_process(command: &mut Command, detached: bool) -> io::Result<ProcessChild> {
    use process_wrap::std::{CommandWrap, CreationFlags, JobObject};
    if !detached {
        return command.spawn().map(|child| Box::new(child) as ProcessChild);
    }
    // Move the actual Command rather than reconstructing its private stdio or
    // environment state. Restore it even when spawning fails.
    let original = std::mem::replace(command, Command::new(""));
    let mut wrapped = CommandWrap::from(original);
    let mut flags = CreationFlags(Default::default());
    // Git must have no console so its helpers also detect absent console input.
    flags.0.0 = DETACHED_PROCESS;
    wrapped.wrap(flags).wrap(JobObject);
    // JobObject starts the process suspended, assigns it, and only then resumes
    // its threads. Assignment or resume errors terminate the suspended child.
    let result = wrapped.spawn();
    *command = wrapped.into_command();
    result
}

/// An unreaped child. Dropping the handle cancels and reaps it.
/// Capture streams may be moved to readers while this handle remains alive.
pub struct TrackedChild {
    id: u32,
    child: Arc<Mutex<ManagedChild>>,
    registered: bool,
}

impl TrackedChild {
    pub fn take_stdout(&self) -> Option<ChildStdout> {
        {
            let mut child = lock(&self.child);
            #[cfg(unix)]
            {
                child.child.stdout.take()
            }
            #[cfg(windows)]
            {
                child.child.stdout().take()
            }
        }
    }

    pub fn take_stderr(&self) -> Option<ChildStderr> {
        {
            let mut child = lock(&self.child);
            #[cfg(unix)]
            {
                child.child.stderr.take()
            }
            #[cfg(windows)]
            {
                child.child.stderr().take()
            }
        }
    }

    pub fn take_stdin(&self) -> Option<ChildStdin> {
        {
            let mut child = lock(&self.child);
            #[cfg(unix)]
            {
                child.child.stdin.take()
            }
            #[cfg(windows)]
            {
                child.child.stdin().take()
            }
        }
    }

    pub fn kill_tree(&self) -> io::Result<()> {
        let registry = lock(registry());
        if registry.children.contains_key(&self.id) {
            kill_tree(&mut lock(&self.child))
        } else {
            Ok(())
        }
    }

    /// Call after captured streams have drained. The exit observation does not
    /// reap on Unix, and does not hold a lock needed by cancellation.
    pub fn wait(mut self) -> io::Result<ExitStatus> {
        self.finish()
    }

    /// Observe exit without reaping. Safe while output readers are still active.
    /// A bounded caller can poll this alongside reader completion and its own
    /// deadline, cancel on expiry, then drain streams and call `wait()`.
    pub fn has_exited(&self) -> io::Result<bool> {
        let _registry = lock(registry());
        observe_exit(&mut lock(&self.child))
    }

    fn finish(&mut self) -> io::Result<ExitStatus> {
        // Like Child::wait, close any stdin the caller did not take so a child
        // waiting for EOF can exit. Taken input remains the caller's ownership.
        drop(self.take_stdin());
        loop {
            let mut registry = lock(registry());
            let mut child = lock(&self.child);
            if observe_exit(&mut child)? {
                let status = reap(&mut child)?;
                registry.children.remove(&self.id);
                self.registered = false;
                return Ok(status);
            }
            drop(child);
            drop(registry);
            // Production polling keeps cancellation available without allowing
            // a concurrent cancellation reap to race a PID-based wait syscall.
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

impl Drop for TrackedChild {
    fn drop(&mut self) {
        if self.registered {
            let _ = self.kill_tree();
            let _ = self.finish();
        }
    }
}

/// Close admission, kill all registered children, then reap their leaders.
/// Invoke from a normal cancellation thread, never a raw signal handler.
/// Detached descendants are killed before a leader's PID can be reused.
pub fn cancel_all() -> io::Result<()> {
    let mut registry = lock(registry());
    registry.cancelled = true;
    let mut first_error = None;
    for child in registry.children.values() {
        if let Err(error) = kill_tree(&mut lock(child)) {
            first_error.get_or_insert(error);
        }
    }
    registry.children.retain(|_, child| {
        if let Err(error) = reap(&mut lock(child)) {
            first_error.get_or_insert(error);
            true
        } else {
            false
        }
    });
    first_error.map_or(Ok(()), Err)
}

fn reap(child: &mut ManagedChild) -> io::Result<ExitStatus> {
    if let Some(status) = child.status {
        return Ok(status);
    }
    #[cfg(unix)]
    let status = child.child.wait()?;
    #[cfg(windows)]
    let status = child.child.inner_mut().wait()?;
    child.status = Some(status);
    Ok(status)
}

#[cfg(unix)]
fn process_id(id: u32) -> io::Result<rustix::process::Pid> {
    i32::try_from(id)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid child process ID"))
}

#[cfg(unix)]
fn observe_exit(child: &mut ManagedChild) -> io::Result<bool> {
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    if child.status.is_some() {
        return Ok(true);
    }
    let id = process_id(child.child.id())?;
    loop {
        match waitid(
            WaitId::Pid(id),
            WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG,
        ) {
            Ok(status) => return Ok(status.is_some()),
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(windows)]
fn observe_exit(child: &mut ManagedChild) -> io::Result<bool> {
    if child.status.is_none() {
        // Windows Child owns a process handle independently of PID reuse.
        // Poll the leader without consuming the Job Object completion queue.
        // The retained wrapper still owns the job and can terminate descendants
        // after the leader exits while they hold capture pipes open.
        child.status = child.child.inner_mut().try_wait()?;
    }
    Ok(child.status.is_some())
}

fn kill_tree(child: &mut ManagedChild) -> io::Result<()> {
    if child.status.is_some() {
        #[cfg(unix)]
        return Ok(());
        #[cfg(windows)]
        if !child.detached {
            return Ok(());
        }
    }
    #[cfg(unix)]
    {
        use rustix::process::{Signal, kill_process_group};
        let id = process_id(child.child.id())?;
        let mut first_error = None;
        let mut kill_group = || {
            if child.detached {
                match kill_process_group(id, Signal::KILL) {
                    Ok(()) | Err(rustix::io::Errno::SRCH) => {}
                    Err(error) => {
                        first_error.get_or_insert(io::Error::from(error));
                    }
                }
            }
        };
        kill_group();
        // The child might still be before setsid, so group signalling alone
        // cannot cancel it. Signal the leader and retry the group afterward.
        let direct = child.child.kill();
        kill_group();
        if let Err(error) = direct
            && error.kind() != io::ErrorKind::InvalidInput
        {
            first_error.get_or_insert(error);
        }
        first_error.map_or(Ok(()), Err)
    }
    #[cfg(windows)]
    {
        // JobObjectChild::start_kill uses TerminateJobObject, including any
        // descendants alive after their original parent has exited.
        child.child.start_kill()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn factory_preserves_literal_program_and_arguments() {
        use std::os::unix::ffi::OsStringExt;
        for detached in [false, true] {
            let program = OsString::from_vec(b"git-\xff".to_vec());
            let arguments = [OsString::from("a b"), OsString::from("$(touch unwanted)")];
            let mut command = command(&program, detached, Some(Path::new("/wsp"))).unwrap();
            command.args(&arguments).env_clear().env("ONLY", "value");
            let expected_program = if detached {
                OsStr::new("/wsp")
            } else {
                &program
            };
            assert_eq!(command.get_program(), expected_program);
            let mut expected = Vec::new();
            if detached {
                expected.extend([OsStr::new(TRAMPOLINE_MARKER), program.as_os_str()]);
            }
            expected.extend(arguments.iter().map(OsString::as_os_str));
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
            assert_eq!(
                command.get_envs().collect::<Vec<_>>(),
                [(OsStr::new("ONLY"), Some(OsStr::new("value")))]
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn native_child_output_and_status_are_preserved() {
        use std::io::Read;
        use std::process::Stdio;
        for code in [0, 7] {
            let mut command = command(OsStr::new("sh"), false, None).unwrap();
            command.args([
                "-c",
                &format!("printf output; printf error >&2; exit {code}"),
            ]);
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
            let child = spawn(&mut command, false).unwrap();
            let mut stdout = String::new();
            let mut stderr = String::new();
            child
                .take_stdout()
                .unwrap()
                .read_to_string(&mut stdout)
                .unwrap();
            child
                .take_stderr()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            let status = child.wait().unwrap();
            assert_eq!(status.code(), Some(code));
            assert_eq!(stdout, "output");
            assert_eq!(stderr, "error");
        }
    }
    #[cfg(unix)]
    #[test]
    fn native_cancellation_remains_available_after_streams_close() {
        use std::io::Read;
        use std::process::Stdio;
        use std::sync::mpsc;
        use std::time::Duration;

        let mut command = command(OsStr::new("sh"), false, None).unwrap();
        command
            .args(["-c", "printf ready; exec 1>&- 2>&-; IFS= read -r answer"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let child = spawn(&mut command, false).unwrap();
        let input = child.take_stdin().unwrap();
        let id = child.id;
        let shared = Arc::clone(&child.child);
        let mut output = String::new();
        child
            .take_stdout()
            .unwrap()
            .read_to_string(&mut output)
            .unwrap();
        child
            .take_stderr()
            .unwrap()
            .read_to_end(&mut Vec::new())
            .unwrap();
        assert_eq!(output, "ready");
        let (started_send, started_receive) = mpsc::channel();
        let (done_send, done_receive) = mpsc::channel();
        let waiter = std::thread::spawn(move || {
            started_send.send(()).unwrap();
            done_send.send(child.wait()).unwrap();
        });
        // Channel and child output handshakes establish ordering. These bounds
        // only diagnose a stuck fixture or a lock held across the blocking wait.
        started_receive
            .recv_timeout(Duration::from_secs(30))
            .unwrap();
        {
            let registry = lock(registry());
            assert!(registry.children.contains_key(&id));
            kill_tree(&mut lock(&shared)).unwrap();
        }
        let status = done_receive
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .unwrap();
        assert!(!status.success());
        waiter.join().unwrap();
        drop(input);
        assert!(!lock(registry()).children.contains_key(&id));
    }
    #[cfg(windows)]
    #[test]
    fn windows_descendant_fixture() {
        use std::io::{Read, Write};
        use std::process::Stdio;
        const FIXTURE: &str = "WSP_GIT_PROCESS_FIXTURE";
        match std::env::var(FIXTURE).as_deref() {
            Ok("parent") => {
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "git_process::tests::windows_descendant_fixture",
                        "--nocapture",
                    ])
                    .env(FIXTURE, "descendant")
                    .stdin(Stdio::inherit())
                    .stdout(Stdio::inherit())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .unwrap();
                std::process::exit(0);
            }
            Ok("descendant") => {
                println!("descendant-ready");
                std::io::stdout().flush().unwrap();
                // The test retains the input writer. Only job cancellation may
                // terminate this descendant while its inherited pipe is open.
                let _ = std::io::stdin().read_exact(&mut [0]);
                std::process::exit(77);
            }
            _ => {}
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_job_cancels_descendant_after_parent_exits() {
        use std::io::{BufRead, BufReader, Read};
        use std::process::Stdio;
        use std::sync::mpsc;
        use std::time::{Duration, Instant};
        for drop_handle in [false, true] {
            let mut command =
                command(std::env::current_exe().unwrap().as_os_str(), true, None).unwrap();
            command
                .args([
                    "--exact",
                    "git_process::tests::windows_descendant_fixture",
                    "--nocapture",
                ])
                .env("WSP_GIT_PROCESS_FIXTURE", "parent")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let child = spawn(&mut command, true).unwrap();
            let input = child.take_stdin().unwrap();
            let stdout = child.take_stdout().unwrap();
            let stderr = child.take_stderr().unwrap();
            let (send, receive) = mpsc::channel();
            let stdout_reader = std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    send.send(Some(line.unwrap())).unwrap();
                }
                send.send(None).unwrap();
            });
            let stderr_reader = std::thread::spawn(move || {
                let mut bytes = Vec::new();
                BufReader::new(stderr).read_to_end(&mut bytes).unwrap();
            });
            // Readiness, process exit, and pipe EOF are observed independently.
            // This deadline only bounds missing fixture events on a stuck run.
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                let line = receive
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap();
                if line.as_deref() == Some("descendant-ready") {
                    break;
                }
                assert!(line.is_some(), "descendant exited before readiness");
            }
            while !child.has_exited().unwrap() {
                assert!(Instant::now() < deadline, "fixture parent did not exit");
                std::thread::yield_now();
            }
            if drop_handle {
                drop(child);
            } else {
                child.kill_tree().unwrap();
                assert!(
                    child.wait().unwrap().success(),
                    "parent exit status changed"
                );
            }
            // A surviving descendant keeps stdout open; EOF therefore verifies
            // cancellation even though the parent had already exited normally.
            while receive
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap()
                .is_some()
            {}
            stdout_reader.join().unwrap();
            stderr_reader.join().unwrap();
            drop(input);
        }
    }
}
