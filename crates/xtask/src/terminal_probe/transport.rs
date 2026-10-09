//! Real loopback transports, with an isolated terminal used only to observe native I/O.
//!
//! Prompt matching here drives disposable fixtures. It is deliberately not a production
//! routing policy: a custom helper may read its terminal without emitting any bytes.

use anyhow::{Context, Result, bail, ensure};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant};

const WATCHDOG: Duration = Duration::from_secs(20);

pub fn run() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let home = temp.path();
    config_isolation(home)?;
    final_tty_bytes(home)?;
    checked(
        command("git", home)
            .args(["init", "--bare", "--quiet"])
            .arg(home.join("remote.git")),
    )?;
    let git_dir = home.join("remote.git");
    let tree = String::from_utf8(checked(
        command("git", home)
            .arg("-C")
            .arg(&git_dir)
            .arg("mktree")
            .stdin(Stdio::null()),
    )?)?;
    let commit = String::from_utf8(checked(
        command("git", home)
            .arg("-C")
            .arg(&git_dir)
            .args([
                "-c",
                "user.name=Terminal Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit-tree",
                tree.trim(),
                "-m",
                "transport fixture",
            ])
            .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z"),
    )?)?;
    checked(command("git", home).arg("-C").arg(&git_dir).args([
        "update-ref",
        "refs/heads/main",
        commit.trim(),
    ]))?;
    checked(command("git", home).arg("-C").arg(&git_dir).args([
        "symbolic-ref",
        "HEAD",
        "refs/heads/main",
    ]))?;
    let expected = format!(
        "{}\tHEAD\n{}\trefs/heads/main\n",
        commit.trim(),
        commit.trim()
    )
    .into_bytes();
    let http = Http::start(home)?;
    let url = format!("http://127.0.0.1:{}/remote.git", http.port);
    let mut git = command("git", home);
    git.args(["-c", "credential.helper=", "ls-remote", &url]);
    let native = capture(
        git,
        &[
            (b"Username for", b"fixture-user\n"),
            (b"Password for", b"fixture-password\n"),
        ],
    )?;
    ensure!(
        native.status.success(),
        "native Git HTTP authentication failed: {}",
        String::from_utf8_lossy(&native.stderr)
    );
    ensure!(
        native.answers == 2,
        "Git did not emit both native prompts on its private terminal"
    );
    ensure!(
        native.stdout == expected,
        "Git machine stdout differs from exact remote refs"
    );
    ensure!(
        http.authorized.load(Ordering::Acquire),
        "HTTP endpoint never received fixture credentials"
    );
    println!(
        "PASS Git HTTP: real 401 challenge, native username/password on controlling tty, exact ref stdout, successful ls-remote"
    );
    println!(
        "OBSERVED Git HTTP: native prompts reached tty={}, captured stderr={}",
        contains(&native.tty, b"Username for"),
        contains(&native.stderr, b"Username for")
    );

    let mut git = command("git", home);
    git.env("GIT_TERMINAL_PROMPT", "0")
        .args(["-c", "credential.helper=", "ls-remote", &url]);
    let noninteractive = capture(git, &[])?;
    ensure!(
        !noninteractive.status.success(),
        "GIT_TERMINAL_PROMPT=0 unexpectedly succeeded"
    );
    ensure!(
        contains(&noninteractive.stderr, b"terminal prompts disabled"),
        "Git no-prompt selection changed"
    );
    println!("PASS Git HTTP: GIT_TERMINAL_PROMPT=0 retains failure without terminal interaction");
    let mut authenticated = command("git", home);
    authenticated.env("GIT_TERMINAL_PROMPT", "0").args([
        "-c",
        "credential.helper=",
        "-c",
        "http.extraHeader=Authorization: Basic Zml4dHVyZS11c2VyOmZpeHR1cmUtcGFzc3dvcmQ=",
        "ls-remote",
        &url,
    ]);
    let authenticated = capture(authenticated, &[])?;
    ensure!(
        authenticated.status.success() && authenticated.stdout == expected,
        "private PTY recorder failed authenticated Git HTTP exact refs"
    );
    println!(
        "PASS private PTY recorder: real authenticated Git HTTP with no-prompt selection, exact ref stdout"
    );
    http.finish()?;
    openssh(home)
}

