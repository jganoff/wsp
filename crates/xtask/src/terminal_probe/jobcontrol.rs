//! Disposable same-session job-control experiment. Every terminal here is synthetic.
//! No descriptor from the invoking user's terminal is used or modified.

use anyhow::{Context, Result, bail, ensure};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt;
use std::process::{ChildStderr, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

const WATCHDOG: Duration = Duration::from_secs(20);
const FRAME: &[u8] = b"\r[==      ] example/repo fetching";

const CLEANUP_WATCHDOG: Duration = Duration::from_secs(5);

/// Only the supervisor reaps this PID, under the same lock used to signal it.
/// WNOWAIT observes lifecycle events without releasing the PID to the OS.
struct ProcessIdentity {
    pid: libc::pid_t,
    live: bool,
}

fn terminate(identity: &Mutex<ProcessIdentity>) {
    if let Ok(identity) = identity.lock()
        && identity.live
    {
        // SAFETY: the supervisor cannot reap or release this PID while locked.
        // Every fixture is a single process. Never signal a numeric group ID.
        unsafe {
            libc::kill(identity.pid, libc::SIGKILL);
        }
    }
}

struct Managed {
    identity: Arc<Mutex<ProcessIdentity>>,
    stopped: mpsc::Receiver<libc::c_int>,
    finished: mpsc::Receiver<std::result::Result<libc::c_int, String>>,
    cancel: Option<mpsc::Sender<()>>,
    timeout: Duration,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    lifetime: Option<UnixStream>,
}

impl Managed {
    fn spawn(command: &mut Command) -> Result<Self> {
        Self::spawn_with_deadline(command, WATCHDOG)
    }

    fn spawn_with_deadline(command: &mut Command, timeout: Duration) -> Result<Self> {
        let mut child = command.spawn()?;
        let pid = child.id() as libc::pid_t;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let identity = Arc::new(Mutex::new(ProcessIdentity { pid, live: true }));
        let (cancel, deadline) = mpsc::channel();
        let watchdog_identity = Arc::clone(&identity);
        std::thread::spawn(move || {
            if matches!(
                deadline.recv_timeout(timeout),
                Err(mpsc::RecvTimeoutError::Timeout)
            ) {
                terminate(&watchdog_identity);
            }
        });
        let (stop_tx, stopped) = mpsc::channel();
        let (exit_tx, finished) = mpsc::channel();
        let supervisor_identity = Arc::clone(&identity);
        std::thread::spawn(move || {
            // Keep the Child owned until this sole waiter reaps it.
            let _child = child;
            loop {
                let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
                // SAFETY: only observe our unreaped child; storage is valid.
                let rc = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid as _,
                        info.as_mut_ptr(),
                        libc::WEXITED | libc::WSTOPPED | libc::WNOWAIT,
                    )
                };
                if rc < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    if let Ok(mut identity) = supervisor_identity.lock() {
                        // ECHILD means no signal may safely use this numeric PID.
                        if error.raw_os_error() == Some(libc::ECHILD) {
                            identity.live = false;
                        }
                    }
                    let _ = exit_tx.send(Err(format!("child observation failed: {error}")));
                    return;
                }
                let Ok(mut identity) = supervisor_identity.lock() else {
                    let _ = exit_tx.send(Err("child identity lock poisoned".into()));
                    return;
                };
                let mut status = 0;
                // SAFETY: consume the observed event while signals are excluded.
                let observed =
                    unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED | libc::WNOHANG) };
                if observed == 0 {
                    continue;
                }
                if observed < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    if error.raw_os_error() == Some(libc::ECHILD) {
                        identity.live = false;
                    }
                    let _ = exit_tx.send(Err(format!("child reap failed: {error}")));
                    return;
                }
                if libc::WIFSTOPPED(status) {
                    let _ = stop_tx.send(libc::WSTOPSIG(status));
                } else if libc::WIFEXITED(status) || libc::WIFSIGNALED(status) {
                    identity.live = false;
                    let _ = exit_tx.send(Ok(status));
                    return;
                }
            }
        });
        Ok(Self {
            identity,
            stopped,
            finished,
            cancel: Some(cancel),
            timeout,
            stdout,
            stderr,
            lifetime: None,
        })
    }

    fn id(&self) -> libc::pid_t {
        self.identity.lock().expect("fixture identity lock").pid
    }

    fn wait_status(&mut self) -> Result<std::process::ExitStatus> {
        let status = match self.finished.recv_timeout(self.timeout) {
            Ok(status) => status.map_err(anyhow::Error::msg)?,
            Err(error) => {
                let cleanup = self.cleanup();
                bail!("fixture deadline expired: {error}; cleanup: {cleanup:?}");
            }
        };
        self.cancel.take();
        Ok(std::process::ExitStatus::from_raw(status))
    }

    fn wait(&mut self) -> Result<()> {
        let status = self.wait_status()?;
        ensure!(status.success(), "fixture exited with {status}");
        Ok(())
    }

    fn stopped(&mut self, expected: libc::c_int) -> Result<()> {
        let actual = self
            .stopped
            .recv_timeout(self.timeout)
            .context("missing child stop handshake")?;
        ensure!(actual == expected, "expected stop {expected}, got {actual}");
        Ok(())
    }

    fn resume(&self) -> Result<()> {
        let identity = self
            .identity
            .lock()
            .map_err(|_| anyhow::anyhow!("child identity lock poisoned"))?;
        ensure!(identity.live, "cannot resume a reaped fixture");
        // SAFETY: a stopped owned PID remains pinned under the reaper lock.
        ensure!(
            unsafe { libc::kill(identity.pid, libc::SIGCONT) } == 0,
            "resume fixture"
        );
        Ok(())
    }

    fn cleanup(&mut self) -> Result<()> {
        self.cancel.take();
        self.lifetime.take();
        terminate(&self.identity);
        if self
            .identity
            .lock()
            .map_err(|_| anyhow::anyhow!("child identity lock poisoned"))?
            .live
        {
            self.finished
                .recv_timeout(CLEANUP_WATCHDOG)
                .context("incomplete cleanup: owned fixture did not exit within watchdog")?
                .map_err(anyhow::Error::msg)
                .context("incomplete cleanup: fixture exit was not confirmed")?;
        }
        Ok(())
    }
}

