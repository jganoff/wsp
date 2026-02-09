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

    let app = cli::build_cli();
    let matches = app.get_matches();
    let json = matches.get_flag("json");

    let paths = match config::Paths::resolve() {
        Ok(p) => p,
        Err(err) => {
            render_error(err, json);
            process::exit(1);
        }
    };

    match cli::dispatch(&matches, &paths) {
        Ok(out) => {
            if let Err(err) = output::render(out, json) {
                render_error(err, json);
                process::exit(1);
            }
        }
        Err(err) => {
            if interrupted.load(Ordering::SeqCst) {
                process::exit(130);
            }
            render_error(err, json);
            process::exit(1);
        }
    }
}

fn render_error(err: anyhow::Error, json: bool) {
    if json {
        let _ = serde_json::to_string_pretty(&output::ErrorOutput {
            error: err.to_string(),
        })
        .map(|s| println!("{}", s));
    } else {
        eprintln!("Error: {}", err);
    }
}