fn command(program: &str, home: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.current_dir(home)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CEILING_DIRECTORIES", home.parent().unwrap_or(home))
        .env("LC_ALL", "C");
    cmd
}

fn checked(cmd: &mut Command) -> Result<Vec<u8>> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut process = Process::new(
        cmd.spawn()
            .with_context(|| format!("start {:?}", cmd.get_program()))?,
    );
    let stdout = process
        .child
        .stdout
        .take()
        .context("missing helper stdout")?;
    let stderr = process
        .child
        .stderr
        .take()
        .context("missing helper stderr")?;
    let stdout = Worker::spawn(move || drain(stdout));
    let stderr = Worker::spawn(move || drain(stderr));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = process.try_wait()? {
            break status;
        }
        ensure!(
            start.elapsed() < WATCHDOG,
            "helper completion watchdog expired"
        );
        let _ = poll(-1, 10)?;
    };
    let stdout = stdout.finish("helper stdout")?;
    let stderr = stderr.finish("helper stderr")?;
    ensure!(
        status.success(),
        "{:?}: {}",
        cmd.get_program(),
        String::from_utf8_lossy(&stderr)
    );
    Ok(stdout)
}

fn config_isolation(home: &Path) -> Result<()> {
    let poison = home.join("poison");
    let nested = poison.join("nested");
    checked(command("git", home).args(["init", "--quiet"]).arg(&poison))?;
    checked(command("git", home).arg("-C").arg(&poison).args([
        "config",
        "url.file:///deliberately-invalid-fixture/.insteadOf",
        "https://fixture.invalid/",
    ]))?;
    fs::create_dir(&nested)?;
    let expected = b"https://fixture.invalid/repo.git\n";
    let mut unguarded = command("git", &nested);
    unguarded.env_remove("GIT_CEILING_DIRECTORIES").args([
        "ls-remote",
        "--get-url",
        "https://fixture.invalid/repo.git",
    ]);
    ensure!(
        checked(&mut unguarded)? != expected,
        "poison fixture did not demonstrate inherited URL rewriting"
    );
    let isolated = checked(command("git", &nested).args([
        "ls-remote",
        "--get-url",
        "https://fixture.invalid/repo.git",
    ]))?;
    ensure!(
        isolated == expected,
        "ceiling boundary did not block parent Git configuration"
    );
    ensure!(
        command("git", home).get_current_dir() == Some(home),
        "helper must run outside source checkout"
    );
    println!(
        "PASS config isolation: poisoned parent URL rewrite affects control but cannot affect isolated transport command"
    );
    Ok(())
}

fn final_tty_bytes(home: &Path) -> Result<()> {
    let marker = b"final-tty-marker\n";
    let mut tee = command("tee", home);
    tee.arg("/dev/tty");
    let result = capture_input(tee, &[], Some(marker))?;
    ensure!(
        result.status.success()
            && result.stdout == marker
            && contains(&result.tty, marker.strip_suffix(b"\n").unwrap()),
        "fast final terminal write lost"
    );
    println!(
        "PASS terminal tail: fast native final marker preserved through child completion and terminal EOF"
    );
    // Exercise the final-drain routine independently with bytes queued before
    // its first read. The writer closes its only slave reference, so success
    // requires consuming the tail and observing EOF/EIO, without timing races.
    let (mut master, mut slave) = (-1, -1);
    ensure!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0,
        "tail fixture openpty failed"
    );
    let mut master = unsafe { File::from_raw_fd(master) };
    let mut slave = unsafe { File::from_raw_fd(slave) };
    slave.write_all(marker)?;
    let close = Worker::spawn(move || {
        drop(slave);
        Ok(())
    });
    let mut tail = Vec::new();
    drain_tty(&mut master, &mut tail)?;
    close.finish("tail writer close")?;
    ensure!(
        contains(&tail, b"final-tty-marker"),
        "queued terminal tail was lost"
    );
    println!(
        "PASS terminal tail regression: prequeued bytes consumed by final-drain path through EOF/EIO"
    );
    Ok(())
}

