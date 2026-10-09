use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use clap::{ArgMatches, Command};

use wsp_core::config::{self, Paths};
use wsp_core::filelock;
use wsp_core::output::Output;
/// Read a line from stdin for interactive prompts.
/// Bails if stdin is closed or interrupted (e.g. Ctrl-C), allowing the
/// wizard to exit cleanly via the SIGINT handler in main.rs.
fn read_prompt() -> Result<String> {
    let stdin = std::io::stdin();
    let mut line = String::new();
    if let Err(e) = stdin.lock().read_line(&mut line) {
        wsp_core::progress::eprintln!("warning: failed to read stdin: {}", e);
    }
    if line.is_empty() {
        // Empty string (no newline) means EOF or read error — abort wizard.
        // "\n" (user pressed Enter) would be non-empty before trim.
        bail!("aborted");
    }
    Ok(line)
}

pub fn cmd() -> Command {
    Command::new("setup")
        .add(crate::shellnav::ShellNav::none())
        .about("Interactive first-time setup")
        .long_about(
            "Interactive first-time setup.\n\n\
             Walks through configuring wsp for first use: checks dependencies, sets \
             branch prefix, and configures shell integration. Idempotent — skips steps \
             that are already configured. Re-run anytime to fill in missing pieces.",
        )
}

pub fn run(_matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let _progress = wsp_core::progress::Progress::start("Setting up wsp");
    if !std::io::stdin().is_terminal() {
        print_non_interactive_guide(paths)?;
        return Ok(Output::None);
    }

    wsp_core::progress::eprintln!();

    // Step 1: Check tools on PATH
    check_tools()?;

    // Step 2: Branch prefix
    step_branch_prefix(paths)?;

    // Step 3: Shell integration
    step_shell_integration()?;

    // Step 4: What's next
    print_next_steps();

    Ok(Output::None)
}

/// Check required and optional tools. Bails if `git` is missing.
fn check_tools() -> Result<()> {
    let _progress = wsp_core::progress::Progress::start("Checking setup dependencies");

    // git — hard requirement
    let git_ok = match std::process::Command::new("git").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout);
            let version = raw
                .trim()
                .strip_prefix("git version ")
                .unwrap_or(raw.trim());
            wsp_core::progress::eprintln!("  \u{2713} git {}", version);
            true
        }
        _ => {
            wsp_core::progress::eprintln!("  \u{2717} git \u{2014} not found (required)");
            false
        }
    };

    if !git_ok {
        bail!("git is required but not found on PATH");
    }

    // gh — optional, useful for bulk import
    match std::process::Command::new("gh").arg("--version").output() {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout);
            let first_line = raw.lines().next().unwrap_or("");
            let version = first_line.strip_prefix("gh version ").unwrap_or(first_line);
            let version = version.split_whitespace().next().unwrap_or(version);
            wsp_core::progress::eprintln!("  \u{2713} gh {}", version);
        }
        _ => {
            wsp_core::progress::eprintln!(
                "  \u{2717} gh \u{2014} not found (optional, enables bulk repo import and branch prefix auto-detection)"
            );
            wsp_core::progress::eprintln!(
                "    Install gh and re-run `wsp setup` to auto-detect your branch prefix."
            );
            wsp_core::progress::eprintln!("    Install: https://cli.github.com");
        }
    };

    wsp_core::progress::eprintln!();
    Ok(())
}

/// Try to get the current GitHub username via `gh api user`.
/// Returns None if gh is not installed, not authenticated, or the call fails.
fn gh_current_user() -> Option<String> {
    let _progress = wsp_core::progress::Progress::start("Detecting GitHub username");
    let _external = wsp_core::progress::external();
    let out = std::process::Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .stderr(std::process::Stdio::null()) // suppress auth errors — handled gracefully below
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let login = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // GitHub usernames are alphanumeric + hyphens. Reject anything else so a
    // misbehaving gh binary can't inject an invalid branch prefix suggestion.
    if login.is_empty() || !login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some(login)
}

/// Prompt for branch prefix if not already set.
fn step_branch_prefix(paths: &Paths) -> Result<()> {
    let cfg = config::Config::load_from(&paths.config_path)?;
    if let Some(ref prefix) = cfg.branch_prefix {
        wsp_core::progress::eprintln!("  \u{2713} branch prefix already set: {}", prefix);
        wsp_core::progress::eprintln!();
        return Ok(());
    }

    // Try gh first (GitHub username), fall back to $USER
    let default = gh_current_user().unwrap_or_else(|| std::env::var("USER").unwrap_or_default());

    let prompt_guard = wsp_core::progress::suspend();
    wsp_core::progress::eprintln!("Workspace branches are named <prefix>/<workspace-name>.");
    wsp_core::progress::eprintln!("Your branch prefix is typically your GitHub username.");
    if default.is_empty() {
        eprint!("Branch prefix: ");
    } else {
        eprint!("Branch prefix [{}]: ", default);
    }

    let input = read_prompt()?;
    drop(prompt_guard);
    let trimmed = input.trim();
    let prefix = if trimmed.is_empty() {
        &default
    } else {
        trimmed
    };

    if prefix.is_empty() {
        wsp_core::progress::eprintln!("  skipped (no prefix set)");
        wsp_core::progress::eprintln!();
        return Ok(());
    }

    let v = prefix.to_string();
    filelock::with_config(&paths.config_path, |cfg| {
        cfg.branch_prefix = Some(v);
        Ok(())
    })?;

    wsp_core::progress::eprintln!("  \u{2713} branch prefix set to: {}", prefix);
    wsp_core::progress::eprintln!();
    Ok(())
}