impl Drop for Managed {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            eprintln!("job-control prototype cleanup failed: {error}");
        }
    }
}

fn command(args: &[&str]) -> Result<Command> {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["terminal-probe", "jobcontrol-child"])
        .args(args);
    Ok(command)
}

pub fn run() -> Result<()> {
    for scenario in ["normal", "parent-loss-running", "parent-loss-stopped"] {
        run_case(scenario)?;
    }
    Ok(())
}

fn run_case(scenario: &str) -> Result<()> {
    let mut master = -1;
    let mut slave = -1;
    let mut name = [0i8; 256];
    // SAFETY: all output pointers reference valid storage; default PTY settings.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            name.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    ensure!(rc == 0, "openpty: {}", std::io::Error::last_os_error());
    // SAFETY: openpty returned two distinct newly owned descriptors.
    let (master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    // SAFETY: openpty writes a NUL-terminated name into the buffer.
    let name = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_str()?;
    // SAFETY: prevent accidental extra master/slave inheritance across re-exec.
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        ensure!(
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
            "set PTY close-on-exec"
        );
    }
    let temp = tempfile::tempdir()?;
    let socket = temp.path().join("control.sock");
    let life_path = temp.path().join("lifetime.sock");
    let life_listener = UnixListener::bind(&life_path)?;
    let mut cmd = command(&[
        "session",
        name,
        socket.to_str().context("socket path UTF-8")?,
        life_path.to_str().context("lifetime path UTF-8")?,
        scenario,
    ])?;
    // The session receives only the synthetic master on stdin, for input injection.
    cmd.stdin(Stdio::from(master.try_clone()?))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Capture streams independently; every receipt is bounded even if a fixture
    // retains a descriptor. No thread join can hold the caller indefinitely.
    let (drain_stop, drain) = drain_terminal(master)?;
    let mut session = Managed::spawn_with_deadline(&mut cmd, Duration::from_secs(60))?;
    drop(cmd);
    let output = capture(session.stdout.take().context("session stdout")?);
    let errors = capture(session.stderr.take().context("session stderr")?);
    let mut lifetime = Some(accept(&life_listener)?);
    if scenario != "normal" {
        expect(lifetime.as_mut().context("parent lifetime channel")?, b'R')?;
        // Explicit loss of the outer coordinator channel, after fixture readiness.
        lifetime.take();
    }
    let result = session.wait_status();
    drop(slave);
    let transcript = match drain.recv_timeout(CLEANUP_WATCHDOG) {
        Ok(result) => result.map_err(anyhow::Error::msg)?,
        Err(error) => {
            drop(drain_stop);
            let cleanup = drain.recv_timeout(CLEANUP_WATCHDOG);
            bail!(
                "incomplete cleanup: terminal retained after session exit: {error}; drain cancellation: {cleanup:?}"
            );
        }
    };
    let report = String::from_utf8(receive_capture(output, "session stdout")?)?;
    let errors = String::from_utf8(receive_capture(errors, "session stderr")?)?;
    if let Err(error) = result {
        bail!("job-control session failed: {error}\n{errors}");
    }
    let status = result?;
    if scenario != "normal" {
        ensure!(
            status.code() == Some(124),
            "parent-loss session exited unexpectedly: {status}"
        );
        println!(
            "PASS: {scenario} closes the worker's slave and all report pipes within cleanup watchdog"
        );
        return Ok(());
    }
    ensure!(
        status.success(),
        "job-control session failed: {status}\n{errors}"
    );
    let text = String::from_utf8(transcript)?;
    ensure!(
        text.contains("fetchingDIRECT-WRITE"),
        "missing direct-write overlap evidence: {text:?}"
    );
    ensure!(
        text.contains("fetchingIGNORED-SIGTTOU"),
        "missing ignored-signal overlap evidence: {text:?}"
    );
    ensure!(
        !text.contains("UNRELATED-WRITE"),
        "TOSTOP let the unrelated default-signal writer through"
    );
    print!("{report}");
    println!(
        "PASS: PTY transcript confirms background writes overlap the frame without TOSTOP and with ignored SIGTTOU"
    );
    println!(
        "VERDICT: reject same-session job control as transparent default; TOSTOP stops unrelated jobs and ignored SIGTTOU bypasses it"
    );
    Ok(())
}

