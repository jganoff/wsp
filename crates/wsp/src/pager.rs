use std::ffi::{OsStr, OsString};
use std::io::{IsTerminal, Write};
use std::path::Path;
#[cfg(any(windows, test))]
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use clap::ArgMatches;
use terminal_size::{Width, terminal_size};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    Never,
    Auto,
    Always,
}

impl Policy {
    pub fn from_matches(matches: &ArgMatches, json: bool) -> Self {
        if json || matches.get_flag("no-pager") {
            Self::Never
        } else if matches.get_flag("paginate") {
            Self::Always
        } else {
            Self::Auto
        }
    }

    pub fn for_early_help(args: &[OsString]) -> Self {
        let flags = args
            .iter()
            .skip(1)
            .take_while(|arg| arg.as_os_str() != "--");
        let mut paginate = false;
        for arg in flags {
            match arg.to_str() {
                Some("--json" | "--no-pager") => return Self::Never,
                Some("--paginate") => paginate = true,
                _ => {}
            }
        }
        if paginate { Self::Always } else { Self::Never }
    }

    fn should_page(self, stdout_is_terminal: bool) -> bool {
        match self {
            Self::Never => false,
            Self::Auto => stdout_is_terminal,
            Self::Always => true,
        }
    }
}

pub enum Config<'a> {
    Git { command: &'a str },
    Standard,
}

/// Deliver a complete human-readable document to stdout or its configured pager.
pub fn write(contents: &[u8], policy: Policy, config: Config<'_>) -> Result<()> {
    if contents.is_empty() {
        return Ok(());
    }
    if !policy.should_page(std::io::stdout().is_terminal()) {
        return write_stdout(contents);
    }

    let pager = match config {
        Config::Git { command } => resolve_git_pager(command, policy),
        Config::Standard => resolve_standard_pager(),
    };
    let Some(pager) = pager else {
        return write_stdout(contents);
    };

    write_to_pager(&pager, contents)
}

fn write_stdout(contents: &[u8]) -> Result<()> {
    std::io::stdout().write_all(contents)?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum CommandPager {
    Disabled,
    Default,
    Command(OsString),
}

fn resolve_git_pager(command: &str, policy: Policy) -> Option<OsString> {
    let command_pager = read_command_pager(&format!("pager.{command}"));
    select_git_pager(
        policy,
        command_pager,
        std::env::var_os("GIT_PAGER"),
        read_git_config("core.pager").map(OsString::from),
        std::env::var_os("PAGER"),
        git_default_pager(),
    )
}

fn select_git_pager(
    policy: Policy,
    command_pager: Option<CommandPager>,
    git_pager: Option<OsString>,
    core_pager: Option<OsString>,
    pager: Option<OsString>,
    default_pager: Option<OsString>,
) -> Option<OsString> {
    if policy != Policy::Always && command_pager == Some(CommandPager::Disabled) {
        return None;
    }

    let pager = git_pager
        .or(match command_pager {
            Some(CommandPager::Command(value)) => Some(value),
            _ => None,
        })
        .or(core_pager)
        .or(pager)
        .or(default_pager)?;
    enabled_pager(pager)
}

fn resolve_standard_pager() -> Option<OsString> {
    let pager = std::env::var_os("PAGER").unwrap_or_else(|| OsString::from("less"));
    enabled_pager(pager)
}

fn enabled_pager(pager: OsString) -> Option<OsString> {
    if pager.is_empty() || pager == "cat" {
        None
    } else {
        Some(pager)
    }
}

fn read_git_config(key: &str) -> Option<String> {
    read_git_config_scope("--global", key, None)
        .or_else(|| read_git_config_scope("--system", key, None))
}

fn read_command_pager(key: &str) -> Option<CommandPager> {
    for scope in ["--global", "--system"] {
        let Some(raw) = read_git_config_scope(scope, key, None) else {
            continue;
        };
        if let Some(value) = read_git_config_scope(scope, key, Some("bool")) {
            return match value.as_str() {
                "true" => Some(CommandPager::Default),
                "false" => Some(CommandPager::Disabled),
                _ => None,
            };
        }
        return Some(CommandPager::Command(raw.into()));
    }
    None
}

fn read_git_config_scope(scope: &str, key: &str, value_type: Option<&str>) -> Option<String> {
    let mut command = Command::new("git");
    command.args(["config", "--includes", scope]);
    if let Some(value_type) = value_type {
        command.arg(format!("--type={value_type}"));
    }
    let output = command.arg("--get").arg(key).output().ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_string()
    })
}

