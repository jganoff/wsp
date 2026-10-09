//! A recording of the actual renderer with controlled, live terminal children.
//! Delays here pace the visual demo only. Correctness probes use handshakes.
use super::{display, pty};
use anyhow::{Result, ensure};
use std::io::Write;
use std::sync::mpsc;
use std::time::Duration;
use wsp_core::progress::{self, Progress};

pub fn run() -> Result<()> {
    println!("Terminal prototype: compact rows + native authentication");
    println!("Two live fixture children; public demo answers only.\n");
    let session = display::Session::start(false);
    let mut alpha = pty::Probe::fixture_named("alpha")?;
    let mut bravo = pty::Probe::fixture_named("bravo")?;
    let (ready, started) = mpsc::channel();
    let (alpha_done, alpha_wait) = mpsc::channel();
    let (bravo_done, bravo_wait) = mpsc::channel();
    std::thread::scope(move |scope| -> Result<()> {
        for (name, done) in [("alpha", alpha_wait), ("bravo", bravo_wait)] {
            let ready = ready.clone();
            scope.spawn(move || {
                let operation = Progress::start(format!("Fetching github.com/demo/{name}"));
                operation.reporter().measured(
                    format!("Fetching github.com/demo/{name}"),
                    format!("github.com/demo/{name}"),
                    "Connecting".into(),
                    String::new(),
                );
                let _ = ready.send(());
                let _ = done.recv();
            });
        }
        started.recv()?;
        started.recv()?;
        // Presentation pacing only: both children remain at an acknowledged
        // silent barrier throughout repeated renderer ticks.
        std::thread::sleep(Duration::from_secs(5));
        alpha.begin()?;
        bravo.begin()?;
        alpha.wait_prompt()?;
        bravo.wait_prompt()?;
        {
            // This demo exercises the existing display gate. The feasibility
            // probes own child lifetime independently; this is not a production
            // coordinator or an inference that any quiet pipe is safe to grant.
            let _display_handoff = progress::suspend();
            eprintln!("alpha authentication");
            alpha.relay_prompt(&mut std::io::stderr())?;
            std::io::stderr().flush()?;
            std::thread::sleep(Duration::from_millis(900));
            alpha.answer()?;
            let first = alpha.finish()?;
            ensure!(first.status.success(), "alpha fixture failed");
            ensure!(
                bravo.prompt_pending()? && bravo.still_running()?,
                "queued bravo prompt disappeared or consumed another child's answer"
            );
            alpha_done.send(())?;
            eprintln!("[demo answer]\nAuthenticated alpha.\n");
            eprintln!("bravo authentication");
            bravo.relay_prompt(&mut std::io::stderr())?;
            std::io::stderr().flush()?;
            std::thread::sleep(Duration::from_millis(900));
            bravo.answer()?;
            let second = bravo.finish()?;
            ensure!(second.status.success(), "bravo fixture failed");
            bravo_done.send(())?;
            eprintln!("[demo answer]\nAuthenticated bravo.\n");
        }
        Ok(())
    })?;
    {
        let _saving = Progress::start("Saving workspace");
        std::thread::sleep(Duration::from_millis(1400));
    }
    drop(session);
    println!("Done. Two prompts served in order; terminal display restored.");
    Ok(())
}

pub fn observer_failure() -> Result<()> {
    struct FailedDisplay;
    impl progress::Observer for FailedDisplay {
        fn observe(&self, _: progress::Event) -> bool {
            false
        }
    }
    let _observer = progress::install(std::sync::Arc::new(FailedDisplay));
    let mut child = pty::Probe::fixture()?;
    let _operation = Progress::start("Synthetic failed display");
    child.begin()?;
    child.wait_prompt()?;
    child.relay_prompt(&mut std::io::sink())?;
    child.answer()?;
    ensure!(
        child.finish()?.status.success(),
        "display failure prevented child completion"
    );
    println!("PASS optional observer failure: native input and child cleanup remain independent");
    Ok(())
}