pub fn child(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("session") if args.len() == 5 => session(&args[1], &args[2], &args[3], &args[4]),
        Some("worker") if args.len() == 3 => worker(&args[1], &args[2]),
        _ => bail!("invalid jobcontrol-child arguments"),
    }
}

struct Terminal {
    file: File,
    original: libc::termios,
    foreground: libc::pid_t,
}

impl Terminal {
    fn foreground(&self, group: libc::pid_t) -> Result<()> {
        // SAFETY: only this synthetic session's tty is affected. Ignore SIGTTOU
        // during reclaim because the coordinator is temporarily backgrounded.
        unsafe {
            let previous = libc::signal(libc::SIGTTOU, libc::SIG_IGN);
            let rc = libc::tcsetpgrp(self.file.as_raw_fd(), group);
            let error = std::io::Error::last_os_error();
            libc::signal(libc::SIGTTOU, previous);
            ensure!(rc == 0, "tcsetpgrp: {error}");
        }
        Ok(())
    }

    fn tostop(&self, enabled: bool) -> Result<()> {
        let mut mode = self.original;
        if enabled {
            mode.c_lflag |= libc::TOSTOP;
        } else {
            mode.c_lflag &= !libc::TOSTOP;
        }
        // SAFETY: valid owned synthetic terminal and initialized termios.
        ensure!(
            unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &mode) } == 0,
            "tcsetattr: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }

    fn restore(&self) -> Result<()> {
        self.foreground(self.foreground)?;
        // SAFETY: valid owned synthetic terminal and saved termios.
        ensure!(
            unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.original) } == 0,
            "restore terminal modes"
        );
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

