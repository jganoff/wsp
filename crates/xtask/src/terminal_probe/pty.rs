//! Disposable private-controlling-terminal experiment. No product runner uses it.
//!
//! All interaction is synthetic and confined to a new PTY. In particular, this
//! adapter must never be selected merely because a remote uses SSH or HTTPS.
use anyhow::{Context, Result, bail, ensure};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

const WATCHDOG: Duration = Duration::from_secs(20);
const BINARY_LEN: usize = 262_144;

fn syscall(value: libc::c_int, what: &str) -> Result<libc::c_int> {
    if value < 0 {
        Err(std::io::Error::last_os_error()).with_context(|| what.to_owned())
    } else {
        Ok(value)
    }
}

struct Pty {
    master: File,
    // Keep the slave open until the child acknowledges acquiring it.
    slave: Option<File>,
    path: String,
}

impl Pty {
    fn new() -> Result<Self> {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: openpty initializes these descriptors; null optional pointers
        // request defaults. Ownership is transferred once to File below.
        unsafe {
            syscall(
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                ),
                "open private PTY",
            )?;
        }
        // SAFETY: successful openpty returned owned, valid descriptors.
        let master = unsafe { File::from_raw_fd(master) };
        // SAFETY: this is the other descriptor from the successful openpty.
        let slave = unsafe { File::from_raw_fd(slave) };
        let mut name = [0_i8; 1024];
        // SAFETY: name is a writable buffer of the stated length.
        let result = unsafe { libc::ttyname_r(slave.as_raw_fd(), name.as_mut_ptr(), name.len()) };
        ensure!(result == 0, "ttyname_r failed: {result}");
        // SAFETY: successful ttyname_r writes a NUL-terminated name.
        let path = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }
            .to_str()?
            .to_owned();
        for file in [&master, &slave] {
            // SAFETY: descriptors are live; close-on-exec prevents accidental
            // inheritance of either private terminal endpoint by the bootstrap.
            unsafe {
                syscall(
                    libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC),
                    "close-on-exec",
                )?;
            }
        }
        // Only the synthetic terminal is changed. Keep canonical line input,
        // disable echo and output translations for deterministic prompt bytes.
        let mut attrs = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr initializes attrs on success.
        unsafe {
            syscall(
                libc::tcgetattr(slave.as_raw_fd(), attrs.as_mut_ptr()),
                "read private tty modes",
            )?;
            let mut attrs = attrs.assume_init();
            attrs.c_lflag &= !(libc::ECHO | libc::ECHONL);
            attrs.c_oflag &= !libc::OPOST;
            syscall(
                libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &attrs),
                "set private tty modes",
            )?;
        }
        Ok(Self {
            master,
            slave: Some(slave),
            path,
        })
    }

    fn readable(&self, timeout: i32) -> Result<bool> {
        let mut descriptor = libc::pollfd {
            fd: self.master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: descriptor is a valid pollfd for one live descriptor.
        syscall(
            unsafe { libc::poll(&mut descriptor, 1, timeout) },
            "poll private tty",
        )?;
        Ok(descriptor.revents & (libc::POLLIN | libc::POLLHUP) != 0)
    }

    fn receive(&mut self, bytes: &[u8]) -> Result<Vec<u8>> {
        let mut received = vec![0; bytes.len()];
        for byte in &mut received {
            ensure!(
                self.readable(WATCHDOG.as_millis() as i32)?,
                "private tty output watchdog expired"
            );
            self.master.read_exact(std::slice::from_mut(byte))?;
        }
        ensure!(received == bytes, "private tty bytes differ");
        Ok(received)
    }

    fn resize(&self, rows: u16, columns: u16) -> Result<()> {
        let size = libc::winsize {
            ws_row: rows,
            ws_col: columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: a live terminal descriptor and valid winsize pointer.
        syscall(
            unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &size) },
            "resize private tty",
        )?;
        Ok(())
    }
}

fn drain(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = pipe
            .read_to_end(&mut bytes)
            .map(|_| bytes)
            .map_err(Into::into);
        let _ = sender.send(result);
    });
    receiver
}

pub struct Probe {
    child: Option<Child>,
    reaped: bool,
    tty: Pty,
    control: UnixStream,
    stdout: Option<mpsc::Receiver<Result<Vec<u8>>>>,
    stderr: Option<mpsc::Receiver<Result<Vec<u8>>>>,
    _directory: tempfile::TempDir,
    answer: Vec<u8>,
}