struct Process {
    child: Child,
    reaped: bool,
}
impl Process {
    fn new(child: Child) -> Self {
        Self {
            child,
            reaped: false,
        }
    }
    fn try_wait(&mut self) -> Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        self.reaped |= status.is_some();
        Ok(status)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        // A reaped PID/process-group number can be reused. Only an unreaped
        // direct child pins this identity strongly enough for cancellation.
        if self.reaped {
            return;
        }
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let started = Instant::now();
        while started.elapsed() < WATCHDOG {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => return,
                Ok(None) => {
                    let _ = poll(-1, 10);
                }
            }
        }
    }
}

struct Worker<T>(mpsc::Receiver<Result<T>>);
impl<T: Send + 'static> Worker<T> {
    fn spawn(work: impl FnOnce() -> Result<T> + Send + 'static) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || {
            let _ = tx.send(work());
        });
        Self(rx)
    }
    fn finish(self, label: &str) -> Result<T> {
        self.0
            .recv_timeout(WATCHDOG)
            .with_context(|| format!("{label} completion watchdog expired or worker exited"))?
    }
}

struct Capture {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    tty: Vec<u8>,
    answers: usize,
}

fn capture(cmd: Command, answers: &[(&[u8], &[u8])]) -> Result<Capture> {
    capture_input(cmd, answers, None)
}

fn capture_input(
    cmd: Command,
    answers: &[(&[u8], &[u8])],
    input: Option<&[u8]>,
) -> Result<Capture> {
    capture_exchange(cmd, answers, input, false)
}

fn capture_exchange(
    mut cmd: Command,
    answers: &[(&[u8], &[u8])],
    input: Option<&[u8]>,
    sftp_reply: bool,
) -> Result<Capture> {
    let (mut master_fd, mut slave_fd) = (-1, -1);
    ensure!(
        unsafe {
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } == 0,
        "openpty: {}",
        std::io::Error::last_os_error()
    );
    let mut master = unsafe { File::from_raw_fd(master_fd) };
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    let mut name = [0_i8; 1024];
    ensure!(
        unsafe { libc::ttyname_r(slave_fd, name.as_mut_ptr(), name.len()) } == 0,
        "ttyname_r failed"
    );
    let path = unsafe { std::ffi::CStr::from_ptr(name.as_ptr()) }.to_owned();
    // Null stdin is intentional: Git and SSH native authentication must use their
    // controlling terminal, independently of machine stdout/stderr capture.
    cmd.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    unsafe {
        cmd.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let tty = libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY);
            if tty < 0 || libc::ioctl(tty, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::fcntl(tty, libc::F_SETFD, libc::FD_CLOEXEC);
            libc::close(master_fd);
            libc::close(slave_fd);
            Ok(())
        });
    }
    let mut process = Process::new(cmd.spawn()?);
    let mut child_input = process.child.stdin.take();
    if let Some(input) = input {
        ensure!(
            input.len() <= 512,
            "fixture stdin exceeds small bounded protocol packet"
        );
        child_input
            .as_mut()
            .context("missing stdin")?
            .write_all(input)?;
    }
    if !sftp_reply {
        drop(child_input.take());
    }
    let stdout = process.child.stdout.take().context("missing stdout")?;
    let stderr = process.child.stderr.take().context("missing stderr")?;
    let out_thread = Worker::spawn(move || drain_with_stdin(stdout, child_input));
    let err_thread = Worker::spawn(move || drain(stderr));
    let started = Instant::now();
    let mut tty = Vec::new();
    let mut answered = 0;
    let status = loop {
        ensure!(
            started.elapsed() < WATCHDOG,
            "transport terminal handshake exceeded watchdog"
        );
        if let Some(status) = process.try_wait()? {
            break status;
        }
        if poll(master.as_raw_fd(), 100)? {
            let mut buf = [0; 4096];
            match master.read(&mut buf) {
                Ok(0) => (),
                Ok(n) => {
                    tty.extend_from_slice(&buf[..n]);
                    ensure!(
                        tty.len() <= 65_536,
                        "unexpected excessive native terminal output"
                    );
                }
                Err(e) if e.raw_os_error() == Some(libc::EIO) => (),
                Err(e) => return Err(e.into()),
            }
            if let Some((prompt, answer)) = answers.get(answered)
                && contains(&tty, prompt)
            {
                master.write_all(answer)?;
                answered += 1;
            }
        }
    };
    // Closing the parent's last slave reference permits a real EOF/EIO. A child
    // may exit before the final tty write becomes visible to the polling loop.
    drop(slave);
    drain_tty(&mut master, &mut tty)?;
    drop(process);
    Ok(Capture {
        status,
        stdout: out_thread.finish("stdout capture")?,
        stderr: err_thread.finish("stderr capture")?,
        tty,
        answers: answered,
    })
}

