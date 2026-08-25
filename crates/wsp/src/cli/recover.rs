use crate::usage::UsageExt;
use anyhow::{Result, bail};
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use crate::cli::completers;
use wsp_core::config::Paths;
use wsp_core::gc;
use wsp_core::output::{MutationOutput, Output};

pub fn cmd() -> Command {
    Command::new("recover")
        // Every successful `wsp recover` is a restore, so it always moves the
        // shell. That is load-bearing rather than incidental: mechanisms keyed
        // on the command name (this declaration, the auto-gc gate in main.rs,
        // hints) see only "recover" and cannot tell argument forms apart. When
        // the bare form listed, `recover` was read-only in one form and
        // mutating in another, and every such mechanism was silently wrong for
        // it — the auto-gc gate deleted recoverable workspaces from a read-only
        // invocation before that was fixed.
        .add(crate::shellnav::ShellNav::follows())
        .about("Restore a recently removed workspace")
        .long_about(
            "Restore a recently removed workspace.\n\n\
             Workspaces removed with `wsp rm` are held in a gc directory for 7 days \
             (configurable via gc.retention-days). Set gc.retention-days to 0 to keep \
             deleted workspaces indefinitely.\n\n\
             To see what can be restored, use `wsp ls --removed`.",
        )
        // The positional must stay optional in clap: `test_workspace_arg_is_optional`
        // in cli/mod.rs enforces that project-wide, so `run()` -- not clap --
        // has to reject the empty case. That is also the better error, since it
        // can say how many workspaces are recoverable. `usage()` keeps `--help`
        // and the generated agent docs showing the real grammar.
        .usage("wsp recover <workspace>")
        .arg(
            Arg::new("workspace")
                .help("Name of workspace to restore")
                .add(ArgValueCandidates::new(
                    completers::complete_recoverable_workspaces,
                )),
        )
}

pub fn run(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let Some(name) = matches.get_one::<String>("workspace") else {
        // Deliberately an error rather than a listing. See the note on `cmd()`:
        // a command whose bare form is read-only and whose one-argument form
        // mutates cannot be classified by any mechanism that keys on the
        // command name. The message redirects rather than dead-ends -- it says
        // whether there is anything to recover, and where to see it. Expiry
        // dates are deliberately left to `wsp ls --removed`, which shows them
        // per workspace.
        let count = gc::list(&paths.gc_dir)?.len();
        if count == 0 {
            bail!("nothing to recover: no removed workspaces are being held");
        }
        bail!(
            "`wsp recover` needs a workspace name\n\n  \
             wsp ls --removed      {} recoverable\n  \
             wsp recover <name>    restore one",
            count
        );
    };

    gc::restore(paths, name)?;
    crate::shellcd::request(&wsp_core::workspace::dir(&paths.workspaces_dir, name));
    Ok(Output::Mutation(MutationOutput::new(format!(
        "Workspace {:?} restored.",
        name
    ))))
}
