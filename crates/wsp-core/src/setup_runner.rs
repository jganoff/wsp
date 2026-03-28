//! Interactive prompt and execution of per-repo setup commands.
//!
//! Each command is executed via `sh -c <cmd>` in the repo's clone directory.
//! **The approval flow is the security boundary**: users see the exact commands
//! before anything runs. Approvals are stored by content hash so wsp only
//! re-prompts when the commands change — the same model direnv uses.
//!
//! By default the runner deduplicates the resolved command list (first
//! occurrence wins) before showing the prompt. Pass the undeduped
//! `ResolvedSetup` when `--all` is requested.
//!
//! # Security note
//! `setup_commands` intentionally allows arbitrary shell execution (that is the
//! point — `task setup`, `npm install`, etc.). Never auto-run without user
//! approval. The hash-based store ensures a supply-chain change (new command
//! added to `.wsp.yaml`) triggers a fresh prompt even for previously approved repos.

use std::io::IsTerminal as _;
use std::path::Path;

use anyhow::Result;

use crate::approvals;
use crate::setup_commands::ResolvedSetup;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run resolved setup commands for a single repo clone, with the approval flow.
///
/// Behaviour:
/// - **Approved** (in store): runs without prompting.
/// - **Non-interactive** (stdin is not a tty): prints a notice and skips.
/// - **Interactive**: shows commands with provenance labels, prompts `[y/N]`:
///   - `y` → record approval + run
///   - anything else / empty → skip
///
/// Saying `y` records approval so future runs skip the prompt (like
/// `direnv allow`). If the commands change, the hash changes and wsp
/// re-prompts.
///
/// Returns `Ok(true)` if commands were run, `Ok(false)` if skipped.
/// A non-zero exit from a command is printed as a warning but does not
/// return an error — setup failures are not fatal to workspace creation.
pub fn maybe_run_resolved(
    data_dir: &Path,
    clone_dir: &Path,
    identity: &str,
    resolved: &ResolvedSetup,
) -> Result<bool> {
    if resolved.is_empty() {
        return Ok(false);
    }

    let hash = approvals::commands_hash(&resolved.commands);
    let store = approvals::load(data_dir)?;

    if approvals::is_approved(&store, identity, &hash) {
        eprintln!("Running pre-approved setup for {}...", identity);
        run_commands(clone_dir, &resolved.commands);
        return Ok(true);
    }

    if !std::io::stdin().is_terminal() {
        eprintln!(
            "notice: skipping setup commands for {} (non-interactive; run `wsp repo setup` to approve)",
            identity
        );
        return Ok(false);
    }

    prompt_and_run_resolved(data_dir, clone_dir, identity, resolved, &hash)
}

/// Like [`maybe_run_resolved`] but always prompts, ignoring any existing approval.
/// Used by `wsp repo setup --force` and `wsp doctor --fix`.
pub fn prompt_and_run_resolved_setup(
    data_dir: &Path,
    clone_dir: &Path,
    identity: &str,
    resolved: &ResolvedSetup,
) -> Result<bool> {
    if resolved.is_empty() {
        return Ok(false);
    }
    if !std::io::stdin().is_terminal() {
        eprintln!(
            "notice: skipping setup commands for {} (non-interactive)",
            identity
        );
        return Ok(false);
    }
    let hash = approvals::commands_hash(&resolved.commands);
    prompt_and_run_resolved(data_dir, clone_dir, identity, resolved, &hash)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn prompt_and_run_resolved(
    data_dir: &Path,
    clone_dir: &Path,
    identity: &str,
    resolved: &ResolvedSetup,
    hash: &str,
) -> Result<bool> {
    eprintln!("\nSetup commands for {}:", identity);
    for cmd in &resolved.commands {
        eprintln!("  {}", cmd);
    }
    eprint!("Run these commands? [y/N] ");

    let line = read_line()?;

    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => {
            approvals::record_always(data_dir, identity, hash)?;
            run_commands(clone_dir, &resolved.commands);
            Ok(true)
        }
        _ => {
            eprintln!("Skipping setup for {}.", identity);
            Ok(false)
        }
    }
}

/// Read one line from stdin. Returns `Err` on EOF (aborted).
fn read_line() -> Result<String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    // Empty string means EOF (not just Enter, which gives "\n").
    if line.is_empty() {
        anyhow::bail!("aborted");
    }
    Ok(line)
}

/// Run each command in `clone_dir`. Non-zero exits are printed as warnings.
fn run_commands(clone_dir: &Path, commands: &[String]) {
    for cmd in commands {
        match std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(clone_dir)
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => {
                eprintln!(
                    "  warning: {:?} exited with {}",
                    cmd,
                    status
                        .code()
                        .map_or("signal".to_string(), |c| c.to_string())
                );
            }
            Err(e) => {
                eprintln!("  warning: could not run {:?}: {}", cmd, e);
            }
        }
    }
}
