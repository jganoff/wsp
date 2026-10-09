//! Prove terminal detachment with simultaneous children before adopting it in wsp.
//!
//! This is a process-behavior experiment, not a Git configuration classifier.
use anyhow::{Context, Result, bail, ensure};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const WATCHDOG: Duration = Duration::from_secs(20);

pub fn run() -> Result<()> {
    let home = tempfile::tempdir()?;
    for detached in [false, true] {
        let mut host = super::transport::command(
            std::env::current_exe()?
                .to_str()
                .context("non-UTF8 xtask path")?,
            home.path(),
        );
        host.args([
            "terminal-probe",
            "isolation-host",
            if detached { "detached" } else { "native" },
        ]);
        let result = super::transport::capture(host, &[])?;
        ensure!(
            result.status.success(),
            "isolation host failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let terminal = String::from_utf8_lossy(&result.tty);
        for name in ["alpha", "bravo"] {
            ensure!(
                terminal.contains(&format!("tty-{name}")) != detached,
                "{name}: detachment={detached}, terminal bytes={terminal:?}"
            );
        }
        ensure!(
            String::from_utf8_lossy(&result.stdout).contains("both children held concurrently"),
            "missing concurrency handshake"
        );
        println!(
            "PASS {}: two simultaneous children, captured carriage-return output, {}",
            if detached {
                "detached session"
            } else {
                "negative control"
            },
            if detached {
                "neither child can open /dev/tty"
            } else {
                "both children write to the same controlling terminal"
            }
        );
    }
    Ok(())
}

pub fn host(args: &[String]) -> Result<()> {
    let detached = args.first().is_some_and(|arg| arg == "detached");
    ensure!(
        OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .is_ok(),
        "host must start with a controlling terminal"
    );
    let mut children = Vec::new();
    let (tx, rx) = mpsc::channel();
    for name in ["alpha", "bravo"] {
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.args(["terminal-probe", "isolation-child", name])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if detached {
            // SAFETY: setsid is async-signal-safe and touches only this child's
            // session. No allocation, locking, or terminal I/O occurs here.
            unsafe {
                cmd.pre_exec(|| {
                    if libc::setsid() < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = OwnedChild(cmd.spawn()?);
        let stdout = child.0.stdout.take().context("child stdout")?;
        let stderr = child.0.stderr.take().context("child stderr")?;
        let send = tx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<String> {
                let mut ready = String::new();
                BufReader::new(stdout).read_line(&mut ready)?;
                Ok(ready)
            })();
            let _ = send.send(result);
        });
        let (err_tx, err_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = BufReader::new(stderr)
                .read_to_end(&mut bytes)
                .map(|_| bytes);
            let _ = err_tx.send(result);
        });
        children.push((name, child, err_rx));
    }
    drop(tx);
    // Readiness is published only after attempting terminal I/O. Children cannot
    // exit until the parent releases stdin. Time bounds a missing handshake,
    // never determines whether children overlap.
    let mut reports = Vec::new();
    for _ in 0..children.len() {
        reports.push(
            rx.recv_timeout(WATCHDOG)
                .context("ready handshake watchdog")??,
        );
    }
    for (name, child, _) in &mut children {
        ensure!(
            child.0.try_wait()?.is_none(),
            "{name} exited before release"
        );
        let expected = format!("{name} tty={}\n", !detached);
        ensure!(
            reports.contains(&expected),
            "unexpected child evidence: {reports:?}"
        );
    }
    println!("both children held concurrently");
    for (_, child, _) in &mut children {
        child
            .0
            .stdin
            .take()
            .context("child input")?
            .write_all(b"release\n")?;
    }
    for (name, child, errors) in &mut children {
        let bytes = errors
            .recv_timeout(WATCHDOG)
            .context("stderr EOF watchdog")??;
        ensure!(
            bytes == format!("\r\x1b[2Kstderr-{name} 42%\r\x1b[2Kstderr-{name} done\n").as_bytes(),
            "captured progress differs for {name}: {bytes:?}"
        );
        ensure!(child.0.wait()?.success(), "{name} failed");
    }
    Ok(())
}

pub fn child(args: &[String]) -> Result<()> {
    let name = args.first().context("child name")?;
    let terminal = OpenOptions::new().read(true).write(true).open("/dev/tty");
    let available = terminal.is_ok();
    if let Ok(mut terminal) = terminal {
        write!(terminal, "\r\x1b[2Ktty-{name}\n")?;
        terminal.flush()?;
    }
    eprint!("\r\x1b[2Kstderr-{name} 42%");
    println!("{name} tty={available}");
    std::io::stdout().flush()?;
    let mut release = String::new();
    std::io::stdin().read_line(&mut release)?;
    if release != "release\n" {
        bail!("parent did not release child");
    }
    eprint!("\r\x1b[2Kstderr-{name} done\n");
    Ok(())
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
