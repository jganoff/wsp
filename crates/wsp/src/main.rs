#![deny(unsafe_code)]

#[cfg(all(feature = "test-crash-barriers", not(wsp_crash_test)))]
compile_error!("test-crash-barriers requires --cfg wsp_crash_test; run `just crash-test`");
#[cfg(all(wsp_crash_test, not(feature = "test-crash-barriers")))]
compile_error!("--cfg wsp_crash_test requires the test-crash-barriers feature");
#[cfg(all(feature = "test-crash-barriers", wsp_crash_test, not(debug_assertions)))]
compile_error!("test-crash-barriers may only be compiled with debug assertions");

mod cli;
mod context;
mod git_access;
mod hints;
mod output;
mod pager;
mod pr;
mod progress;
mod shellcd;
mod shellnav;
mod transport;
mod usage;

use std::io::IsTerminal;
use std::process;

use clap_complete::CompleteEnv;

fn main() {
    let raw_args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if raw_args
        .get(1)
        .is_some_and(|arg| arg == wsp_core::git_process::TRAMPOLINE_MARKER)
    {
        match wsp_core::git_process::trampoline(&raw_args[2..]) {
            Ok(never) => match never {},
            Err(error) => {
                eprintln!("Git subprocess launch failed: {error}");
                process::exit(1);
            }
        }
    }
    exit_quietly_on_closed_output();
    init_platform();
    CompleteEnv::with_factory(cli::build_cli).complete();

    let _ = ctrlc::set_handler(move || {
        // Detached Git children do not receive terminal signals. Cancel owned
        // process groups before restoring the display and exiting.
        let _ = wsp_core::git_process::cancel_all();
        if std::io::stderr().is_terminal() {
            progress::restore_cursor();
        }
        process::exit(130);
    });

    let mut app = cli::build_cli();
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let matches = match app.try_get_matches_from_mut(&args) {
        Ok(matches) => matches,
        Err(error) if error.kind() == clap::error::ErrorKind::DisplayHelp => {
            let policy = pager::Policy::for_early_help(&args);
            match pager::write(
                error.to_string().as_bytes(),
                policy,
                pager::Config::Standard,
            ) {
                Ok(()) => process::exit(0),
                Err(error) => {
                    render_error(error, false);
                    process::exit(1);
                }
            }
        }
        Err(error) => error.exit(),
    };
    let json = matches.get_flag("json");
    let pager_policy = pager::Policy::from_matches(&matches, json);

    #[cfg(all(feature = "test-crash-barriers", wsp_crash_test, debug_assertions))]
    if let Err(err) = wsp_core::crash_barrier::initialize() {
        render_error(err, json);
        process::exit(1);
    }

    // Handle `wsp help [topic]` before general dispatch — it needs
    // the Command definition to print subcommand help.
    if let Some(("help", m)) = matches.subcommand() {
        match cli::help::run(m, &mut app, json, pager_policy) {
            Ok(_) => process::exit(0),
            Err(err) => {
                render_error(err, json);
                process::exit(1);
            }
        }
    }

    if let Some(("completion", m)) = matches.subcommand() {
        match cli::completion::run(m) {
            Ok(out) => {
                if let Err(err) = output::render(out, json, pager_policy) {
                    render_error(err, json);
                    process::exit(1);
                }
                process::exit(0);
            }
            Err(err) => {
                render_error(err, json);
                process::exit(1);
            }
        }
    }

    let mut progress_session = progress::Session::start(json);
    let context = match context::InvocationContext::resolve(&matches) {
        Ok(p) => p,
        Err(err) => {
            progress_session.finish();
            render_error(err, json);
            process::exit(1);
        }
    };

    let progress_config = context.config.progress.clone().unwrap_or_default();
    let invocation_mode = matches.get_one::<String>("git-progress").map(|mode| {
        wsp_core::git_policy::Mode::parse(mode).expect("clap validates progress modes")
    });
    let executable = match resolve_git_launcher(
        std::env::current_exe(),
        raw_args.first().map(std::ffi::OsString::as_os_str),
        std::env::current_dir().ok().as_deref(),
        std::env::var_os("PATH").as_deref(),
        is_executable_file,
    ) {
        Ok(path) => path,
        Err(err) => {
            progress_session.finish();
            render_error(err, json);
            process::exit(1);
        }
    };
    let _git_policy = wsp_core::git_policy::install(
        progress_config.mode.unwrap_or_default(),
        progress_config.repos,
        invocation_mode,
        executable,
        json,
    );

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

    let operation = wsp_core::progress::Progress::start(format!(
        "Running wsp {}",
        if command.is_empty() {
            "st".into()
        } else {
            command.replace('/', " ")
        }
    ));
    let dispatched = cli::dispatch(&matches, &context);
    operation.finish();
    progress_session.finish();
    match dispatched {
        Ok(out) => {
            let code = output::exit_code(&out);
            let mut output_progress = progress::Session::start(json);
            let rendered = output::render(out, json, pager_policy);
            output_progress.finish();
            if let Err(err) = rendered {
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
            let cfg = &context.config;
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
            if !context.is_workspace_local()
                && matches!(command.as_str(), "new" | "rm" | "rename" | "recover")
                && let Some(paths) = &context.paths
            {
                let mut cleanup_progress = progress::Session::start(json);
                wsp_core::gc::maybe_run(paths, cfg.retention_days());
                cleanup_progress.finish();
            }
            // Contextual hints (git-style advice.*) -- only on success
            if !json
                && code == 0
                && context.allows_global_advice(&command)
                && let Some(paths) = &context.paths
            {
                let mut advice_session = progress::Session::start(json);
                let advice = wsp_core::progress::Progress::start("Evaluating workspace advice");
                // One-time upgrade notice (version-gated, independent of cooldown).
                maybe_print_upgrade_notice(paths, cfg, &command);
                let hints = hints::evaluate(&command, cfg, paths);
                advice.finish();
                advice_session.finish();
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

/// Resolve the executable used to detach Git children even without `/proc`.
///
/// The OS-reported path is preferred. The fallback follows invocation path and
/// PATH semantics, which trust the caller's argv and environment rather than
/// authenticating that the selected file is the running executable. Resolve
/// relative paths now so later subprocess working directories cannot change it.
fn resolve_git_launcher(
    current_exe: std::io::Result<std::path::PathBuf>,
    argv0: Option<&std::ffi::OsStr>,
    cwd: Option<&std::path::Path>,
    search_path: Option<&std::ffi::OsStr>,
    is_executable: impl Fn(&std::path::Path) -> bool,
) -> anyhow::Result<std::path::PathBuf> {
    let original_error = match current_exe {
        Ok(path) => return Ok(path),
        Err(error) => error,
    };
    let absolute = |path: std::path::PathBuf| {
        if path.is_absolute() {
            Some(path)
        } else {
            cwd.map(|cwd| cwd.join(path))
        }
    };
    if let Some(argv0) = argv0.filter(|arg| !arg.is_empty()) {
        let path = std::path::Path::new(argv0);
        if path.is_absolute() || path.components().count() > 1 {
            if let Some(path) = absolute(path.to_path_buf())
                && is_executable(&path)
            {
                return Ok(path);
            }
        } else if let Some(search_path) = search_path {
            for directory in std::env::split_paths(search_path) {
                let Some(candidate) = absolute(directory.join(path)) else {
                    continue;
                };
                if is_executable(&candidate) {
                    return Ok(candidate);
                }
                #[cfg(windows)]
                if candidate.extension().is_none() {
                    let candidate = candidate.with_extension("exe");
                    if is_executable(&candidate) {
                        return Ok(candidate);
                    }
                }
            }
        }
    }
    anyhow::bail!(
        "cannot locate the wsp executable for isolated Git subprocesses ({original_error}); invoke wsp using an absolute executable path"
    )
}

fn is_executable_file(path: &std::path::Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
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
            wsp_core::progress::eprintln!(
                "hint: wsp upgraded from v{} to v{}. Run `wsp whatsnew` to see what changed.",
                last,
                current
            );
            wsp_core::progress::eprintln!("      (suppress: wsp config set advice.whatsnew false)");
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
    is_closed_pipe_message(msg)
}

/// The decision itself, over the panic message alone.
///
/// Separate from the hook so the narrowness can be tested. A `PanicHookInfo`
/// cannot be constructed, so a test of the hook can only prove that a broken
/// pipe exits quietly — never that anything else still surfaces, which is the
/// direction that loses data.
fn is_closed_pipe_message(msg: &str) -> bool {
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
    let _handoff = wsp_core::progress::suspend();
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

#[cfg(test)]
mod tests {
    use super::is_closed_pipe_message;

    #[test]
    fn git_launcher_resolution() {
        use std::ffi::OsString;
        use std::path::PathBuf;

        let root = if cfg!(windows) {
            PathBuf::from(r"C:\runtime")
        } else {
            PathBuf::from("/runtime")
        };
        let executable = root.join("wsp");
        let other = root.join("other");
        let search_path = std::env::join_paths([root.join("missing"), root.clone()]).unwrap();
        let relative_search_path = OsString::from("bin");
        struct Case {
            name: &'static str,
            os_path: Option<PathBuf>,
            argv0: Option<OsString>,
            cwd: Option<PathBuf>,
            path: Option<OsString>,
            available: Vec<PathBuf>,
            expected: Option<PathBuf>,
        }
        let cases = [
            Case {
                name: "prefer OS path over argv and PATH",
                os_path: Some(other.clone()),
                argv0: Some(executable.clone().into_os_string()),
                cwd: None,
                path: Some(search_path.clone()),
                available: vec![executable.clone()],
                expected: Some(other),
            },
            Case {
                name: "absolute argv works without cwd or proc",
                os_path: None,
                argv0: Some(executable.clone().into_os_string()),
                cwd: None,
                path: None,
                available: vec![executable.clone()],
                expected: Some(executable.clone()),
            },
            Case {
                name: "explicit relative argv resolves before changing cwd",
                os_path: None,
                argv0: Some(PathBuf::from(".").join("wsp").into_os_string()),
                cwd: Some(root.clone()),
                path: None,
                available: vec![root.join(".").join("wsp")],
                expected: Some(root.join(".").join("wsp")),
            },
            Case {
                name: "bare argv searches PATH in order",
                os_path: None,
                argv0: Some(OsString::from("wsp")),
                cwd: None,
                path: Some(search_path.clone()),
                available: vec![executable.clone()],
                expected: Some(executable.clone()),
            },
            Case {
                name: "empty PATH entry searches current directory",
                os_path: None,
                argv0: Some(OsString::from("wsp")),
                cwd: Some(root.clone()),
                path: Some(OsString::new()),
                available: vec![executable.clone()],
                expected: Some(executable.clone()),
            },
            Case {
                name: "relative PATH entries resolve against cwd",
                os_path: None,
                argv0: Some(OsString::from("wsp")),
                cwd: Some(root.clone()),
                path: Some(relative_search_path.clone()),
                available: vec![root.join("bin/wsp")],
                expected: Some(root.join("bin/wsp")),
            },
            Case {
                name: "explicit missing argv cannot fall back to another PATH file",
                os_path: None,
                argv0: Some(root.join("missing/wsp").into_os_string()),
                cwd: Some(root.clone()),
                path: Some(search_path),
                available: vec![executable],
                expected: None,
            },
            Case {
                name: "relative PATH requires cwd",
                os_path: None,
                argv0: Some(OsString::from("wsp")),
                cwd: None,
                path: Some(relative_search_path),
                available: vec![PathBuf::from("bin/wsp")],
                expected: None,
            },
            Case {
                name: "missing argv fails with actionable error",
                os_path: None,
                argv0: None,
                cwd: Some(root),
                path: None,
                available: vec![],
                expected: None,
            },
            Case {
                name: "empty argv fails",
                os_path: None,
                argv0: Some(OsString::new()),
                cwd: None,
                path: None,
                available: vec![],
                expected: None,
            },
        ];
        for case in cases {
            let os_path = case.os_path.ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::NotFound, "proc is unavailable")
            });
            let result = super::resolve_git_launcher(
                os_path,
                case.argv0.as_deref(),
                case.cwd.as_deref(),
                case.path.as_deref(),
                |candidate| case.available.iter().any(|path| path == candidate),
            );
            match case.expected {
                Some(expected) => assert_eq!(result.unwrap(), expected, "{}", case.name),
                None => {
                    let message = result.unwrap_err().to_string();
                    assert!(message.contains("proc is unavailable"), "{}", case.name);
                    assert!(
                        message.contains("invoke wsp using an absolute executable path"),
                        "{}",
                        case.name
                    );
                }
            }
        }
    }

    /// Written out per platform rather than derived from `CLOSED_PIPE_MARKERS`:
    /// building the expected message from the constant under test would pass
    /// even if the constant held the wrong code.
    #[cfg(unix)]
    #[test]
    fn a_closed_pipe_is_recognised() {
        assert!(is_closed_pipe_message(
            "failed printing to stdout: Broken pipe (os error 32)"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn a_closed_pipe_is_recognised() {
        // Windows reports one of two codes depending on which end noticed.
        assert!(is_closed_pipe_message(
            "failed printing to stdout: The pipe has been ended. (os error 109)"
        ));
        assert!(is_closed_pipe_message(
            "failed printing to stdout: The pipe is being closed. (os error 232)"
        ));
    }

    /// The numeric code carries the decision, so a message in another language
    /// still matches. Both platforms localize: glibc translates `strerror`, and
    /// Windows `FormatMessage` returns the system language.
    #[cfg(unix)]
    #[test]
    fn a_translated_message_still_matches_on_the_code() {
        assert!(is_closed_pipe_message(
            "failed printing to stdout: Relais brisé (pipe) (os error 32)"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn a_translated_message_still_matches_on_the_code() {
        assert!(is_closed_pipe_message(
            "failed printing to stdout: Le canal a été fermé. (os error 109)"
        ));
    }

    /// The one that matters. Exiting 0 here would report truncated output as
    /// success, so a full disk must reach the default hook and abort.
    #[test]
    fn other_write_failures_are_not_swallowed() {
        assert!(!is_closed_pipe_message(
            "failed printing to stdout: No space left on device (os error 28)"
        ));
        assert!(!is_closed_pipe_message(
            "failed printing to stdout: Input/output error (os error 5)"
        ));
    }

    #[test]
    fn unrelated_panics_are_not_swallowed() {
        assert!(!is_closed_pipe_message(
            "index out of bounds: the len is 3 but the index is 7"
        ));
        assert!(!is_closed_pipe_message(""));
        // A broken pipe that did not come from a write to our output.
        assert!(!is_closed_pipe_message("Broken pipe (os error 32)"));
    }
}
