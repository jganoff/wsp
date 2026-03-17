#![deny(unsafe_code)]

mod cli;
mod output;

use std::process;

use clap_complete::CompleteEnv;

fn main() {
    CompleteEnv::with_factory(cli::build_cli).complete();

    let _ = ctrlc::set_handler(move || {
        // Exit immediately on Ctrl-C. ctrlc runs handlers in a normal thread
        // context (sigwait-based), so process::exit is safe here. Child processes
        // (e.g. git clone during exec) receive SIGINT independently from the
        // terminal and terminate on their own.
        process::exit(130);
    });

    let mut app = cli::build_cli();
    let matches = app.get_matches_mut();
    let json = matches.get_flag("json");

    // Handle `wsp help [topic]` before general dispatch — it needs
    // the Command definition to print subcommand help.
    if let Some(("help", m)) = matches.subcommand() {
        match cli::help::run(m, &mut app, json) {
            Ok(_) => process::exit(0),
            Err(err) => {
                render_error(err, json);
                process::exit(1);
            }
        }
    }

    let paths = match wsp_core::config::Paths::resolve() {
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
            // Opportunistic gc — runs at most once per hour
            let retention = wsp_core::config::Config::load_from(&paths.config_path)
                .ok()
                .and_then(|c| c.gc_retention_days);
            wsp_core::gc::maybe_run(&paths, retention);
            if code != 0 {
                process::exit(code);
            }
        }
        Err(err) => {
            render_error(err, json);
            process::exit(1);
        }
    }
}

fn render_error(err: anyhow::Error, json: bool) {
    if json {
        match serde_json::to_string_pretty(&wsp_core::output::ErrorOutput {
            error: err.to_string(),
        }) {
            Ok(s) => println!("{}", s),
            Err(_) => eprintln!("Error: {}", err),
        }
    } else {
        eprintln!("Error: {}", err);
    }
}
