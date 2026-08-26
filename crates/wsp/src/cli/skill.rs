// ---------------------------------------------------------------------------
// SKILL.md generation (codegen only)
// ---------------------------------------------------------------------------

#[cfg(feature = "codegen")]
use anyhow::Result;

#[cfg(feature = "codegen")]
use clap::{ArgMatches, Command};

#[cfg(feature = "codegen")]
use wsp_core::config::Paths;

#[cfg(feature = "codegen")]
use wsp_core::output::Output;

#[cfg(feature = "codegen")]
pub fn generate_cmd() -> Command {
    Command::new("generate").about("Generate SKILL.md from CLI introspection (dev only)")
}

#[cfg(feature = "codegen")]
pub fn run_generate(_matches: &ArgMatches, _paths: &Paths) -> Result<Output> {
    use wsp_core::output::{
        ConfigGetOutput, ConfigListOutput, DiffOutput, ErrorOutput, ExecOutput, FetchOutput,
        ImportOutput, LogOutput, MutationOutput, RepoListOutput, StatusOutput, SyncAbortOutput,
        SyncOutput, TemplateListOutput, TemplateShowOutput, WorkspaceListOutput,
        WorkspaceRepoListOutput,
    };

    let cli = super::build_cli();
    let mut out = String::new();

    // --- Front-matter ---
    out.push_str(FRONT_MATTER);

    // --- Quick Reference: introspected from clap ---
    out.push_str("## Quick Reference\n\n");

    // Registry (global repo registry) — top-level
    out.push_str("### Registry (global repo registry)\n\n```bash\n");
    write_subcommand_section(&cli, &mut out, "registry", &["wsp", "registry"]);
    out.push_str("```\n\n");

    // Templates — top-level
    out.push_str("### Templates (shareable workspace definitions)\n\n```bash\n");
    write_subcommand_section(&cli, &mut out, "template", &["wsp", "template"]);
    out.push_str("```\n\n");

    // Workspaces — top-level workspace commands + `repo` subcommands
    out.push_str("### Workspaces\n\n```bash\n");
    let ws_cmds = [
        "new", "ls", "st", "diff", "log", "sync", "exec", "cd", "rm", "recover", "rename",
    ];
    for name in &ws_cmds {
        if let Some(sub) = cli.find_subcommand(name) {
            write_cmd_line(&mut out, &["wsp"], sub);
        }
    }
    // Workspace-scoped repo commands
    if let Some(repo) = cli.find_subcommand("repo") {
        for sub in repo.get_subcommands() {
            write_cmd_line(&mut out, &["wsp", "repo"], sub);
        }
    }
    out.push_str("```\n\n");

    // Config — top-level
    out.push_str("### Config\n\n```bash\n");
    write_subcommand_section(&cli, &mut out, "config", &["wsp", "config"]);
    out.push_str("```\n\n");

    // Doctor — top-level (no subcommands, just write the command itself)
    out.push_str("### Diagnostics\n\n```bash\n");
    if let Some(sub) = cli.find_subcommand("doctor") {
        write_cmd_line(&mut out, &["wsp"], sub);
    }
    out.push_str("```\n\n");

    // --- JSON Output Schemas ---
    out.push_str("## JSON Output Schemas\n\n");

    write_schema::<RepoListOutput>(&mut out, "wsp registry ls --json");
    write_schema::<WorkspaceListOutput>(&mut out, "wsp ls --json");
    write_sample(
        &mut out,
        "wsp ls --removed --json",
        &WorkspaceListOutput::sample_removed(),
    );
    write_schema::<StatusOutput>(&mut out, "wsp st --json");
    write_schema::<DiffOutput>(&mut out, "wsp diff --json");
    write_schema::<LogOutput>(&mut out, "wsp log --json");
    write_schema::<SyncOutput>(&mut out, "wsp sync --json");
    write_schema::<SyncAbortOutput>(&mut out, "wsp sync --abort --json");
    write_schema::<WorkspaceRepoListOutput>(&mut out, "wsp repo ls --json");
    write_schema::<ExecOutput>(&mut out, "wsp exec <workspace> --json -- <command>");
    write_schema::<FetchOutput>(&mut out, "wsp repo fetch --json");
    write_schema::<TemplateListOutput>(&mut out, "wsp template ls --json");
    write_schema::<TemplateShowOutput>(&mut out, "wsp template show <name> --json");
    write_schema::<ConfigListOutput>(&mut out, "wsp config ls --json");
    write_schema::<ConfigGetOutput>(&mut out, "wsp config get <key> --json");
    write_schema::<MutationOutput>(
        &mut out,
        "Mutation commands (new, rm, add, remove, set, etc.)",
    );
    write_schema::<ImportOutput>(&mut out, "wsp registry add --from <org> --all --json");
    write_schema::<wsp_core::output::SetupCommandsOutput>(
        &mut out,
        "wsp repo setup-commands --json",
    );
    write_schema::<crate::cli::help::HelpTopicListOutput>(&mut out, "wsp help --json");
    write_schema::<crate::cli::help::HelpTopicOutput>(&mut out, "wsp help <topic> --json");
    write_schema::<wsp_core::output::DoctorOutput>(&mut out, "wsp doctor --json");
    write_schema::<ErrorOutput>(&mut out, "Errors");

    // --- Static reference sections ---
    out.push_str(REFERENCE_SECTIONS);

    print!("{}", out);
    Ok(Output::None)
}