fn session(slave: &str, path: &str, lifetime_path: &str, scenario: &str) -> Result<()> {
    // This re-exec starts without pre_exec hooks or inherited user tty fds.
    // SAFETY: called in a disposable process before spawning any threads.
    ensure!(
        unsafe { libc::setsid() } >= 0,
        "setsid: {}",
        std::io::Error::last_os_error()
    );
    let mut parent_lifetime = UnixStream::connect(lifetime_path)?;
    watch_parent(parent_lifetime.try_clone()?);
    let file = OpenOptions::new().read(true).write(true).open(slave)?;
    // SAFETY: establishes only this new session's synthetic controlling terminal.
    ensure!(
        unsafe { libc::ioctl(file.as_raw_fd(), libc::TIOCSCTTY as _, 0) } == 0,
        "TIOCSCTTY: {}",
        std::io::Error::last_os_error()
    );
    let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: valid output storage; assume_init follows a successful tcgetattr.
    ensure!(
        unsafe { libc::tcgetattr(file.as_raw_fd(), original.as_mut_ptr()) } == 0,
        "tcgetattr"
    );
    let mut tty = Terminal {
        file,
        original: unsafe { original.assume_init() },
        foreground: unsafe { libc::getpgrp() },
    };
    tty.foreground(tty.foreground)?;
    let listener = UnixListener::bind(path)?;
    if scenario != "normal" {
        let mode = if scenario == "parent-loss-stopped" {
            "suspend"
        } else {
            "hold"
        };
        let (mut held, mut control) = launch(mode, path, &listener)?;
        control.write_all(b"G")?;
        if mode == "suspend" {
            held.stopped(libc::SIGTSTP)?;
        } else {
            expect(&mut control, b'H')?;
        }
        parent_lifetime.write_all(b"R")?;
        // The parent-death watcher exits this process. Keep the worker lease live
        // until then, proving EOF cleanup rather than ordinary scope cleanup.
        loop {
            std::thread::park();
        }
    }

    tty.tostop(false)?;
    tty.file.write_all(FRAME)?;
    let (mut writer, mut control) = launch("write", path, &listener)?;
    control.write_all(b"G")?;
    expect(&mut control, b'D')?;
    writer.wait()?;
    println!("PASS: no TOSTOP permits background direct terminal output");

    let (mut reader, mut control) = launch("read", path, &listener)?;
    control.write_all(b"G")?;
    reader.stopped(libc::SIGTTIN)?;
    tty.file.write_all(b"\r\x1b[2K")?;
    tty.foreground(reader.id())?;
    reader.resume()?;
    // stdin is the synthetic PTY master supplied by run(), never user stdin.
    // SAFETY: literal bytes and their length are valid for write.
    ensure!(
        unsafe { libc::write(libc::STDIN_FILENO, b"answer\n".as_ptr().cast(), 7) } == 7,
        "inject synthetic input"
    );
    expect(&mut control, b'D')?;
    reader.wait()?;
    tty.foreground(tty.foreground)?;
    println!(
        "PASS: read-first child stops with SIGTTIN; clear then foreground handoff delivers exact input"
    );

    let (mut suspended, mut control) = launch("suspend", path, &listener)?;
    control.write_all(b"G")?;
    suspended.stopped(libc::SIGTSTP)?;
    tty.foreground(suspended.id())?;
    suspended.resume()?;
    expect(&mut control, b'D')?;
    suspended.wait()?;
    tty.foreground(tty.foreground)?;
    println!("PASS: explicit SIGTSTP stop and SIGCONT resume preserve foreground restoration");

    tty.tostop(true)?;
    let (mut unrelated, mut control) = launch("unrelated", path, &listener)?;
    control.write_all(b"G")?;
    unrelated.stopped(libc::SIGTTOU)?;
    drop(unrelated);
    println!("PASS: TOSTOP also stops an unrelated background writer with SIGTTOU");

    tty.file.write_all(FRAME)?;
    let (mut ignored, mut control) = launch("ignore", path, &listener)?;
    control.write_all(b"G")?;
    expect(&mut control, b'D')?;
    ignored.wait()?;
    println!("PASS: ignored SIGTTOU bypasses TOSTOP");

    tty.restore()?;
    let mut restored = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: valid owned tty and writable termios storage.
    ensure!(
        unsafe { libc::tcgetattr(tty.file.as_raw_fd(), restored.as_mut_ptr()) } == 0,
        "read restored modes"
    );
    let restored = unsafe { restored.assume_init() };
    ensure!(
        restored.c_lflag == tty.original.c_lflag,
        "terminal local flags were not restored"
    );
    ensure!(
        unsafe { libc::tcgetpgrp(tty.file.as_raw_fd()) } == tty.foreground,
        "foreground group not restored"
    );
    println!("PASS: all owned children reaped; synthetic tty flags and foreground group restored");
    Ok(())
}

fn launch(mode: &str, path: &str, listener: &UnixListener) -> Result<(Managed, UnixStream)> {
    let mut cmd = command(&["worker", mode, path])?;
    let (parent_lifetime, child_lifetime) = UnixStream::pair()?;
    cmd.stdin(Stdio::from(OwnedFd::from(child_lifetime)))
        .stdout(Stdio::null())
        .stderr(Stdio::inherit());
    let mut child = Managed::spawn(&mut cmd)?;
    child.lifetime = Some(parent_lifetime);
    drop(cmd);
    let mut stream = accept(listener)?;
    expect(&mut stream, b'R')?;
    Ok((child, stream))
}

fn expect(stream: &mut UnixStream, expected: u8) -> Result<()> {
    let mut value = [0];
    stream.read_exact(&mut value)?;
    ensure!(
        value[0] == expected,
        "control handshake expected {expected}, got {}",
        value[0]
    );
    Ok(())
}

