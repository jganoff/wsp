//! Integration tests that spawn real shells to verify tab-completion works
//! end-to-end.
//!
//! These tests catch runtime behavior that string-pattern unit tests cannot —
//! e.g. PowerShell 5.1 silently dropping empty string arguments when calling
//! native executables with `&`, which breaks `wsp <TAB>` (empty wordToComplete).
//!
//! Each test simulates what the Register-ArgumentCompleter / complete scriptblock
//! does when the user presses TAB with an empty prefix, directly exercising the
//! clap_complete invocation path.

const WSP: &str = env!("CARGO_BIN_EXE_wsp");

// ---------------------------------------------------------------------------
// Windows — PowerShell
// ---------------------------------------------------------------------------

/// Empty-word completion (wsp <TAB>): the empty case exercises the --% fix.
///
/// PowerShell 5.1 silently drops `''` when passed to a native exe via `&`.
/// The generated scriptblock uses `--% ` (stop-parsing) so the trailing `""`
/// survives as a raw command-line token that CommandLineToArgvW parses as an
/// empty string. This test verifies that mechanism works end-to-end.
#[test]
#[cfg(windows)]
fn powershell_empty_word_completion_returns_subcommands() {
    let wsp_ps = WSP.replace('\'', "''"); // escape ' for PS single-quoted string
    // --% passes everything after it verbatim to CreateProcess. "" in the raw
    // Windows command line is how an empty argument is represented;
    // CommandLineToArgvW parses it as an empty string.
    // (The generated scriptblock uses Invoke-Expression @"..."@ which expands
    // `"`" → "" before --% sees it — same net effect via a different route.)
    let script = format!("$env:COMPLETE = 'powershell'; & '{wsp_ps}' --% -- wsp \"\"");

    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("powershell.exe not found — should always be present on Windows");

    assert!(
        out.status.success(),
        "completion exited non-zero\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.is_empty(),
        "expected completions but got empty output\nstderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        stdout.contains("new"),
        "expected 'new' subcommand in completions\ngot:\n{stdout}"
    );
    assert!(
        stdout.contains("list"),
        "expected 'list' subcommand in completions\ngot:\n{stdout}"
    );
}

/// Non-empty prefix completion (wsp n<TAB>): the else branch, guards against
/// regressions in the normal completion path.
#[test]
#[cfg(windows)]
fn powershell_prefix_completion_returns_matching() {
    let wsp_ps = WSP.replace('\'', "''");
    let script = format!("$env:COMPLETE = 'powershell'; & '{wsp_ps}' -- wsp n");

    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .expect("powershell.exe not found");

    assert!(out.status.success(), "completion exited non-zero");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("new"),
        "expected 'new' for prefix 'n'\ngot:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// Unix — bash
// ---------------------------------------------------------------------------

/// Empty-word completion under bash.
///
/// bash passes empty strings to child processes correctly (no PS 5.1 bug).
/// bash's write_complete uses _CLAP_COMPLETE_INDEX from the environment rather
/// than args.len()-1, so we set it to 1 and pass an explicit empty token.
/// This guards against regressions in the completion engine itself.
#[test]
#[cfg(unix)]
fn bash_empty_word_completion_returns_subcommands() {
    let wsp_esc = WSP.replace('\'', "'\\''"); // POSIX single-quote escape
    let script =
        format!("_CLAP_COMPLETE_INDEX=1 _CLAP_IFS=$'\\n' COMPLETE=bash '{wsp_esc}' -- wsp ''");

    let out = std::process::Command::new("bash")
        .args(["-c", &script])
        .output()
        .expect("bash not found — should always be present on Unix");

    assert!(
        out.status.success(),
        "completion exited non-zero\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.is_empty(),
        "expected completions but got empty output\nstderr: {}",
        String::from_utf8_lossy(&out.stderr),
    );
    assert!(
        stdout.contains("new"),
        "expected 'new' subcommand in completions\ngot:\n{stdout}"
    );
    assert!(
        stdout.contains("list"),
        "expected 'list' subcommand in completions\ngot:\n{stdout}"
    );
}

/// Non-empty prefix completion under bash: guards the normal path.
#[test]
#[cfg(unix)]
fn bash_prefix_completion_returns_matching() {
    let wsp_esc = WSP.replace('\'', "'\\''");
    let script =
        format!("_CLAP_COMPLETE_INDEX=1 _CLAP_IFS=$'\\n' COMPLETE=bash '{wsp_esc}' -- wsp n");

    let out = std::process::Command::new("bash")
        .args(["-c", &script])
        .output()
        .expect("bash not found");

    assert!(out.status.success(), "completion exited non-zero");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("new"),
        "expected 'new' for prefix 'n'\ngot:\n{stdout}"
    );
}
