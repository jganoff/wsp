mod cli;
mod config;
mod git;
mod giturl;
mod group;
mod mirror;
mod output;
mod workspace;

use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap_complete::CompleteEnv;

fn main() {
    CompleteEnv::with_factory(cli::build_cli).complete();

    let interrupted = Arc::new(AtomicBool::new(false));
    let i = interrupted.clone();
    let _ = ctrlc::set_handler(move || {
        i.store(true, Ordering::SeqCst);
    });

    if let Err(err) = cli::run() {
        if interrupted.load(Ordering::SeqCst) {
            process::exit(130);
        }
        eprintln!("Error: {}", err);
        process::exit(1);
    }
}
