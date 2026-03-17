use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};

use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::Paths;
use wsp_core::output::{ExecOutput, ExecRepoResult, Output};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("exec")
        .about("Run a command in each repo of a workspace")
        .long_about(
            "Run a command in each repo of a workspace.\n\n\
             Executes the given command sequentially in each repo directory. The command and \
             its arguments follow `--` (e.g., `wsp exec my-ws -- make test`). Exit codes \
             are collected per repo and reported in the output.\n\n\
             The workspace name is optional when running from inside a workspace directory.",
        )
        .arg(
            Arg::new("workspace")
                .required(false)
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
        .arg(Arg::new("command").required(true).num_args(1..).last(true))
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let command: Vec<&String> = matches.get_many::<String>("command").unwrap().collect();
    let is_json = matches.get_flag("json");

    let ws_dir: PathBuf = if let Some(name) = matches.get_one::<String>("workspace") {
        workspace::dir(&paths.workspaces_dir, name)
    } else {
        let cwd = std::env::current_dir()?;
        workspace::detect(&cwd)?
    };
    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    let mut results = Vec::new();

    for identity in meta.repos.keys() {
        let dir_name = match meta.dir_name(identity) {
            Ok(d) => d,
            Err(e) => {
                if !is_json {
                    eprintln!("[{}] error: {}", identity, e);
                }
                results.push(ExecRepoResult {
                    identity: identity.to_string(),
                    shortname: identity.rsplit('/').next().unwrap_or(identity).to_string(),
                    path: String::new(),
                    directory: String::new(),
                    exit_code: -1,
                    ok: false,
                    stdout: None,
                    stderr: None,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };

        let repo_dir = ws_dir.join(&dir_name);
        let cmd_str = command
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        if !is_json {
            println!("==> [{}] {}", dir_name, cmd_str);
        }

        match run_command(&command, &repo_dir, is_json, identity, &dir_name) {
            Ok(result) => {
                if !is_json && !result.ok {
                    eprintln!("[{}] error: exit status {}", dir_name, result.exit_code);
                }
                results.push(result);
            }
            Err(e) => {
                if !is_json {
                    eprintln!("[{}] error: {}", dir_name, e);
                }
                results.push(ExecRepoResult {
                    identity: identity.to_string(),
                    shortname: dir_name.clone(),
                    path: repo_dir.to_string_lossy().to_string(),
                    directory: dir_name,
                    exit_code: -1,
                    ok: false,
                    stdout: None,
                    stderr: None,
                    error: Some(e.to_string()),
                });
            }
        }

        if !is_json {
            println!();
        }
    }

    Ok(Output::Exec(ExecOutput {
        workspace: meta.name,
        repos: results,
    }))
}

fn run_command(
    command: &[&String],
    dir: &Path,
    capture: bool,
    identity: &str,
    dir_name: &str,
) -> Result<ExecRepoResult> {
    debug_assert!(
        !command.is_empty(),
        "command must have at least one element"
    );
    let mut cmd = ProcessCommand::new(command[0].as_str());
    for arg in &command[1..] {
        cmd.arg(arg.as_str());
    }
    cmd.current_dir(dir);
    // In capture mode (--json), use null stdin so subprocesses that read stdin
    // get immediate EOF instead of hanging in automated/agent pipelines.
    cmd.stdin(if capture {
        Stdio::null()
    } else {
        Stdio::inherit()
    });

    if capture {
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let output = cmd.spawn()?.wait_with_output()?;
        let code = output.status.code().unwrap_or(-1);
        Ok(ExecRepoResult {
            identity: identity.to_string(),
            shortname: dir_name.to_string(),
            path: dir.to_string_lossy().to_string(),
            directory: dir_name.to_string(),
            exit_code: code,
            ok: code == 0,
            stdout: Some(String::from_utf8_lossy(&output.stdout).into_owned()),
            stderr: Some(String::from_utf8_lossy(&output.stderr).into_owned()),
            error: None,
        })
    } else {
        cmd.stdout(Stdio::inherit());
        cmd.stderr(Stdio::inherit());

        let status = cmd.status()?;
        let code = status.code().unwrap_or(-1);
        Ok(ExecRepoResult {
            identity: identity.to_string(),
            shortname: dir_name.to_string(),
            path: dir.to_string_lossy().to_string(),
            directory: dir_name.to_string(),
            exit_code: code,
            ok: code == 0,
            stdout: None,
            stderr: None,
            error: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_with_workspace() {
        let m = cmd().get_matches_from(["exec", "my-ws", "--", "echo", "hello"]);
        assert_eq!(
            m.get_one::<String>("workspace").map(|s| s.as_str()),
            Some("my-ws")
        );
        let command: Vec<&str> = m
            .get_many::<String>("command")
            .unwrap()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(command, vec!["echo", "hello"]);
    }

    #[test]
    fn parse_args_without_workspace() {
        let m = cmd().get_matches_from(["exec", "--", "make", "test"]);
        assert!(m.get_one::<String>("workspace").is_none());
        let command: Vec<&str> = m
            .get_many::<String>("command")
            .unwrap()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(command, vec!["make", "test"]);
    }
}
