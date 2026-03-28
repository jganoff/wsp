//! `wsp init` — scaffold or update a per-repo `.wsp.yaml` with `setup_commands`.

use std::io::Write as _;
use std::path::Path;

use anyhow::Result;
use clap::{Arg, ArgMatches, Command};

use wsp_core::config::Paths;
use wsp_core::output::{MutationOutput, Output};

pub fn cmd() -> Command {
    Command::new("init")
        .about("Create or update .wsp.yaml in the current repo")
        .long_about(
            "Create or update .wsp.yaml in the current repo.\n\n\
             Must be run from the root of a git repo. Interactively prompts for \
             setup_commands (post-clone hooks such as 'task setup' or 'lefthook install'). \
             If .wsp.yaml already exists, shows existing commands and lets you add or \
             replace them.\n\n\
             Use --print-sample to print a commented sample .wsp.yaml to stdout and exit.",
        )
        .arg(
            Arg::new("print-sample")
                .long("print-sample")
                .action(clap::ArgAction::SetTrue)
                .help("Print a commented sample .wsp.yaml to stdout and exit"),
        )
}

pub fn run(matches: &ArgMatches, _paths: &Paths) -> Result<Output> {
    if matches.get_flag("print-sample") {
        print_sample();
        return Ok(Output::None);
    }

    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        anyhow::bail!(
            "wsp init requires an interactive terminal.\n\
             To generate a sample .wsp.yaml without prompts, run: wsp init --print-sample"
        );
    }

    let cwd = std::env::current_dir()?;
    check_git_repo(&cwd)?;

    let existing =
        wsp_core::template::read_setup_commands(&cwd.join(".wsp.yaml")).unwrap_or_default();
    let commands = prompt_for_commands(&existing)?;

    wsp_core::template::write_setup_commands(&cwd.join(".wsp.yaml"), &commands)?;
    wsp_core::template::ensure_gitignore(&cwd)?;

    eprintln!("Wrote .wsp.yaml");
    eprintln!("Run `wsp repo setup` inside a workspace to execute these commands.");

    Ok(Output::Mutation(MutationOutput::new(
        "Wrote .wsp.yaml with setup_commands.",
    )))
}

fn check_git_repo(dir: &Path) -> Result<()> {
    // Use --show-toplevel to verify dir is the repo root, not just inside one.
    // Running `wsp init` from a subdirectory would write .wsp.yaml there, which
    // wsp cannot discover when cloning.
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()?;
    if !out.status.success() {
        anyhow::bail!(
            "not a git repository: {}\n\
             wsp init must be run from the root of a git repo.",
            dir.display()
        );
    }
    let toplevel = std::path::Path::new(std::str::from_utf8(&out.stdout)?.trim());
    // Canonicalize both sides — macOS /var -> /private/var symlink can cause mismatches.
    if dir.canonicalize()? != toplevel.canonicalize()? {
        anyhow::bail!(
            "not at repo root: {}\n\
             wsp init must be run from the repo root, not a subdirectory.\n\
             Try: cd {} && wsp init",
            dir.display(),
            toplevel.display()
        );
    }
    Ok(())
}

fn prompt_for_commands(existing: &[String]) -> Result<Vec<String>> {
    if existing.is_empty() {
        eprintln!("Enter setup_commands (e.g. 'task setup', 'lefthook install').");
        return collect_commands();
    }

    eprintln!("Existing setup_commands:");
    for cmd in existing {
        eprintln!("  - {}", cmd);
    }
    eprint!("\nReplace existing commands? [y/N]: ");
    std::io::stderr().flush()?;
    let answer = read_prompt()?;

    if matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        return collect_commands();
    }

    eprint!("Add more commands? [y/N]: ");
    std::io::stderr().flush()?;
    let answer = read_prompt()?;

    if matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        let mut commands = existing.to_vec();
        commands.extend(collect_commands()?);
        return Ok(commands);
    }

    Ok(existing.to_vec())
}

fn collect_commands() -> Result<Vec<String>> {
    eprintln!("Enter one command per line. Empty line to finish.");
    let mut commands = Vec::new();
    loop {
        eprint!("  command {}: ", commands.len() + 1);
        std::io::stderr().flush()?;
        let line = read_prompt()?;
        let trimmed = line.trim().to_string();
        if trimmed.is_empty() {
            break;
        }
        commands.push(trimmed);
    }
    Ok(commands)
}

/// Read one line from stdin. Per AGENTS.md: EOF returns `""`, Enter returns `"\n"` --
/// detect EOF before trimming.
fn read_prompt() -> Result<String> {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    if line.is_empty() {
        anyhow::bail!("aborted");
    }
    Ok(line)
}

fn print_sample() {
    print!(
        "# .wsp.yaml - per-repo setup commands\n\
#\n\
# Declare commands to run after this repo is cloned into a workspace.\n\
# wsp will show these commands and prompt for approval before running them.\n\
# Approving records the decision so future clones skip the prompt.\n\
#\n\
# Examples:\n\
#   setup_commands:\n\
#     - task setup          # install git hooks, generate files, etc.\n\
#     - lefthook install\n\
#     - npm install\n\
#\n\
setup_commands: []\n"
    );
}

#[cfg(test)]
mod tests {
    use wsp_core::template;

    #[test]
    fn write_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".wsp.yaml");
        let cmds = vec!["task setup".to_string(), "lefthook install".to_string()];
        template::write_setup_commands(&path, &cmds).unwrap();
        assert!(path.exists());
        let back = template::read_setup_commands(&path).unwrap_or_default();
        assert_eq!(back, cmds);
    }

    #[test]
    fn write_preserves_unknown_fields() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".wsp.yaml");
        std::fs::write(&path, "some_future_key: value\n").unwrap();
        template::write_setup_commands(&path, &["task setup".to_string()]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("some_future_key"),
            "unknown field lost: {content}"
        );
        assert!(
            content.contains("setup_commands"),
            "setup_commands missing: {content}"
        );
    }

    #[test]
    fn write_replaces_existing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".wsp.yaml");
        template::write_setup_commands(&path, &["old".to_string()]).unwrap();
        template::write_setup_commands(&path, &["new".to_string()]).unwrap();
        let back = template::read_setup_commands(&path).unwrap_or_default();
        assert_eq!(back, vec!["new".to_string()]);
    }

    #[test]
    fn write_empty_removes_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".wsp.yaml");
        template::write_setup_commands(&path, &["task setup".to_string()]).unwrap();
        template::write_setup_commands(&path, &[]).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            !content.contains("setup_commands"),
            "key not removed: {content}"
        );
    }

    #[test]
    fn write_rejects_non_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".wsp.yaml");
        std::fs::write(&path, "- item1\n- item2\n").unwrap();
        let err = template::write_setup_commands(&path, &["task setup".to_string()]).unwrap_err();
        assert!(
            err.to_string().contains("not a YAML mapping"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn ensure_gitignore_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        template::ensure_gitignore(dir.path()).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(content, ".wsp.yaml.lock\n");
    }

    #[test]
    fn ensure_gitignore_appends_to_existing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        template::ensure_gitignore(dir.path()).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(content, "target/\n.wsp.yaml.lock\n");
    }

    #[test]
    fn ensure_gitignore_appends_with_missing_newline() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "target/").unwrap();
        template::ensure_gitignore(dir.path()).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(content, "target/\n.wsp.yaml.lock\n");
    }

    #[test]
    fn ensure_gitignore_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), ".wsp.yaml.lock\nother\n").unwrap();
        template::ensure_gitignore(dir.path()).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
        assert_eq!(content, ".wsp.yaml.lock\nother\n");
    }
}