fn drain(reader: impl Read + AsRawFd) -> Result<Vec<u8>> {
    drain_with_stdin(reader, None)
}

fn drain_with_stdin(
    mut reader: impl Read + AsRawFd,
    mut input_until_reply: Option<ChildStdin>,
) -> Result<Vec<u8>> {
    let started = Instant::now();
    let mut bytes = Vec::new();
    loop {
        ensure!(
            started.elapsed() < WATCHDOG,
            "pipe drain watchdog expired (possible inherited descriptor)"
        );
        if !poll(reader.as_raw_fd(), 100)? {
            continue;
        }
        let mut buf = [0; 4096];
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buf[..n]);
        ensure!(
            bytes.len() <= 1_048_576,
            "fixture pipe capture exceeded bound"
        );
        if input_until_reply.is_some() && bytes.len() >= 4 {
            let packet = u32::from_be_bytes(bytes[..4].try_into()?) as usize;
            ensure!(
                packet <= 1_048_572,
                "SFTP reply packet exceeded capture bound"
            );
            if bytes.len() >= packet + 4 {
                drop(input_until_reply.take());
            }
        }
    }
}

fn drain_tty(master: &mut File, bytes: &mut Vec<u8>) -> Result<()> {
    let started = Instant::now();
    loop {
        ensure!(
            started.elapsed() < WATCHDOG,
            "terminal tail watchdog expired (possible inherited descriptor)"
        );
        if !poll(master.as_raw_fd(), 100)? {
            continue;
        }
        let mut buf = [0; 4096];
        match master.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                bytes.extend_from_slice(&buf[..n]);
                ensure!(
                    bytes.len() <= 65_536,
                    "native terminal output exceeded bound"
                );
            }
            Err(error) if error.raw_os_error() == Some(libc::EIO) => return Ok(()),
            Err(error) => return Err(error.into()),
        }
    }
}

fn contains(bytes: &[u8], pattern: &[u8]) -> bool {
    bytes.windows(pattern.len()).any(|w| w == pattern)
}

fn poll(fd: i32, milliseconds: i32) -> Result<bool> {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut pfd, 1, milliseconds) };
    if result < 0 {
        let err = std::io::Error::last_os_error();
        if err.kind() == std::io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(err.into());
    }
    Ok(result > 0)
}

struct Http {
    port: u16,
    stop: Arc<AtomicBool>,
    authorized: Arc<AtomicBool>,
    worker: Option<Worker<()>>,
}

