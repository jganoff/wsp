#![deny(unsafe_code)]

mod cli;
mod hints;
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

    // Resolve effective command path before consuming matches.
    // Goes up to three levels for nested subcommands (e.g. repo/setup-commands/add).
    // For `setup-commands add`, explicit scope flags are encoded as a suffix
    // (e.g. /registry, /workspace, /repo) so hints can avoid false positives.
    let command = match matches.subcommand() {
        Some(("repo", sub)) => match sub.subcommand() {
            Some((name, sub2)) => match sub2.subcommand() {
                Some((leaf, leaf_m)) => {
                    let scope_tag = if name == "setup-commands" && leaf == "add" {
                        if leaf_m.get_flag("registry") {
                            "/registry"
                        } else if leaf_m.get_flag("workspace") {
                            "/workspace"
                        } else if leaf_m.get_flag("repo-scope") {
                            "/repo"
                        } else {
                            ""
                        }
                    } else {
                        ""
                    };
                    format!("repo/{}/{}{}", name, leaf, scope_tag)
                }
                None => format!("repo/{}", name),
            },
            None => "repo".to_string(),
        },
        Some((name, _)) => name.to_string(),
        None => String::new(),
    };

    match cli::dispatch(&matches, &paths) {
        Ok(out) => {
            let code = output::exit_code(&out);
            if let Err(err) = output::render(out, json) {
                render_error(err, json);
                process::exit(1);
            }
            // Load config once for gc and hints
            let cfg = wsp_core::config::Config::load_from(&paths.config_path).unwrap_or_default();
            // Opportunistic gc -- runs at most once per hour
            wsp_core::gc::maybe_run(&paths, cfg.gc_retention_days);
            // Contextual hints (git-style advice.*) -- only on success
            if !json && code == 0 {
                let hints = hints::evaluate(&command, &cfg);
                if !hints.is_empty() {
                    eprintln!();
                }
                for hint in hints {
                    eprintln!("{}", hint);
                }
            }
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