#[cfg(feature = "codegen")]
trait Sample {
    fn sample() -> Self;
}

#[cfg(feature = "codegen")]
macro_rules! impl_sample {
    ($($t:ty),+ $(,)?) => {
        $(impl Sample for $t {
            fn sample() -> Self { <$t>::sample() }
        })+
    };
}

#[cfg(feature = "codegen")]
impl_sample!(
    wsp_core::output::RepoListOutput,
    wsp_core::output::TemplateListOutput,
    wsp_core::output::TemplateShowOutput,
    wsp_core::output::WorkspaceListOutput,
    wsp_core::output::StatusOutput,
    wsp_core::output::DiffOutput,
    wsp_core::output::LogOutput,
    wsp_core::output::SyncOutput,
    wsp_core::output::SyncAbortOutput,
    wsp_core::output::ConfigListOutput,
    wsp_core::output::ConfigGetOutput,
    wsp_core::output::WorkspaceRepoListOutput,
    wsp_core::output::ExecOutput,
    wsp_core::output::FetchOutput,
    wsp_core::output::MutationOutput,
    wsp_core::output::ImportOutput,
    wsp_core::output::SetupCommandsOutput,
    crate::cli::help::HelpTopicListOutput,
    crate::cli::help::HelpTopicOutput,
    wsp_core::output::DoctorOutput,
    wsp_core::output::ErrorOutput,
);

#[cfg(feature = "codegen")]
fn write_schema<T: Sample + serde::Serialize>(out: &mut String, heading: &str) {
    write_sample(out, heading, &T::sample());
}

/// For the second shape of a type that has more than one -- `wsp ls --removed`
/// against `wsp ls`. `write_schema` covers the one-sample-per-type case.
#[cfg(feature = "codegen")]
fn write_sample<T: serde::Serialize>(out: &mut String, heading: &str, sample: &T) {
    use std::fmt::Write;
    let json = serde_json::to_string_pretty(sample).expect("sample serialization");
    // The type is recorded so a section can be checked against the struct it
    // came from, rather than against every field name in the file. An HTML
    // comment renders as nothing, and `type_name` cannot drift from the type.
    let ty = std::any::type_name::<T>()
        .rsplit("::")
        .next()
        .unwrap_or("unknown");
    writeln!(
        out,
        "### `{}`\n<!-- type: {} -->\n```json\n{}\n```\n",
        heading, ty, json
    )
    .unwrap();
}