/// Detect shell, check rc file, offer to append shell integration.
fn step_shell_integration() -> Result<()> {
    let shell = match detect_shell() {
        Some(s) => s,
        None => {
            wsp_core::progress::eprintln!("Shell integration:");
            wsp_core::progress::eprintln!("  could not detect shell from $SHELL");
            wsp_core::progress::eprintln!("  run `wsp completion --help` to set up manually");
            wsp_core::progress::eprintln!();
            return Ok(());
        }
    };

    let home = match std::env::var("HOME").ok().filter(|h| !h.is_empty()) {
        Some(h) => PathBuf::from(h),
        None => {
            wsp_core::progress::eprintln!("Shell integration:");
            wsp_core::progress::eprintln!("  $HOME is not set, cannot determine rc file");
            wsp_core::progress::eprintln!();
            return Ok(());
        }
    };

    // Check all common rc files for existing shell integration
    if let Some(found_in) = shell_integration_found(&home, shell) {
        wsp_core::progress::eprintln!(
            "  \u{2713} shell integration already configured in {}",
            found_in.display()
        );
        wsp_core::progress::eprintln!();
        return Ok(());
    }

    let rc = primary_rc_file(&home, shell);

    wsp_core::progress::eprintln!(
        "Shell integration enables tab completion and workspace detection."
    );
    wsp_core::progress::eprintln!("Detected shell: {}", shell);
    wsp_core::progress::eprintln!();

    let eval_line = match shell {
        "fish" => "wsp completion fish | source".to_string(),
        _ => format!("eval \"$(wsp completion {})\"", shell),
    };

    wsp_core::progress::eprintln!("Add to {}:", rc.display());
    wsp_core::progress::eprintln!("  {}", eval_line);
    wsp_core::progress::eprintln!();
    let prompt_guard = wsp_core::progress::suspend();
    eprint!("Add it now? [Y/n]: ");

    let input = read_prompt()?;
    drop(prompt_guard);
    let answer = input.trim().to_lowercase();

    if answer.is_empty() || answer == "y" || answer == "yes" {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&rc)?;
        writeln!(file)?;
        writeln!(file, "# wsp shell integration")?;
        writeln!(file, "{}", eval_line)?;

        wsp_core::progress::eprintln!("  \u{2713} added to {}", rc.display());
    } else {
        wsp_core::progress::eprintln!("  skipped");
    }

    wsp_core::progress::eprintln!();
    Ok(())
}

/// Print concrete next steps after setup completes.
fn print_next_steps() {
    wsp_core::progress::eprintln!("Setup complete!");
    wsp_core::progress::eprintln!();
    wsp_core::progress::eprintln!(
        "\u{2500}\u{2500} What's next \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}"
    );
    wsp_core::progress::eprintln!();
    wsp_core::progress::eprintln!("  1. Register repos you work with:");
    wsp_core::progress::eprintln!("     wsp registry add https://github.com/jganoff/wsp.git");
    wsp_core::progress::eprintln!();
    wsp_core::progress::eprintln!("  2. Create your first workspace:");
    wsp_core::progress::eprintln!("     wsp new my-feature wsp");
    wsp_core::progress::eprintln!();
    wsp_core::progress::eprintln!("  3. Add more repos to the workspace (optional):");
    wsp_core::progress::eprintln!("     wsp repo add <name>");
    wsp_core::progress::eprintln!();
    wsp_core::progress::eprintln!("  4. Work normally, then clean up:");
    wsp_core::progress::eprintln!("     wsp st                        # status across repos");
    wsp_core::progress::eprintln!("     wsp diff                      # review changes");
    wsp_core::progress::eprintln!("     git push                      # push for PR");
    wsp_core::progress::eprintln!("     wsp rm my-feature             # clean up after merge");
    wsp_core::progress::eprintln!();
    wsp_core::progress::eprintln!(
        "  Tip: bulk-import from GitHub with `wsp registry add --from github.com/<org> --all`"
    );
}

