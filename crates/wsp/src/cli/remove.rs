use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::gc;
use wsp_core::giturl;
use wsp_core::output::{MutationOutput, Output};
use wsp_core::workspace;

use super::completers;

pub fn cmd() -> Command {
    Command::new("rm")
        .visible_alias("remove")
        .about("Remove repo(s) from the current workspace")
        .long_about(
            "Remove repo(s) from the current workspace.\n\n\
             Runs the same safety checks as `wsp rm` (pending changes, branch merge status) \
             on each repo before removal. The repo's directory is deleted but the mirror is \
             kept. Use --force to skip safety checks.",
        )
        .arg(
            Arg::new("repos")
                .required(true)
                .num_args(1..)
                .add(ArgValueCandidates::new(
                    completers::complete_workspace_repos,
                )),
        )
        .arg(
            Arg::new("force")
                .short('f')
                .long("force")
                .action(clap::ArgAction::SetTrue)
                .help("Remove even if repos have pending changes or unmerged branches"),
        )
}

pub fn run_context(
    matches: &ArgMatches,
    context: &crate::context::InvocationContext,
) -> Result<Output> {
    let repo_args: Vec<&String> = matches.get_many::<String>("repos").unwrap().collect();
    let force = matches.get_flag("force");

    let cwd = crate::shellcd::invocation_dir()?;
    let ws_dir = workspace::detect(&cwd)?;
    gc::check_workspace(&ws_dir, /* read_only */ false)?;

    let meta = workspace::load_metadata(&ws_dir)
        .map_err(|e| anyhow::anyhow!("reading workspace: {}", e))?;

    // Resolve repo args to full identities using workspace repos
    let ws_identities: Vec<String> = meta.repos.keys().cloned().collect();

    let cfg = meta.apply_workspace_config(&context.config);

    let mut resolved = Vec::new();
    for rn in &repo_args {
        let id = giturl::resolve(rn, &ws_identities)?;
        if !resolved.contains(&id) {
            resolved.push(id);
        }
    }

    eprintln!("Removing {} repo(s) from workspace...", resolved.len());
    workspace::remove_repos_with_refresh(&ws_dir, &resolved, force, |clone_dir, identity| {
        crate::transport::refresh_clone(
            context.paths.as_ref(),
            context.allows_mirror_write(),
            &ws_dir,
            clone_dir,
            identity,
            true,
            context.direct_transport_reason(),
        )
        .map(|_| ())
    })?;

    // A directory that no longer exists is not somewhere to leave the shell.
    // Asking the filesystem beats working out which paths were removed and
    // comparing them: `ws_dir` was found by walking up from `cwd`, so the shell
    // is inside the workspace either way, and the workspace root is the nearest
    // place that certainly survived.
    if !cwd.exists() {
        crate::shellcd::request(&ws_dir);
    }

    let meta_result = workspace::load_metadata(&ws_dir);
    match &meta_result {
        Ok(meta) => wsp_core::lang::run_integrations(&ws_dir, meta, &cfg),
        Err(e) => eprintln!("warning: skipping language integrations: {}", e),
    }
    if cfg.agent_md.unwrap_or(true)
        && let Ok(meta) = &meta_result
        && let Err(e) = wsp_core::agentmd::update(&ws_dir, meta)
    {
        eprintln!("warning: AGENTS.md generation failed: {}", e);
    }

    Ok(Output::Mutation(MutationOutput::new("Done.")))
}