impl Http {
    fn start(home: &Path) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let authorized = Arc::new(AtomicBool::new(false));
        let stop_copy = stop.clone();
        let auth_copy = authorized.clone();
        let home = home.to_owned();
        let worker = Worker::spawn(move || {
            while !stop_copy.load(Ordering::Acquire) {
                let (mut stream, _) = listener.accept()?;
                if stop_copy.load(Ordering::Acquire) {
                    break;
                }
                stream.set_read_timeout(Some(WATCHDOG))?;
                stream.set_write_timeout(Some(WATCHDOG))?;
                let request_start = Instant::now();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    ensure!(
                        request_start.elapsed() < WATCHDOG,
                        "HTTP header watchdog expired"
                    );
                    let mut b = [0];
                    stream.read_exact(&mut b)?;
                    header.push(b[0]);
                    ensure!(header.len() <= 16_384, "HTTP request header too large");
                }
                let header = String::from_utf8(header)?;
                // This value is only the public fixture username/password above.
                let valid = header.lines().any(|l| {
                    l.eq_ignore_ascii_case(
                        "Authorization: Basic Zml4dHVyZS11c2VyOmZpeHR1cmUtcGFzc3dvcmQ=",
                    )
                });
                if !valid {
                    stream.write_all(b"HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"terminal-prototype\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")?;
                    continue;
                }
                auth_copy.store(true, Ordering::Release);
                let request = header.lines().next().context("empty request")?;
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .context("missing HTTP path")?;
                let (path, query) = path.split_once('?').unwrap_or((path, ""));
                ensure!(
                    path == "/remote.git/info/refs" || path == "/remote.git/HEAD",
                    "unexpected HTTP path {path}"
                );
                let output = checked(
                    command("git", &home)
                        .arg("http-backend")
                        .env("GIT_PROJECT_ROOT", &home)
                        .env("GIT_HTTP_EXPORT_ALL", "1")
                        .env("PATH_INFO", path)
                        .env("QUERY_STRING", query)
                        .env("REQUEST_METHOD", "GET")
                        .env("REMOTE_USER", "fixture-user")
                        .stdin(Stdio::null()),
                )?;
                let split = output
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .context("CGI header missing")?;
                let headers = &output[..split];
                let body = &output[split + 4..];
                stream.write_all(b"HTTP/1.1 200 OK\r\n")?;
                stream.write_all(headers)?;
                write!(
                    stream,
                    "\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )?;
                stream.write_all(body)?;
            }
            Ok(())
        });
        Ok(Self {
            port,
            stop,
            authorized,
            worker: Some(worker),
        })
    }
    fn finish(mut self) -> Result<()> {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        self.worker
            .take()
            .context("missing HTTP worker")?
            .finish("HTTP server")
    }
}
impl Drop for Http {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn openssh(home: &Path) -> Result<()> {
    let host_key = home.join("host-key");
    let user_key = home.join("user-key");
    let protected_key = home.join("user-key-protected");
    for key in [&host_key, &user_key] {
        checked(
            command("ssh-keygen", home)
                .args(["-q", "-t", "ed25519", "-N", "", "-f"])
                .arg(key),
        )?;
    }
    checked(
        command("ssh-keygen", home)
            .args(["-q", "-t", "ed25519", "-N", "fixture-only", "-f"])
            .arg(&protected_key),
    )?;
    let authorized_keys = home.join("authorized-keys");
    let mut public_keys = fs::read(home.join("user-key.pub"))?;
    public_keys.extend(fs::read(home.join("user-key-protected.pub"))?);
    fs::write(&authorized_keys, public_keys)?;
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    let config = home.join("sshd-config");
    fs::write(
        &config,
        format!(
            "ListenAddress 127.0.0.1\nPort {port}\nHostKey {}\nAuthorizedKeysFile {}\nPidFile {}\nStrictModes no\nUsePAM no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitUserRC no\nPermitUserEnvironment no\nDisableForwarding yes\nPermitTTY no\nSubsystem sftp internal-sftp\nForceCommand internal-sftp -d {}\nLogLevel VERBOSE\n",
            host_key.display(),
            authorized_keys.display(),
            home.join("sshd.pid").display(),
            home.display()
        ),
    )?;
    let daemon_path = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|directory| directory.join("sshd"))
        .find(|path| path.is_file());
    let Some(daemon_path) = daemon_path else {
        println!("INCOMPLETE OpenSSH endpoint: sshd not found on PATH");
        return Ok(());
    };
    if !system_rc_absent(&daemon_path, &config, home)? {
        println!(
            "INCOMPLETE OpenSSH endpoint: cannot establish absence of system sshrc; refusing authenticated sessions"
        );
        return Ok(());
    }
    let mut daemon_cmd = command(daemon_path.to_str().context("non-UTF8 sshd path")?, home);
    daemon_cmd
        .args(["-D", "-e", "-f"])
        .arg(&config)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    unsafe {
        daemon_cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut daemon = match daemon_cmd.spawn() {
        Ok(child) => Process::new(child),
        Err(error) => {
            println!("INCOMPLETE OpenSSH endpoint: could not start unprivileged sshd: {error}");
            return Ok(());
        }
    };
    let mut stderr = daemon.child.stderr.take().context("missing sshd log")?;
    let mut log = Vec::new();
    let start = Instant::now();
    loop {
        if contains(&log, b"Server listening on") {
            break;
        }
        if let Some(status) = daemon.try_wait()? {
            log.extend(Worker::spawn(move || drain(stderr)).finish("failed sshd logs")?);
            println!(
                "INCOMPLETE OpenSSH endpoint: unprivileged sshd exited {status}: {}",
                String::from_utf8_lossy(&log).trim()
            );
            return Ok(());
        }
        if start.elapsed() >= WATCHDOG {
            bail!("sshd readiness handshake exceeded watchdog");
        }
        if poll(stderr.as_raw_fd(), 100)? {
            let mut buf = [0; 4096];
            let n = stderr.read(&mut buf)?;
            log.extend_from_slice(&buf[..n]);
        }
    }
    let logs = Worker::spawn(move || drain(stderr));
    let username = String::from_utf8(checked(command("id", home).arg("-un"))?)?;
    let build_client = |batch: bool, key: &Path| {
        let mut client = command("ssh", home);
        client.args([
            "-o",
            if batch {
                "BatchMode=yes"
            } else {
                "BatchMode=no"
            },
        ]);
        client
            .args([
                "-F",
                "/dev/null",
                "-o",
                "IdentityAgent=none",
                "-o",
                "IdentitiesOnly=yes",
                "-o",
                "GlobalKnownHostsFile=/dev/null",
                "-o",
                "StrictHostKeyChecking=ask",
                "-o",
                "PreferredAuthentications=publickey",
                "-o",
            ])
            .arg(format!(
                "UserKnownHostsFile={}",
                home.join("known-hosts").display()
            ))
            .arg("-i")
            .arg(key)
            .arg("-p")
            .arg(port.to_string())
            .arg("-l")
            .arg(username.trim())
            .arg("-s")
            .arg("127.0.0.1")
            .arg("sftp");
        client
    };
    let batch_config = home.join("ssh-batch-config");
    fs::write(&batch_config, "Host *\n    BatchMode yes\n")?;
    let mut batch = command("ssh", home);
    batch
        .args(["-F"])
        .arg(&batch_config)
        .args([
            "-o",
            "IdentityAgent=none",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
            "-o",
            "UserKnownHostsFile=/dev/null",
            "-o",
            "StrictHostKeyChecking=ask",
            "-p",
        ])
        .arg(port.to_string())
        .arg("-s")
        .arg("127.0.0.1")
        .arg("sftp");
    let batch = capture(batch, &[])?;
    ensure!(
        !batch.status.success() && batch.tty.is_empty(),
        "OpenSSH BatchMode changed native prompting"
    );
    println!("PASS OpenSSH: BatchMode=yes fails unknown-host verification without prompting");
    // SSH_FXP_INIT version 3. This invokes sshd's internal subsystem, never
    // the real account's shell or startup files, and EOF closes the session.
    let init = [0, 0, 0, 5, 1, 0, 0, 0, 3];
    let native = capture_exchange(
        build_client(false, &user_key),
        &[(b"Are you sure you want to continue connecting", b"yes\n")],
        Some(&init),
        true,
    )?;
    ensure!(
        native.answers == 1,
        "OpenSSH host trust prompt did not reach private tty"
    );
    println!(
        "OBSERVED OpenSSH: native host trust prompt reached tty={}, captured stderr={}",
        contains(&native.tty, b"Are you sure"),
        contains(&native.stderr, b"Are you sure")
    );
    if native.status.success() {
        ensure!(
            native.stdout.len() >= 9
                && native.stdout[4..9] == [2, 0, 0, 0, 3]
                && u32::from_be_bytes(native.stdout[..4].try_into()?) as usize
                    == native.stdout.len() - 4,
            "OpenSSH internal SFTP version response is malformed: {} bytes {:?}",
            native.stdout.len(),
            native.stdout.get(..9)
        );
        println!(
            "PASS OpenSSH: real endpoint, host trust prompt on controlling tty, public-key authentication, binary internal-SFTP handshake"
        );
        let capture = capture_exchange(build_client(true, &user_key), &[], Some(&init), true)?;
        ensure!(
            capture.status.success() && capture.stdout == native.stdout,
            "private PTY recorder failed trusted-host BatchMode SSH"
        );
        println!(
            "PASS private PTY recorder: real authenticated OpenSSH with trusted host and BatchMode=yes, identical binary SFTP response"
        );
        let protected = capture_exchange(
            build_client(false, &protected_key),
            &[(b"Enter passphrase for key", b"fixture-only\n")],
            Some(&init),
            true,
        )?;
        ensure!(
            protected.status.success()
                && protected.answers == 1
                && protected.stdout == native.stdout,
            "OpenSSH encrypted fixture key failed native passphrase/internal-SFTP exchange"
        );
        ensure!(
            contains(&protected.tty, b"Enter passphrase for key")
                && !contains(&protected.stderr, b"Enter passphrase for key"),
            "OpenSSH encrypted key prompt did not exclusively use private terminal"
        );
        println!(
            "PASS OpenSSH: encrypted disposable key native passphrase on tty, no user agent/keychain, identical binary SFTP response"
        );
    } else {
        println!(
            "INCOMPLETE OpenSSH authenticated command: native prompt worked but session failed: {}",
            String::from_utf8_lossy(&native.stderr).trim()
        );
    }
    drop(daemon);
    logs.finish("sshd logs")?;
    Ok(())
}

// PermitUserRC does not disable system sshrc. Inspect the stock OpenSSH
// session executable selected by this exact fixture configuration, and fail
// closed when its compiled path cannot be established. This is a test-host
// precondition, not a sandbox for arbitrary executables.
fn system_rc_absent(daemon: &Path, config: &Path, home: &Path) -> Result<bool> {
    let settings = checked(
        command(daemon.to_str().context("non-UTF8 sshd path")?, home)
            .args(["-T", "-f"])
            .arg(config),
    )?;
    let settings = String::from_utf8(settings)?;
    let session = settings
        .lines()
        .find_map(|line| {
            line.split_once(' ')
                .filter(|(key, _)| key.eq_ignore_ascii_case("SshdSessionPath"))
                .map(|(_, value)| value)
        })
        .map(Path::new)
        .unwrap_or(daemon);
    let executable = fs::read(session)?;
    let absent = compiled_rc_absent(&executable);
    if absent {
        println!(
            "PASS OpenSSH preflight: configured session executable's compiled system sshrc path is absent"
        );
    }
    Ok(absent)
}

fn compiled_rc_absent(executable: &[u8]) -> bool {
    let paths: std::collections::BTreeSet<_> = executable
        .split(|byte| *byte == 0)
        .filter_map(|bytes| std::str::from_utf8(bytes).ok())
        .filter(|text| {
            text.starts_with('/') && text.ends_with("/sshrc") && !text.contains(char::is_whitespace)
        })
        .collect();
    if paths.is_empty() {
        return false;
    }
    for path in paths {
        match Path::new(path).try_exists() {
            Ok(false) => (),
            Ok(true) | Err(_) => return false,
        }
    }
    true
}

#[test]
fn system_rc_preflight_refuses_existing_or_unidentified_startup_files() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("sshrc");
    let literal = format!("{}\0", path.display());
    assert!(compiled_rc_absent(literal.as_bytes()));
    fs::write(&path, b"must not run").unwrap();
    for candidate in [
        literal.as_bytes(),
        b"no identified rc",
        b"/bin/sh /fake/sshrc\0",
    ] {
        assert!(!compiled_rc_absent(candidate));
    }
}