fn worker(mode: &str, path: &str) -> Result<()> {
    // SAFETY: disposable fixture sets its own group and job-control dispositions.
    unsafe {
        ensure!(libc::setpgid(0, 0) == 0, "setpgid");
        libc::signal(libc::SIGTTIN, libc::SIG_DFL);
        libc::signal(libc::SIGTSTP, libc::SIG_DFL);
        libc::signal(
            libc::SIGTTOU,
            if mode == "ignore" {
                libc::SIG_IGN
            } else {
                libc::SIG_DFL
            },
        );
    }
    watch_parent(std::io::stdin());
    let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    let mut control = UnixStream::connect(path)?;
    control.set_read_timeout(Some(WATCHDOG))?;
    control.write_all(b"R")?;
    expect(&mut control, b'G')?;
    match mode {
        "hold" => {
            control.write_all(b"H")?;
            loop {
                std::thread::park();
            }
        }
        "read" => {
            let mut input = [0; 7];
            tty.read_exact(&mut input)?;
            ensure!(
                &input == b"answer\n",
                "reader received incorrect synthetic input"
            );
        }
        "suspend" => {
            // SAFETY: the fixture stops only its own non-orphaned process group.
            ensure!(unsafe { libc::raise(libc::SIGTSTP) } == 0, "self stop");
        }
        "write" => tty.write_all(b"DIRECT-WRITE")?,
        "unrelated" => tty.write_all(b"UNRELATED-WRITE")?,
        "ignore" => tty.write_all(b"IGNORED-SIGTTOU")?,
        _ => bail!("unknown worker mode"),
    }
    control.write_all(b"D")?;
    Ok(())
}

/// Every managed fixture has a distinct lifetime channel. EOF makes even a
/// tty-blocked fixture exit. Stopped fixture groups also receive the kernel's
/// orphan-group SIGHUP/SIGCONT when their session parent exits, exercised above.
fn watch_parent(mut lifetime: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        let mut ignored = [0; 1];
        loop {
            match lifetime.read(&mut ignored) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        // SAFETY: exits only this disposable fixture on coordinator loss. No
        // process IDs are signaled, and OS closure releases descendant leases.
        unsafe {
            libc::_exit(124);
        }
    });
}

fn accept(listener: &UnixListener) -> Result<UnixStream> {
    let mut descriptor = libc::pollfd {
        fd: listener.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid descriptor; timeout only bounds absent startup readiness.
    ensure!(
        unsafe { libc::poll(&mut descriptor, 1, 15_000) } == 1,
        "fixture control startup failed"
    );
    let (stream, _) = listener.accept()?;
    stream.set_read_timeout(Some(WATCHDOG))?;
    stream.set_write_timeout(Some(WATCHDOG))?;
    Ok(stream)
}

type Capture = mpsc::Receiver<std::result::Result<Vec<u8>, String>>;

fn capture(mut reader: impl Read + Send + 'static) -> Capture {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = reader
            .by_ref()
            .take(65536)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())
            .and_then(|_| {
                if bytes.len() < 65536 {
                    Ok(bytes)
                } else {
                    Err("fixture capture exceeded 64 KiB".into())
                }
            });
        let _ = send.send(result);
    });
    receive
}

fn receive_capture(receive: Capture, label: &str) -> Result<Vec<u8>> {
    receive
        .recv_timeout(CLEANUP_WATCHDOG)
        .with_context(|| format!("incomplete cleanup: {label} remained open"))?
        .map_err(anyhow::Error::msg)
}

fn drain_terminal(mut master: File) -> Result<(UnixStream, Capture)> {
    let (stop, wakeup) = UnixStream::pair()?;
    // SAFETY: apply nonblocking mode only to the synthetic master.
    unsafe {
        let flags = libc::fcntl(master.as_raw_fd(), libc::F_GETFL);
        ensure!(
            flags >= 0
                && libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) == 0,
            "set master nonblocking"
        );
    }
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let result = (|| -> Result<Vec<u8>> {
            let mut bytes = Vec::new();
            let mut buffer = [0; 2048];
            loop {
                let mut descriptors = [
                    libc::pollfd {
                        fd: master.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: wakeup.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                // SAFETY: valid descriptor array. Parent cancellation wakes poll
                // through socket EOF, so this thread never needs an unbounded join.
                if unsafe { libc::poll(descriptors.as_mut_ptr(), 2, -1) } < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error.into());
                }
                ensure!(
                    descriptors[1].revents == 0,
                    "terminal drain cancelled before slave closure"
                );
                loop {
                    match master.read(&mut buffer) {
                        Ok(0) => return Ok(bytes),
                        Ok(count) => {
                            bytes.extend_from_slice(&buffer[..count]);
                            ensure!(
                                bytes.len() <= 65536,
                                "terminal fixture output exceeded 64 KiB"
                            );
                        }
                        Err(error) if error.raw_os_error() == Some(libc::EIO) => return Ok(bytes),
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        })()
        .map_err(|error| error.to_string());
        let _ = send.send(result);
    });
    Ok((stop, receive))
}
