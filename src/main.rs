#![deny(unsafe_code)]

mod cli;
mod config;
mod git;
mod giturl;
mod group;
mod lang;
mod mirror;
mod output;
mod workspace;

#[cfg(test)]
mod testutil;

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
            let code = output::exit_code(&out);
            if let Err(err) = output::render(out, json) {
                render_error(err, json);
                process::exit(1);
            }
            if code != 0 {
                process::exit(code);
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
        match serde_json::to_string_pretty(&output::ErrorOutput {
            error: err.to_string(),
        }) {
            Ok(s) => println!("{}", s),
            Err(_) => eprintln!("Error: {}", err),
        }
    } else {
        eprintln!("Error: {}", err);
    }
}
