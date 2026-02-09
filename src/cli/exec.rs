use std::path::Path;
use std::process::Command as ProcessCommand;

use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::config::Paths;
use crate::giturl;
use crate::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("exec")
        .about("Run a command in each repo of a workspace")
        .arg(
            Arg::new("workspace")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
        .arg(Arg::new("command").required(true).num_args(1..).last(true))
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<()> {
    let ws_name = matches.get_one::<String>("workspace").unwrap();
    let command: Vec<&String> = matches.get_many::<String>("command").unwrap().collect();

    let ws_dir = workspace::dir(&paths.workspaces_dir, ws_name);
    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    let mut failed = 0;
    for identity in meta.repos.keys() {
        let parsed = match giturl::Parsed::from_identity(identity) {
            Ok(p) => p,
            Err(e) => {
                println!("[{}] error: {}", identity, e);
                failed += 1;
                continue;
            }
        };

        let repo_dir = ws_dir.join(&parsed.repo);
        let cmd_str = command
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        println!("==> [{}] {}", parsed.repo, cmd_str);

        match run_command(&command, &repo_dir) {
            Ok(None) => {}
            Ok(Some(code)) => {
                println!("[{}] error: exit status {}", parsed.repo, code);
                failed += 1;
            }
            Err(e) => {
                println!("[{}] error: {}", parsed.repo, e);
                failed += 1;
            }
        }
        println!();
    }

    if failed > 0 {
        bail!("{} command(s) failed", failed);
    }
    Ok(())
}

fn run_command(command: &[&String], dir: &Path) -> Result<Option<i32>> {
    let mut cmd = ProcessCommand::new(command[0].as_str());
    for arg in &command[1..] {
        cmd.arg(arg.as_str());
    }
    cmd.current_dir(dir);
    cmd.stdin(std::process::Stdio::inherit());
    cmd.stdout(std::process::Stdio::inherit());
    cmd.stderr(std::process::Stdio::inherit());

    let status = cmd.status()?;
    if status.success() {
        Ok(None)
    } else {
        Ok(status.code())
    }
}
