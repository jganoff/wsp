#![deny(unsafe_code)]

mod cli;
mod hints;
mod output;
mod pr;
mod shellcd;
mod shellnav;

use std::process;

use clap_complete::CompleteEnv;

fn main() {
    init_platform();
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
            // Captured before render consumes the output; the reservedName hint
            // needs to know which workspace was just created.
            let workspace = match &out {
                wsp_core::output::Output::Mutation(m) => m.workspace.clone(),
                _ => None,
            };
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
                // One-time upgrade notice (version-gated, independent of cooldown).
                maybe_print_upgrade_notice(&paths, &cfg, &command);
                let hints = hints::evaluate(&command, workspace.as_deref(), &cfg, &paths);
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

/// Prints a one-time upgrade notice when the installed version changes.
///
/// Reads `~/.local/share/wsp/last-version` and compares it to the current binary
/// version. On mismatch, prints a hint pointing to `wsp whatsnew`, then writes
/// the current version to the file. Silent on any I/O error.
///
/// Skipped when:
/// - `--json` is set (caller already guards this)
/// - running `wsp whatsnew` itself (no circular prompt)
/// - `advice.whatsnew = false` in config
fn maybe_print_upgrade_notice(
    paths: &wsp_core::config::Paths,
    cfg: &wsp_core::config::Config,
    command: &str,
) {
    if command == "whatsnew" {
        return;
    }
    if !cfg
        .advice
        .as_ref()
        .and_then(|m| m.get("whatsnew"))
        .copied()
        .unwrap_or(true)
    {
        return;
    }
    let current = env!("CARGO_PKG_VERSION");
    let version_file = paths.data_dir().join("last-version");
    let last = std::fs::read_to_string(&version_file).unwrap_or_default();
    let last = last.trim();
    if last != current {
        if !last.is_empty() {
            eprintln!(
                "hint: wsp upgraded from v{} to v{}. Run `wsp whatsnew` to see what changed.",
                last, current
            );
            eprintln!("      (suppress: wsp config set advice.whatsnew false)");
        }
        let _ = std::fs::write(&version_file, current);
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

// Set the Windows console output code page to UTF-8 so that stderr newlines
// (0x0A) render correctly in PowerShell, which otherwise decodes them as the
// CP437 ◙ character. This matches the approach used by Python and Node.js on
// Windows. Only the OUTPUT code page is set; the input code page is left alone
// to avoid interfering with interactive stdin prompts.
#[cfg(windows)]
#[allow(unsafe_code)]
fn init_platform() {
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
    }
}

#[cfg(not(windows))]
fn init_platform() {}