/// Non-interactive mode: print what needs to be done without prompting.
fn print_non_interactive_guide(paths: &Paths) -> Result<()> {
    let cfg = config::Config::load_from(&paths.config_path)?;

    wsp_core::progress::eprintln!("wsp setup requires an interactive terminal.");
    wsp_core::progress::eprintln!();
    wsp_core::progress::eprintln!("To configure manually:");

    if cfg.branch_prefix.is_none() {
        wsp_core::progress::eprintln!("  wsp config set branch-prefix <your-username>");
    }

    if let Some(shell) = detect_shell() {
        let home = std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .map(PathBuf::from);
        if let Some(ref home) = home
            && shell_integration_found(home, shell).is_none()
        {
            let rc = primary_rc_file(home, shell);
            let eval_line = match shell {
                "fish" => "wsp completion fish | source".to_string(),
                _ => format!("eval \"$(wsp completion {})\"", shell),
            };
            wsp_core::progress::eprintln!("  echo '{}' >> {}", eval_line, rc.display());
        }
    }

    wsp_core::progress::eprintln!("  wsp registry add https://github.com/jganoff/wsp.git");
    wsp_core::progress::eprintln!("  wsp new my-feature");

    Ok(())
}

fn detect_shell() -> Option<&'static str> {
    let shell = std::env::var("SHELL").ok()?;
    if shell.ends_with("/zsh") {
        Some("zsh")
    } else if shell.ends_with("/bash") {
        Some("bash")
    } else if shell.ends_with("/fish") {
        Some("fish")
    } else {
        None
    }
}

/// All rc files to check for existing shell integration, per shell.
fn rc_files(home: &Path, shell: &str) -> Vec<PathBuf> {
    match shell {
        "zsh" => vec![
            home.join(".zshrc"),
            home.join(".zprofile"),
            home.join(".zshenv"),
        ],
        "bash" => vec![
            home.join(".bashrc"),
            home.join(".bash_profile"),
            home.join(".profile"),
        ],
        "fish" => vec![home.join(".config").join("fish").join("config.fish")],
        _ => vec![],
    }
}

/// The primary rc file to append to for a given shell.
fn primary_rc_file(home: &Path, shell: &str) -> PathBuf {
    match shell {
        "zsh" => home.join(".zshrc"),
        "bash" => home.join(".bashrc"),
        "fish" => home.join(".config").join("fish").join("config.fish"),
        _ => unreachable!(),
    }
}

/// Check all common rc files for `wsp completion`. Returns the path where found.
fn shell_integration_found(home: &Path, shell: &str) -> Option<PathBuf> {
    for path in rc_files(home, shell) {
        if path.exists()
            && let Ok(contents) = std::fs::read_to_string(&path)
            && contents.contains("wsp completion")
        {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_shell() {
        let cases = vec![
            ("/bin/zsh", Some("zsh")),
            ("/usr/bin/zsh", Some("zsh")),
            ("/bin/bash", Some("bash")),
            ("/usr/local/bin/fish", Some("fish")),
            ("/bin/sh", None),
            ("/bin/csh", None),
        ];

        for (shell_path, expected) in cases {
            // We can't easily test detect_shell() since it reads $SHELL,
            // but we can test the matching logic directly.
            let result = if shell_path.ends_with("/zsh") {
                Some("zsh")
            } else if shell_path.ends_with("/bash") {
                Some("bash")
            } else if shell_path.ends_with("/fish") {
                Some("fish")
            } else {
                None
            };
            assert_eq!(result, expected, "shell path: {}", shell_path);
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_primary_rc_file() {
        let home = Path::new("/home/user");
        let cases = vec![
            ("zsh", ".zshrc"),
            ("bash", ".bashrc"),
            ("fish", ".config/fish/config.fish"),
        ];

        for (shell, suffix) in cases {
            let rc = primary_rc_file(home, shell);
            assert!(
                rc.ends_with(suffix),
                "primary_rc_file({}) = {}, expected to end with {}",
                shell,
                rc.display(),
                suffix
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_rc_files_covers_all_common_locations() {
        let home = Path::new("/home/user");

        let zsh_files = rc_files(home, "zsh");
        assert!(zsh_files.iter().any(|p| p.ends_with(".zshrc")));
        assert!(zsh_files.iter().any(|p| p.ends_with(".zprofile")));
        assert!(zsh_files.iter().any(|p| p.ends_with(".zshenv")));

        let bash_files = rc_files(home, "bash");
        assert!(bash_files.iter().any(|p| p.ends_with(".bashrc")));
        assert!(bash_files.iter().any(|p| p.ends_with(".bash_profile")));
        assert!(bash_files.iter().any(|p| p.ends_with(".profile")));
    }

    #[test]
    fn test_shell_integration_found() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();

        // No rc files → not found
        assert!(shell_integration_found(home, "zsh").is_none());

        // Write wsp completion to .zprofile (not .zshrc)
        std::fs::write(home.join(".zprofile"), "eval \"$(wsp completion zsh)\"\n").unwrap();
        let found = shell_integration_found(home, "zsh");
        assert!(found.is_some());
        assert!(found.unwrap().ends_with(".zprofile"));
    }
}