pub struct Capture {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Probe {
    /// Synthetic output-first prompt, paused at a deterministic start barrier.
    pub fn fixture() -> Result<Self> {
        Self::fixture_named("fixture")
    }
    pub fn fixture_named(name: &str) -> Result<Self> {
        ensure!(
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
            "fixture name must be ASCII alphanumeric"
        );
        Self::spawn_with_name("output-first", "null", name)
    }
    pub fn begin(&mut self) -> Result<()> {
        self.send(b'G')
    }
    pub fn prompt_pending(&self) -> Result<bool> {
        self.tty.readable(0)
    }
    pub fn wait_prompt(&self) -> Result<()> {
        ensure!(
            self.tty.readable(WATCHDOG.as_millis() as i32)?,
            "prompt watchdog expired"
        );
        Ok(())
    }
    /// Call only after acquiring display/input ownership. Fixture text is public.
    pub fn prompt(&mut self) -> Result<()> {
        self.tty.receive(b"Synthetic token: ").map(|_| ())
    }
    pub fn relay_prompt(&mut self, writer: &mut impl Write) -> Result<()> {
        let bytes = self.tty.receive(b"Synthetic token: ")?;
        writer.write_all(&bytes)?;
        writer.flush()?;
        Ok(())
    }
    pub fn answer(&mut self) -> Result<()> {
        self.tty.master.write_all(&self.answer)?;
        self.expect(b'A')
    }
    pub fn still_running(&mut self) -> Result<bool> {
        let status = self
            .child
            .as_mut()
            .context("child handle already consumed")?
            .try_wait()?;
        // try_wait reaps too. No signal may use this numeric process/group ID
        // after the kernel releases it, even if a pipe is still open elsewhere.
        self.reaped |= status.is_some();
        Ok(status.is_none())
    }
    fn spawn(case: &str, stdin: &str) -> Result<Self> {
        Self::spawn_with_name(case, stdin, "fixture")
    }
    fn spawn_with_name(case: &str, stdin: &str, name: &str) -> Result<Self> {
        let directory = tempfile::tempdir()?;
        let socket = directory.path().join("control");
        let listener = UnixListener::bind(&socket)?;
        let tty = Pty::new()?;
        let mut bootstrap = Command::new(std::env::current_exe()?);
        bootstrap.args(["terminal-probe", "pty-child", case, stdin, &tty.path]);
        bootstrap.arg(&socket);
        bootstrap.arg(name);
        bootstrap.stdin(if stdin == "data" {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        bootstrap.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = bootstrap.spawn()?;
        let stdout = drain(child.stdout.take().context("stdout pipe")?);
        let stderr = drain(child.stderr.take().context("stderr pipe")?);
        let mut ready = libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // A real-time watchdog bounds startup failure; no assertion depends on
        // scheduling speed or passage of time. Success is a socket handshake.
        // SAFETY: valid single-element poll array.
        let polled = unsafe { libc::poll(&mut ready, 1, WATCHDOG.as_millis() as i32) };
        if polled <= 0 {
            let _ = child.kill();
            let _ = child.wait();
            bail!("private tty bootstrap did not connect");
        }
        let (control, _) = listener.accept()?;
        control.set_read_timeout(Some(WATCHDOG))?;
        control.set_write_timeout(Some(WATCHDOG))?;
        let mut probe = Self {
            child: Some(child),
            reaped: false,
            tty,
            control,
            stdout: Some(stdout),
            stderr: Some(stderr),
            _directory: directory,
            answer: format!("{name}-only\n").into_bytes(),
        };
        probe.expect(b'R')?;
        probe.tty.slave.take();
        if stdin == "data" {
            let mut input = probe
                .child
                .as_mut()
                .unwrap()
                .stdin
                .take()
                .context("data stdin")?;
            input.write_all(&[0, 255, b'\n', b'\r', 17])?;
        }
        Ok(probe)
    }

    fn expect(&mut self, expected: u8) -> Result<()> {
        let mut byte = [0];
        self.control.read_exact(&mut byte)?;
        ensure!(
            byte[0] == expected,
            "control handshake differs: expected {expected}, got {}",
            byte[0]
        );
        Ok(())
    }

    fn send(&mut self, value: u8) -> Result<()> {
        self.control.write_all(&[value]).map_err(Into::into)
    }

    fn wait_for_exit(&mut self) -> Result<ExitStatus> {
        let deadline = Instant::now() + WATCHDOG;
        loop {
            if let Some(status) = self
                .child
                .as_mut()
                .context("child handle already consumed")?
                .try_wait()?
            {
                self.reaped = true;
                return Ok(status);
            }
            // This is exclusively a hang watchdog, not a concurrency assertion
            // or prompt-completion heuristic. The sole thread that reaps the
            // child also decides timeout; no background watchdog can race a
            // successful reap and signal a reused process-group identifier.
            if Instant::now() >= deadline {
                bail!("child exit watchdog expired");
            }
            thread::park_timeout(Duration::from_millis(5));
        }
    }

    pub fn finish(mut self) -> Result<Capture> {
        let status = self.wait_for_exit()?;
        // Receive timeouts bound inherited-pipe failures. An escaped descendant
        // can retain a pipe despite group cancellation, so never join its pipe
        // drainer without a bound. This fixture adapter cannot supervise an
        // arbitrary escaped process and is not a production runner.
        let stdout = self
            .stdout
            .take()
            .unwrap()
            .recv_timeout(WATCHDOG)
            .context("stdout EOF watchdog")??;
        let stderr = self
            .stderr
            .take()
            .unwrap()
            .recv_timeout(WATCHDOG)
            .context("stderr EOF watchdog")??;
        self.child.take();
        Ok(Capture {
            status,
            stdout,
            stderr,
        })
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if self.reaped {
            // The numeric PID/group may already belong to an unrelated job.
            // Fixture descendants are reaped by their parent before it exits;
            // arbitrary surviving descendants require stronger supervision.
            return;
        }
        if let Some(child) = &mut self.child {
            // Only this isolated child's process group can be signalled. The
            // process group exists only after setsid. The direct child kill below
            // also covers a failed or missing bootstrap acknowledgement.
            // SAFETY: kill targets the private process group, not the user's.
            unsafe {
                libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn binary_bytes(reverse: bool) -> Vec<u8> {
    (0..BINARY_LEN)
        .map(|index| {
            if reverse {
                255 - (index % 256) as u8
            } else {
                (index % 256) as u8
            }
        })
        .collect()
}

pub fn run() -> Result<()> {
    let mut alpha = Probe::fixture_named("alpha")?;
    let mut bravo = Probe::fixture_named("bravo")?;
    alpha.begin()?;
    bravo.begin()?;
    alpha.wait_prompt()?;
    bravo.wait_prompt()?;
    alpha.prompt()?;
    alpha.answer()?;
    ensure!(alpha.finish()?.status.success(), "alpha fixture failed");
    ensure!(
        bravo.still_running()? && bravo.prompt_pending()?,
        "alpha consumed bravo's prompt or input"
    );
    bravo.prompt()?;
    bravo.answer()?;
    ensure!(bravo.finish()?.status.success(), "bravo fixture failed");
    println!(
        "PASS private-pty competing prompts: distinct answers stay with their assigned live child"
    );
    for (case, input) in [
        ("binary", "null"),
        ("stdin", "null"),
        ("stdin", "data"),
        ("stdin", "terminal"),
        ("output-first", "null"),
        ("stderr-first", "null"),
        ("read-first", "null"),
        ("resize", "null"),
        ("cancel", "null"),
        ("group-cancel", "null"),
        ("siblings", "null"),
        ("eof", "null"),
        ("no-ctty", "null"),
    ] {
        let mut probe = Probe::spawn(case, input)?;
        probe.send(b'G')?;
        match case {
            "output-first" => {
                // The coordinator can acquire exclusive display ownership
                // before these private tty bytes reach the user's terminal.
                probe.tty.receive(b"Synthetic token: ")?;
                probe.tty.master.write_all(b"fixture-only\n")?;
                probe.expect(b'A')?;
            }
            "stderr-first" | "read-first" => {
                probe.expect(b'B')?;
                ensure!(!probe.tty.readable(0)?, "unexpected terminal output");
                // Explicit fixture knowledge supplies input. A general runner
                // cannot infer this reader from the empty private tty stream.
                probe.tty.master.write_all(b"fixture-only\n")?;
                probe.expect(b'A')?;
            }
            "stdin" if input == "terminal" => {
                probe.expect(b'B')?;
                probe.tty.master.write_all(b"fixture-only\n")?;
            }
            "eof" => {
                probe.expect(b'B')?;
                probe.tty.master.write_all(&[4])?;
                probe.expect(b'A')?;
            }
            "resize" => {
                probe.tty.resize(37, 101)?;
                probe.send(b'S')?;
                probe.expect(b'A')?;
            }
            "siblings" => {
                probe.expect(b'B')?;
                ensure!(
                    !probe.tty.readable(0)?,
                    "sibling readers unexpectedly announced themselves"
                );
                probe.tty.master.write_all(b"A\nB\n")?;
                probe.expect(b'A')?;
            }
            "group-cancel" => {
                probe.expect(b'B')?;
                // Both group members block SIGTERM then consume it with
                // sigwait, allowing the parent fixture to prove and reap both.
                syscall(
                    unsafe {
                        libc::kill(
                            -(probe.child.as_ref().unwrap().id() as libc::pid_t),
                            libc::SIGTERM,
                        )
                    },
                    "signal private parent and descendant",
                )?;
                probe.expect(b'A')?;
            }
            "cancel" => {
                probe.expect(b'B')?;
                // SAFETY: the known session leader also identifies its group.
                syscall(
                    unsafe {
                        libc::kill(
                            -(probe.child.as_ref().unwrap().id() as libc::pid_t),
                            libc::SIGTERM,
                        )
                    },
                    "cancel private group",
                )?;
            }
            _ => {}
        }
        let capture = probe.finish()?;
        if case == "cancel" {
            use std::os::unix::process::ExitStatusExt;
            ensure!(
                capture.status.signal() == Some(libc::SIGTERM),
                "cancellation status differs"
            );
        } else {
            ensure!(
                capture.status.success(),
                "{case}/{input} failed: {}",
                String::from_utf8_lossy(&capture.stderr)
            );
        }
        if case == "binary" {
            ensure!(
                capture.stdout == binary_bytes(false),
                "binary stdout was transformed"
            );
            ensure!(
                capture.stderr == binary_bytes(true),
                "binary stderr was transformed"
            );
        }
        println!("PASS private-pty {case}/{input}");
    }
    println!(
        "LIMIT private-pty: stderr prompts and read-first helpers require fallback; same-child competing readers are not distinguishable"
    );
    Ok(())
}

pub fn child(args: &[String]) -> Result<()> {
    if args.first().map(String::as_str) == Some("group-descendant") {
        let mut control = UnixStream::connect(args.get(1).context("descendant socket")?)?;
        control.write_all(b"R")?;
        wait_for_termination()?;
        return Ok(());
    }
    ensure!(
        args.len() >= 4,
        "pty-child needs case, stdin mode, slave and socket"
    );
    let (case, input, slave_path, socket) = (&args[0], &args[1], &args[2], &args[3]);
    // This runs after reexec in a single-threaded disposable bootstrap, not in
    // a pre_exec closure in the threaded product process.
    // SAFETY: setsid detaches only this child from its inherited session.
    syscall(unsafe { libc::setsid() }, "create private session")?;
    let tty = if case == "no-ctty" {
        ensure!(
            OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .is_err(),
            "setsid retained controlling tty"
        );
        None
    } else {
        let tty = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY)
            .open(slave_path)?;
        // SAFETY: new session has no controlling tty; zero forbids stealing one.
        syscall(
            unsafe { libc::ioctl(tty.as_raw_fd(), libc::TIOCSCTTY as _, 0) },
            "acquire private controlling tty",
        )?;
        if input == "terminal" {
            // SAFETY: duplicates the private descriptor onto this child's stdin.
            syscall(
                unsafe { libc::dup2(tty.as_raw_fd(), libc::STDIN_FILENO) },
                "map terminal stdin",
            )?;
        }
        Some(tty)
    };
    let mut control = UnixStream::connect(socket)?;
    control.set_read_timeout(Some(WATCHDOG))?;
    control.write_all(b"R")?;
    let mut go = [0];
    control.read_exact(&mut go)?;
    ensure!(go == *b"G", "missing go handshake");
    match case.as_str() {
        "binary" => {
            let out = thread::spawn(|| std::io::stdout().write_all(&binary_bytes(false)));
            std::io::stderr().write_all(&binary_bytes(true))?;
            out.join()
                .map_err(|_| anyhow::anyhow!("binary writer panicked"))??;
        }
        "stdin" => {
            let mut bytes = Vec::new();
            if input == "terminal" {
                control.write_all(b"B")?;
                bytes.resize(13, 0);
                std::io::stdin().read_exact(&mut bytes)?;
            } else {
                std::io::stdin().read_to_end(&mut bytes)?;
            }
            let expected: &[u8] = match input.as_str() {
                "data" => &[0, 255, b'\n', b'\r', 17],
                "terminal" => b"fixture-only\n",
                _ => &[],
            };
            ensure!(bytes == expected, "stdin mapping differs");
        }
        "output-first" | "stderr-first" | "read-first" => {
            let mut tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
            if case == "output-first" {
                tty.write_all(b"Synthetic token: ")?;
            }
            if case == "stderr-first" {
                std::io::stderr().write_all(b"Synthetic token: ")?;
            }
            if case != "output-first" {
                control.write_all(b"B")?;
            }
            let expected = format!(
                "{}-only\n",
                args.get(4).map(String::as_str).unwrap_or("fixture")
            );
            let mut bytes = vec![0; expected.len()];
            tty.read_exact(&mut bytes)?;
            ensure!(bytes == expected.as_bytes(), "private input differs");
            control.write_all(b"A")?;
        }
        "eof" => {
            let mut tty = OpenOptions::new().read(true).open("/dev/tty")?;
            control.write_all(b"B")?;
            let mut byte = [0];
            ensure!(
                tty.read(&mut byte)? == 0,
                "canonical EOF did not reach child"
            );
            control.write_all(b"A")?;
        }
        "resize" => {
            control.read_exact(&mut go)?;
            ensure!(go == *b"S", "missing resize handshake");
            let mut size = std::mem::MaybeUninit::<libc::winsize>::uninit();
            // SAFETY: live tty and a valid writable winsize buffer.
            unsafe {
                syscall(
                    libc::ioctl(
                        tty.as_ref().unwrap().as_raw_fd(),
                        libc::TIOCGWINSZ,
                        size.as_mut_ptr(),
                    ),
                    "get private size",
                )?;
                let size = size.assume_init();
                ensure!(
                    size.ws_row == 37 && size.ws_col == 101,
                    "private resize differs"
                );
            }
            control.write_all(b"A")?;
        }
        "siblings" => {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
            let mut readers = Vec::new();
            for _ in 0..2 {
                let barrier = barrier.clone();
                readers.push(thread::spawn(move || -> Result<[u8; 2]> {
                    let mut tty = OpenOptions::new().read(true).open("/dev/tty")?;
                    barrier.wait();
                    let mut bytes = [0; 2];
                    tty.read_exact(&mut bytes)?;
                    Ok(bytes)
                }));
            }
            barrier.wait();
            control.write_all(b"B")?;
            let mut results = Vec::new();
            for reader in readers {
                results.push(
                    reader
                        .join()
                        .map_err(|_| anyhow::anyhow!("sibling reader panicked"))??,
                );
            }
            results.sort();
            ensure!(results == [*b"A\n", *b"B\n"], "sibling readers lost input");
            control.write_all(b"A")?;
        }
        "group-cancel" => {
            block_termination()?;
            let directory = tempfile::tempdir()?;
            let socket = directory.path().join("descendant");
            let listener = UnixListener::bind(&socket)?;
            let mut descendant = Command::new(std::env::current_exe()?)
                .args(["terminal-probe", "pty-child", "group-descendant"])
                .arg(&socket)
                .stdin(Stdio::null())
                .spawn()?;
            let (mut channel, _) = listener.accept()?;
            channel.set_read_timeout(Some(WATCHDOG))?;
            channel.read_exact(&mut go)?;
            ensure!(go == *b"R", "descendant readiness differs");
            control.write_all(b"B")?;
            wait_for_termination()?;
            ensure!(
                descendant.wait()?.success(),
                "descendant did not observe group signal"
            );
            control.write_all(b"A")?;
        }
        "cancel" => {
            control.write_all(b"B")?;
            control.read_exact(&mut go)?;
            bail!("cancel fixture unexpectedly continued");
        }
        "no-ctty" => {}
        _ => bail!("unknown private-pty fixture: {case}"),
    }
    Ok(())
}

fn termination_set() -> libc::sigset_t {
    // SAFETY: sigemptyset initializes the set and sigaddset takes a valid signal.
    unsafe {
        let mut set = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        set
    }
}

fn block_termination() -> Result<()> {
    let set = termination_set();
    // SAFETY: called in the single-threaded fixture, before its child inherits
    // this mask. Only the disposable fixture's SIGTERM delivery changes.
    let error = unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) };
    ensure!(error == 0, "could not block fixture SIGTERM: {error}");
    Ok(())
}

fn wait_for_termination() -> Result<()> {
    let set = termination_set();
    let mut signal = 0;
    // SAFETY: SIGTERM was blocked before fork/exec and the pointers are valid.
    let error = unsafe { libc::sigwait(&set, &mut signal) };
    ensure!(
        error == 0 && signal == libc::SIGTERM,
        "group signal was not observed"
    );
    Ok(())
}
