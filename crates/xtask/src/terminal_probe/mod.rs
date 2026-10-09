//! Disposable terminal-ownership experiments. Never linked into wsp.

use anyhow::{Result, bail};

#[allow(dead_code)]
#[path = "../../../wsp/src/progress.rs"]
mod display;

mod demo;
mod isolation;
mod jobcontrol;
mod pty;
mod screen;
mod transport;

pub fn run(args: &[String]) -> Result<()> {
    let (mode, rest) = args
        .split_first()
        .map_or(("all", &[][..]), |(m, r)| (m.as_str(), r));
    match mode {
        "all" => {
            isolation::run()?;
            pty::run()?;
            jobcontrol::run()?;
            demo::observer_failure()?;
            transport::run()
        }
        "isolation" => isolation::run(),
        "isolation-host" => isolation::host(rest),
        "isolation-child" => isolation::child(rest),
        "pty" => pty::run(),
        "jobcontrol" => jobcontrol::run(),
        "transport" => transport::run(),
        "demo" => demo::run(),
        "observer" => demo::observer_failure(),
        "screen" => screen::check(
            rest.first()
                .ok_or_else(|| anyhow::anyhow!("screen requires a raw recording path"))?,
        ),
        "pty-child" => pty::child(rest),
        "jobcontrol-child" => jobcontrol::child(rest),
        _ => bail!("unknown terminal prototype mode: {mode}"),
    }
}
