use std::fs;

use anyhow::Result;
use clap::{ArgMatches, Command};

use crate::config::Paths;
use crate::output::{MutationOutput, Output};

const SKILL_CONTENT: &str = include_str!("../../skills/ws-manage/SKILL.md");

pub fn install_cmd() -> Command {
    Command::new("install").about("Install ws Claude Code skill to ~/.claude/skills/")
}

pub fn run_install(_matches: &ArgMatches, _paths: &Paths) -> Result<Output> {
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    let skill_dir = home.join(".claude").join("skills").join("ws-manage");
    fs::create_dir_all(&skill_dir)?;

    let skill_path = skill_dir.join("SKILL.md");
    fs::write(&skill_path, SKILL_CONTENT)?;

    Ok(Output::Mutation(MutationOutput {
        ok: true,
        message: format!("Installed skill to {}", skill_path.display()),
    }))
}
