#![deny(unsafe_code)]

mod cli;
mod hints;
mod output;
mod pr;
mod shellcd;
mod shellnav;
mod usage;

use std::process;

use clap_complete::CompleteEnv;

fn main() {
    exit_quietly_on_closed_output();
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
            if let Err(err) = output::render(out, json) {
                // Tables reach stdout through `io::Write`, so a reader that left
                // surfaces here as an error rather than as the panic the hook
                // catches. Same situation, so same quiet exit.
                if is_closed_pipe(&err) {
                    process::exit(0);
                }
                render_error(err, json);
                process::exit(1);
            }
            // Load config once for gc and hints
            let cfg = wsp_core::config::Config::load_from(&paths.config_path).unwrap_or_default();
            // Opportunistic gc, modelled on `git gc --auto`: no daemon, runs at
            // most once per hour, piggybacking on commands the user already ran.
            //
            // Gated to workspace-mutating commands, which is what git does too --
            // it triggers auto-gc from commit/merge/rebase/fetch, never from
            // status/log/diff. Without the gate a read-only `wsp ls` could
            // permanently delete recoverable workspaces, which is both surprising
            // and against "no silent mutations hiding inside read commands".
            //
            // Nothing is lost by gating: gc entries are only created by `wsp rm`
            // (the sole caller of workspace::remove), which is in the set. The
            // worst case is an expired entry lingering until the next mutation --
            // the opposite of data loss, and `wsp doctor` reports it.
            //
            // `recover` belongs here because it always restores: the listing
            // that made bare `wsp recover` read-only moved to
            // `wsp ls --removed`. While both forms shared one name the gate
            // could not tell them apart -- it sees only the command name -- so
            // including `recover` would have reintroduced the bug this closes.
            if matches!(command.as_str(), "new" | "rm" | "rename" | "recover") {
                wsp_core::gc::maybe_run(&paths, cfg.retention_days());
            }
            // Contextual hints (git-style advice.*) -- only on success
            if !json && code == 0 {
                // One-time upgrade notice (version-gated, independent of cooldown).
                maybe_print_upgrade_notice(&paths, &cfg, &command);
                let hints = hints::evaluate(&command, &cfg, &paths);
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

/// Exit quietly, instead of panicking, when whoever was reading our output
/// stops — `wsp exec ... | head -1`, or a `| grep -q` that found its match.
///
/// Rust ignores SIGPIPE, so writing to a closed pipe returns EPIPE and the
/// `print!` family panics. That turned an ordinary shell idiom into a panic dump
/// and exit 101. Only commands that write in bursts separated by slow work can
/// hit it — `exec`, `fetch` and `sync` print per repo with a git subprocess in
/// between — because anything that writes once fits in the pipe buffer and never
/// notices the reader has gone.
///
/// Restoring `SIG_DFL` for SIGPIPE is the usual fix, but it needs `unsafe` plus a
/// `libc` dependency and does nothing on Windows. A panic hook covers every
/// `println!` in the binary, present and future, with neither.
///
/// Exit 0, not 141: the reader chose to stop, so nothing actually failed. It also
/// keeps `set -o pipefail` from turning every `wsp ... | grep -q` into a failure,
/// which is the shape most scripts use — `scripts/smoke.sh` included.
fn exit_quietly_on_closed_output() {
    let next = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if is_closed_pipe_panic(info) {
            process::exit(0);
        }
        next(info);
    }));
}

/// Did this panic come from `print!` failing because nobody is reading?
///
/// std formats these as `failed printing to stdout: <io error>` and panics with
/// that string, so the payload is all there is to match on. Deliberately narrow
/// on both halves: the label pins it to a stdio write, and the marker pins it to
/// a closed pipe rather than to any write failure at all. Exiting 0 on a full
/// disk would report truncated output as success.
///
/// This matches a message std owns, so `tests/broken_pipe.rs` asserts the
/// behaviour against a real binary rather than trusting this to keep matching.
fn is_closed_pipe_panic(info: &std::panic::PanicHookInfo<'_>) -> bool {
    let payload = info.payload();
    let msg = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    msg.starts_with("failed printing to") && CLOSED_PIPE_MARKERS.iter().any(|m| msg.contains(m))
}

/// How each platform spells "nobody is reading this pipe any more".
///
/// Matched on the numeric code first, because that is the half std does not
/// localize: it renders os errors as `<strerror> (os error <code>)`, and
/// glibc's `strerror` is translated when `LC_MESSAGES` is set. Matching only
/// the text would quietly stop working on a non-English Linux box and let the
/// panic dump back out.
///
/// Windows has two codes, depending on which end noticed first:
/// `ERROR_BROKEN_PIPE` (109) and `ERROR_NO_DATA` (232). std maps both to
/// `ErrorKind::BrokenPipe`, but the panic message carries the raw code.
#[cfg(windows)]
const CLOSED_PIPE_MARKERS: &[&str] = &["os error 109", "os error 232"];
/// EPIPE is 32 on Linux, macOS and the BSDs. The English text is kept as a
/// second chance in case a platform reports a different code.
#[cfg(not(windows))]
const CLOSED_PIPE_MARKERS: &[&str] = &["os error 32", "Broken pipe", "broken pipe"];

/// Is this error a write that failed because the reader is gone?
fn is_closed_pipe(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
    })
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
