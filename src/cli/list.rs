use anyhow::Result;
use clap::{ArgMatches, Command};

use crate::config::Paths;
use crate::output;
use crate::workspace;

pub fn cmd() -> Command {
    Command::new("list").about("List active workspaces")
}

pub fn run(_matches: &ArgMatches, paths: &Paths) -> Result<()> {
    let names = workspace::list_all(&paths.workspaces_dir)?;

    if names.is_empty() {
        println!("No workspaces.");
        return Ok(());
    }

    let mut table = output::Table::new(
        Box::new(std::io::stdout()),
        vec![
            "Name".to_string(),
            "Branch".to_string(),
            "Repos".to_string(),
            "Path".to_string(),
        ],
    );

    for name in &names {
        let ws_dir = workspace::dir(&paths.workspaces_dir, name);
        let meta = match workspace::load_metadata(&ws_dir) {
            Ok(m) => m,
            Err(_) => {
                let _ = table.add_row(vec![
                    name.clone(),
                    "ERROR".to_string(),
                    "?".to_string(),
                    ws_dir.display().to_string(),
                ]);
                continue;
            }
        };
        let _ = table.add_row(vec![
            name.clone(),
            meta.branch,
            meta.repos.len().to_string(),
            ws_dir.display().to_string(),
        ]);
    }

    table.render()
}