#[cfg(feature = "codegen")]
fn write_cmd_line(out: &mut String, prefix: &[&str], cmd: &Command) {
    use std::fmt::Write;

    let name = cmd.get_name();
    let about = cmd.get_about().map(|a| a.to_string()).unwrap_or_default();

    // A command sets `override_usage` because the derived signature is wrong:
    // `wsp recover` keeps an optional positional so `run()` can answer with a
    // better error than clap's, and `wsp rename` reads `[old] <new>`. Prefer
    // the override -- it is what `--help` prints, so the two cannot disagree.
    // Multi-line overrides (`describe`) do not fit a one-line table, so those
    // fall through to the derived form.
    if let Some(explicit) = cmd
        .get::<crate::usage::Usage>()
        .map(|u| u.0)
        .filter(|u| !u.contains('\n'))
    {
        writeln!(out, "{:<47} # {}", explicit, about).unwrap();
        return;
    }

    // Build the usage string: prefix + name + args + flags
    let mut usage = prefix.join(" ");
    write!(usage, " {}", name).unwrap();

    for arg in cmd.get_arguments() {
        let id = arg.get_id().as_str();
        if id == "json" || id == "help" || id == "version" {
            continue;
        }
        if arg.is_positional() {
            if arg.is_required_set() {
                write!(usage, " <{}>", id).unwrap();
            } else {
                write!(usage, " [<{}>]", id).unwrap();
            }
            if let Some(num_args) = arg.get_num_args()
                && num_args.max_values() > 1
            {
                usage.push_str("...");
            }
        } else {
            // Named flags/options
            let long = arg
                .get_long()
                .map(|l| format!("--{}", l))
                .unwrap_or_default();
            let short = arg.get_short().map(|s| format!("-{}", s));
            let flag_name = short.unwrap_or(long);
            if flag_name.is_empty() {
                continue;
            }
            if arg.get_action().takes_values() {
                write!(usage, " [{} <{}>]", flag_name, id).unwrap();
                if let Some(num_args) = arg.get_num_args()
                    && num_args.max_values() > 1
                {
                    usage.push_str("...");
                }
            } else {
                write!(usage, " [{}]", flag_name).unwrap();
            }
        }
    }

    // Visible aliases
    let aliases: Vec<&str> = cmd.get_visible_aliases().collect();
    let alias_suffix = if aliases.is_empty() {
        String::new()
    } else {
        format!(" (alias: {})", aliases.join(", "))
    };

    let pad = 48usize.saturating_sub(usage.len()).max(1);
    writeln!(
        out,
        "{}{}# {}{}",
        usage,
        " ".repeat(pad),
        about,
        alias_suffix
    )
    .unwrap();
}

/// Write all subcommands of a top-level noun.
#[cfg(feature = "codegen")]
fn write_subcommand_section(cli: &Command, out: &mut String, noun: &str, prefix: &[&str]) {
    if let Some(parent) = cli.find_subcommand(noun) {
        for sub in parent.get_subcommands() {
            write_cmd_line(out, prefix, sub);
        }
    }
}

// ---------------------------------------------------------------------------
// Static prose sections
// ---------------------------------------------------------------------------

#[cfg(feature = "codegen")]
const FRONT_MATTER: &str = r#"---
name: wsp-manage
description: Manage multi-repo workspaces with wsp
user_invocable: true
---

# wsp — Multi-Repo Workspace Manager

Use `wsp` to manage workspaces that span multiple git repositories. Each workspace creates local clones from bare mirror clones, sharing a single branch name across repos.

**Always use `--json` when calling wsp programmatically.** JSON output goes to stdout; progress messages go to stderr.

"#;

#[cfg(feature = "codegen")]
const REFERENCE_SECTIONS: &str = r#"## Shortname Resolution

Repos are identified by `host/owner/repo` (e.g., `github.com/acme/api-gateway`). You can use the shortest unique suffix:
- `api-gateway` if unambiguous
- `acme/api-gateway` to disambiguate from `other-org/api-gateway`

## Directory Layout

```
~/dev/workspaces/<workspace-name>/
  .wsp.yaml              # Workspace metadata
  <repo-name>/          # Local clone for each repo
```

## Common Agent Workflows

### Create a workspace and start working
```bash
wsp registry ls --json                         # See available repos
wsp new my-feature api-gateway user-service    # Create workspace
cd ~/dev/workspaces/my-feature                # Enter workspace
```

### Check what's changed
```bash
wsp st --json          # From inside a workspace
wsp diff --json        # See all diffs
```

### Sync with upstream
```bash
wsp sync --json        # Fetch + rebase all repos
wsp sync --strategy merge --json  # Use merge instead of rebase
```

### Run tests across all repos
```bash
wsp exec my-feature -- make test
```

### Clean up when done
```bash
wsp rm my-feature      # Removes clones + branch (if merged)
wsp rm my-feature -f   # Force remove even if unmerged
```
"#;
