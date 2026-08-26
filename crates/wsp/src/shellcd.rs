//! Shell cd requests.
//!
//! The wrapper function cannot learn a destination from the binary's stdout —
//! human-readable progress output shares that channel — so it passes the path
//! of a scratch file in `WSP_CD_FILE` and reads the directory back after the
//! process exits.
//!
//! This exists because the wrapper cannot correctly re-derive the destination
//! from argv. `wsp new` has five value-taking flags (`-b`, `-t`, `-w`, `-f`,
//! `-d`) and may omit the workspace name entirely, deriving it from the branch
//! given to `-b`. No positional scan in shell gets that right: taking `$1`
//! mistakes a leading flag for the name, and taking the first non-flag token
//! mistakes a flag's *value* for it — so `wsp new -w existing new-ws` lands in
//! `existing`. Reporting the resolved directory from the one place that already
//! knows it removes the guesswork.

use std::path::Path;

/// Ask the calling shell to change directory to `dir`.
///
/// A no-op when `WSP_CD_FILE` is unset, so piping, scripts, and shells without
/// the wrapper loaded are unaffected.
///
/// Write failures are ignored on purpose. By the time this is called the
/// command has already done its work; not moving the shell is a cosmetic loss
/// and not worth failing an otherwise successful operation over.
pub fn request(dir: &Path) {
    let Some(file) = std::env::var_os("WSP_CD_FILE") else {
        return;
    };
    let _ = std::fs::write(file, dir.to_string_lossy().as_bytes());
}

/// The directory the user ran the command from.
///
/// Normally this is the process cwd. But the wrapper *vacates* — steps out of
/// the workspaces tree — before invoking commands that relocate or remove a
/// directory, because Windows cannot rename or delete a live process's cwd.
/// That would otherwise destroy cwd-based workspace detection: `wsp rm` takes
/// the workspace name as an optional positional and falls back to detecting it
/// from where the user was standing.
///
/// So the wrapper forwards the original directory in `WSP_PWD`, and every
/// command resolves "where is the user" through here rather than calling
/// `current_dir()` directly. Not only the ones behind a vacating wrapper case:
/// which cases vacate changes, and a command that reads the process cwd is
/// wrong the moment one is added above it. `cli::tests::the_cli_asks_where_the
/// _user_is` keeps that from happening quietly.
///
/// Falls back to the real cwd when `WSP_PWD` is unset or does not name an
/// existing directory, so an unwrapped shell and a stale value both behave
/// exactly as before.
pub fn invocation_dir() -> std::io::Result<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("WSP_PWD") {
        let p = std::path::PathBuf::from(p);
        if p.is_dir() {
            return Ok(p);
        }
    }
    std::env::current_dir()
}