fn git_default_pager() -> Option<OsString> {
    let isolated = tempfile::Builder::new()
        .prefix("wsp-git-pager-")
        .tempdir()
        .ok()?;
    let output = git_default_pager_command(isolated.path()).output().ok()?;
    output.status.success().then(|| {
        OsString::from(
            String::from_utf8_lossy(&output.stdout)
                .trim_end_matches(['\r', '\n'])
                .to_string(),
        )
    })
}

fn git_default_pager_command(isolated: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .args(["var", "GIT_PAGER"])
        .current_dir(isolated)
        .env_remove("GIT_PAGER")
        .env_remove("PAGER")
        .env_remove("GIT_CONFIG_COUNT")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", isolated.join("config"))
        .env("GIT_DIR", isolated.join("nonexistent-git-dir"));
    command
}

fn git_shell() -> OsString {
    select_git_shell(query_git_shell(), fallback_git_shell)
}

fn query_git_shell() -> Option<OsString> {
    let output = Command::new("git")
        .args(["var", "GIT_SHELL_PATH"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout);
    let path = path.trim_end_matches(['\r', '\n']);
    (!path.is_empty()).then(|| path.into())
}

fn select_git_shell(configured: Option<OsString>, fallback: impl FnOnce() -> OsString) -> OsString {
    configured.unwrap_or_else(fallback)
}

#[cfg(not(windows))]
fn fallback_git_shell() -> OsString {
    OsString::from("sh")
}

#[cfg(windows)]
fn fallback_git_shell() -> OsString {
    let output = Command::new("git").arg("--exec-path").output();
    if let Ok(output) = output
        && output.status.success()
    {
        let exec_path = PathBuf::from(
            String::from_utf8_lossy(&output.stdout)
                .trim_end_matches(['\r', '\n'])
                .to_string(),
        );
        if let Some(shell) = git_for_windows_shell(&exec_path) {
            if shell.is_file() {
                return shell.into_os_string();
            }
        }
    }
    OsString::from("sh")
}

#[cfg(any(windows, test))]
fn git_for_windows_shell(exec_path: &Path) -> Option<PathBuf> {
    let root = exec_path.ancestors().nth(3)?;
    Some(root.join("usr").join("bin").join("sh.exe"))
}

fn write_to_pager(pager: &OsStr, contents: &[u8]) -> Result<()> {
    let mut command = Command::new(git_shell());
    command.arg("-c").arg(pager);
    command
        .env("GIT_PAGER_IN_USE", "true")
        .env(
            "LESS",
            std::env::var_os("LESS").unwrap_or_else(|| "FRX".into()),
        )
        .env("LV", std::env::var_os("LV").unwrap_or_else(|| "-c".into()))
        .stdin(Stdio::piped());
    if std::env::var_os("COLUMNS").is_none()
        && let Some((Width(width), _)) = terminal_size()
    {
        command.env("COLUMNS", width.to_string());
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("starting pager {:?}", pager))?;
    let write_result = child
        .stdin
        .take()
        .expect("pager stdin was configured as piped")
        .write_all(contents);
    let status = child.wait()?;

    if let Err(error) = write_result
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(error.into());
    }
    if !status.success() {
        bail!("pager exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output_pager(path: &std::path::Path) -> String {
        format!("cat > '{}'", path.display().to_string().replace('\\', "/"))
    }

    #[test]
    fn cat_and_empty_commands_disable_paging() {
        assert_eq!(enabled_pager("cat".into()), None);
        assert_eq!(enabled_pager(OsString::new()), None);
        assert_eq!(enabled_pager("less".into()), Some("less".into()));
    }

    #[test]
    fn git_pager_precedence_matches_git() {
        let selected = select_git_pager(
            Policy::Auto,
            Some(CommandPager::Command("delta".into())),
            Some("git-pager".into()),
            Some("core-pager".into()),
            Some("pager".into()),
            Some("default".into()),
        );
        assert_eq!(selected, Some("git-pager".into()));

        let selected = select_git_pager(
            Policy::Auto,
            Some(CommandPager::Command("delta".into())),
            None,
            Some("core-pager".into()),
            Some("pager".into()),
            Some("default".into()),
        );
        assert_eq!(selected, Some("delta".into()));
    }

    #[test]
    fn compiled_default_pager_comes_from_git() {
        let dir = tempfile::tempdir().unwrap();
        let output = Command::new("git")
            .args(["var", "GIT_PAGER"])
            .current_dir(dir.path())
            .env_remove("GIT_PAGER")
            .env_remove("PAGER")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", dir.path().join("missing-config"))
            .env("GIT_DIR", dir.path().join("missing-git-dir"))
            .output()
            .unwrap();
        assert!(output.status.success());
        let expected = OsString::from(
            String::from_utf8(output.stdout)
                .unwrap()
                .trim_end_matches(['\r', '\n']),
        );

        assert_eq!(git_default_pager(), Some(expected));
    }

    #[test]
    fn compiled_default_query_isolated_from_injected_git_config() {
        let dir = tempfile::tempdir().unwrap();
        let command = git_default_pager_command(dir.path());
        let env = command
            .get_envs()
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(env.get(OsStr::new("GIT_CONFIG_COUNT")), Some(&None));
        assert_eq!(
            env.get(OsStr::new("GIT_CONFIG_GLOBAL")),
            Some(&Some(dir.path().join("config").as_os_str()))
        );
    }

    #[test]
    fn command_config_can_disable_auto_but_not_forced_paging() {
        let inputs = || {
            (
                Some(CommandPager::Disabled),
                Some(OsString::from("git-pager")),
                Some(OsString::from("core-pager")),
                Some(OsString::from("pager")),
                Some(OsString::from("default")),
            )
        };
        let (command, git, core, pager, default) = inputs();
        assert_eq!(
            select_git_pager(Policy::Auto, command, git, core, pager, default),
            None
        );

        let (command, git, core, pager, default) = inputs();
        assert_eq!(
            select_git_pager(Policy::Always, command, git, core, pager, default),
            Some("git-pager".into())
        );
    }

    #[test]
    fn pager_receives_the_complete_document() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("pager-output");
        let pager = output_pager(&output);

        write_to_pager(OsStr::new(&pager), b"first repo\nsecond repo\n").unwrap();

        assert_eq!(std::fs::read(output).unwrap(), b"first repo\nsecond repo\n");
    }

    #[test]
    fn leaving_the_pager_early_is_successful() {
        let contents = vec![b'x'; 1024 * 1024];
        write_to_pager(OsStr::new("head -c 1 >/dev/null"), &contents).unwrap();
    }

    #[test]
    fn git_for_windows_shell_is_derived_from_the_exec_path() {
        let shell =
            git_for_windows_shell(Path::new("C:/Program Files/Git/mingw64/libexec/git-core"));
        assert_eq!(
            shell,
            Some(PathBuf::from("C:/Program Files/Git/usr/bin/sh.exe"))
        );
    }

    #[test]
    fn missing_git_shell_variable_uses_the_supplied_fallback() {
        assert_eq!(
            select_git_shell(None, || OsString::from("bundled-sh")),
            OsString::from("bundled-sh")
        );
        assert_eq!(
            select_git_shell(Some(OsString::from("configured-sh")), || {
                panic!("fallback should not run")
            }),
            OsString::from("configured-sh")
        );
    }

    #[test]
    fn early_help_pages_only_when_explicitly_requested() {
        let cases = [
            (&["wsp", "--help"][..], Policy::Never),
            (&["wsp", "--paginate", "--help"][..], Policy::Always),
            (
                &["wsp", "--paginate", "--no-pager", "--help"][..],
                Policy::Never,
            ),
            (
                &["wsp", "--paginate", "--json", "--help"][..],
                Policy::Never,
            ),
            (&["wsp", "diff", "--", "--paginate"][..], Policy::Never),
        ];
        for (args, expected) in cases {
            let args: Vec<OsString> = args.iter().map(OsString::from).collect();
            assert_eq!(Policy::for_early_help(&args), expected, "{args:?}");
        }
    }

    #[test]
    fn automatic_policy_tracks_terminal_state_on_every_platform() {
        assert!(Policy::Auto.should_page(true));
        assert!(!Policy::Auto.should_page(false));
        assert!(Policy::Always.should_page(false));
        assert!(!Policy::Never.should_page(true));
    }

    #[cfg(windows)]
    #[test]
    fn windows_fallback_finds_git_for_windows_shell() {
        let shell = PathBuf::from(fallback_git_shell());
        assert!(
            shell.is_file(),
            "Git-for-Windows shell does not exist: {}",
            shell.display()
        );
        assert_eq!(
            shell.file_name().and_then(|name| name.to_str()),
            Some("sh.exe")
        );
    }
}
