use clap::{Arg, Command};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use serde::Serialize;

use wsp_core::output::Output;

/// Built-in help topics. Each is (name, short description, full text).
const TOPICS: &[(&str, &str, &str)] = &[
    (
        "gc",
        "Garbage collection and workspace recovery",
        "\
gc — garbage collection and workspace recovery

When you remove a workspace with `wsp rm`, it isn't permanently deleted.
Instead, it's moved to a gc (garbage collection) directory and held there
for a configurable retention period. This gives you a safety net to recover
workspaces you removed by mistake.

HOW IT WORKS

  `wsp rm <name>`       Moves workspace to gc area (soft delete)

  GC runs automatically (at most once per hour) and purges entries older
  than the retention period. The retention check happens at purge time,
  so changing the setting protects all existing entries immediately.

CONFIGURATION

  gc.retention-days     How long deleted workspaces are kept (default: 7).
                        Set to 0 to disable gc entirely — workspaces are
                        kept indefinitely until you manually delete them.

  wsp config set gc.retention-days 30    # keep for 30 days
  wsp config set gc.retention-days 0     # never auto-purge

RECOVERY

  wsp recover           List all recoverable workspaces
  wsp recover show <n>  Inspect a deleted workspace (repos, size, path)
  wsp recover <name>    Restore workspace to its original location

  Only the most recent deletion of a given name is restored.

STORAGE

  GC'd workspaces live in ~/.local/share/wsp/gc/<name>__<timestamp>/
  with a .wsp-gc.yaml metadata file inside. The original .wsp.yaml and
  all repo clones are preserved intact.

EMERGENCY: PREVENTING DATA LOSS

  If you accidentally removed a workspace and are worried gc might purge
  it before you can recover, you have two options:

  1. Recover immediately:  wsp recover <name>
  2. Disable gc:           wsp config set gc.retention-days 0

  Option 2 prevents ALL future purges until you re-enable gc. Existing
  entries are safe because gc checks retention at purge time, not at
  deletion time.
",
    ),
    (
        "wspignore",
        "Suppress files from workspace root checks",
        "\
wspignore — suppress files from workspace root checks

`wsp st` checks the workspace root directory for files that aren't managed
by wsp (repos, .wsp.yaml, etc.). OS noise, editor configs, and other
harmless files can be suppressed with wspignore patterns.

PATTERN SYNTAX

  One pattern per line. Blank lines and lines starting with # are ignored.

  .DS_Store         exact filename match
  .claude/          trailing / matches a directory and everything inside it

IGNORE FILES

  There are two wspignore files, checked in order:

  1. Global:        ~/.local/share/wsp/wspignore
                    Created automatically on first use with sensible defaults
                    (.DS_Store, Thumbs.db, etc.). Edit to add patterns that
                    apply to all workspaces.

  2. Per-workspace: <workspace-root>/.wspignore
                    Patterns specific to a single workspace.

  Patterns from both files are merged. A match in either file suppresses
  the path.

EXAMPLES

  # Suppress all Claude Code local settings
  .claude/settings.local.json

  # Suppress an entire directory
  .idea/

  # Suppress a one-off file in this workspace
  echo 'notes.md' >> .wspignore
",
    ),
    (
        "config",
        "Configuration keys and their effects",
        "\
config — configuration keys and their effects

Settings are stored at two levels:

  Global:     ~/.local/share/wsp/config.yaml
  Workspace:  .wsp.yaml `config` field (per-workspace overrides)

When run inside a workspace, `wsp config set/get/unset/ls` operate on
workspace config by default. Use --global to target global config instead.
Outside a workspace, commands always use global config.

Workspace-scoped keys: sync-strategy, git.*, lang.*
Global-only keys: branch-prefix, workspaces-dir, gc.retention-days, agent-md,
                  hints, advice.*, shell.tmux, shell.prompt

Config hierarchy (top wins): workspace → global → built-in defaults.

GENERAL

  branch-prefix         String. Prefix prepended to workspace branch names.
                        Example: `jganoff` → branch `jganoff/my-feature`.
                        Default: not set (branches are just the workspace name).

  workspaces-dir        Absolute path. Where workspaces are created.
                        Default: ~/dev/workspaces

  sync-strategy         `rebase` or `merge`. How `wsp sync` integrates upstream.
                        Default: rebase

  agent-md              Boolean. Generate AGENTS.md (+ CLAUDE.md symlink) in
                        workspace roots. Provides context for AI agents.
                        Default: true

GC (GARBAGE COLLECTION)

  gc.retention-days     Integer (≥0). How many days `wsp rm` keeps deleted
                        workspaces recoverable via `wsp recover`.
                        Set to 0 to disable gc (keep indefinitely).
                        Default: 7

SHELL (experimental)

  shell.prompt          Boolean. Emit a shell hook that sets the WSP_WORKSPACE
                        environment variable to the current workspace name.
                        Use in your prompt: PS1='${WSP_WORKSPACE:+[wsp:$WSP_WORKSPACE] }%~ $ '
                        Requires re-sourcing: eval \"$(wsp completion zsh)\"
                        Default: false

  shell.tmux            String. Controls tmux integration in shell hooks.
                        Values: window-title, false
                        window-title: sets the tmux window name to `wsp:<workspace>`
                          when inside a workspace. Restores automatic-rename when
                          outside. Requires tmux on PATH and $TMUX to be set.
                        false: disabled (default)
                        Requires re-sourcing: eval \"$(wsp completion zsh)\"

GIT CONFIG

  git.<key>             Override git config applied to every clone. The key
                        is any valid git config key (e.g., push.default).
                        These merge with built-in defaults:

                          push.autoSetupRemote  true
                          push.default          current
                          rerere.enabled        true
                          branch.sort           -committerdate

                        Example: `wsp config set git.merge.conflictstyle zdiff3`
                        Unset reverts to the built-in default (if any).

LANGUAGE INTEGRATIONS

  lang.<name>           Boolean. Enable/disable per-language workspace support.
                        Available: go (generates go.work for multi-module repos).
                        Default: false

HINTS

  hints                 Boolean. Master switch for all contextual hints.
                        Set to false to silence all hints globally.
                        Default: true

  advice.<key>          Boolean. Suppress a specific hint by key.
                        Keys: branchPrefix, setupCommands, registrySetupCommands
                        Example: `wsp config set advice.branchPrefix false`
                        Default: true (hint enabled)

EXAMPLES

  wsp config ls                                   # show all settings (workspace-aware)
  wsp config set sync-strategy merge              # set in workspace (if inside one)
  wsp config set --global sync-strategy merge     # set in global config
  wsp config set branch-prefix jganoff            # global-only key (always global)
  wsp config set gc.retention-days 30             # keep deleted workspaces 30 days
  wsp config set git.merge.conflictstyle zdiff3         # workspace or global
  wsp config set shell.prompt true                      # enable prompt variable (global)
  wsp config unset sync-strategy                  # unset workspace override
  wsp config unset --global branch-prefix         # revert global to default
",
    ),
    (
        "wsp.yaml",
        "Setup commands: layers, approval, and management",
        "\
wsp.yaml — setup commands

Setup commands can be declared at four layers. When wsp clones a repo, all
layers are gathered and concatenated in order. The merged list is what the
user approves; its hash determines whether re-approval is needed.

LAYERS (in resolution order)

  1. Registry     Per-repo commands in ~/.local/share/wsp/config.yaml
  2. Template     Per-repo commands on a TemplateRepo
  3. Repo         Committed setup_commands in the repo's own .wsp.yaml
  4. Workspace    Per-repo overrides in workspace .wsp.yaml metadata

  Commands are not deduplicated at resolution time — setup commands are
  expected to be idempotent. Use `wsp repo setup --all` to run every
  occurrence; by default, exact duplicates are removed before prompting
  (first occurrence wins).

DECLARING SETUP COMMANDS

  Repo .wsp.yaml (committed to git):

    setup_commands:
      - task setup
      - lefthook install

  Registry (personal, not committed):

    wsp repo setup-commands add <repo> 'npm install' --registry

  Template (per-repo):

    wsp template setup-commands add <template> <repo> 'make deps'

  Workspace (local override):

    wsp repo setup-commands add <repo> './extra.sh' --workspace

SCAFFOLDING

  wsp init                     Interactive wizard: creates or updates .wsp.yaml
                               in the current git repo.
  wsp init --print-sample      Print a commented sample .wsp.yaml to stdout.

APPROVAL FLOW

  wsp prompts before running (like direnv):

    Setup commands for github.com/acme/api-gateway:
      lefthook install
      task setup
      ./extra.sh
    Run these commands? [y/N]

  y        Allow and run. Future runs skip the prompt automatically.
  N/Enter  Skip.

  Approvals are stored by SHA-256 hash of the merged command list in
  ~/.local/share/wsp/approvals.yaml. Any change at any layer triggers a
  fresh prompt.

MANAGING SETUP COMMANDS

  wsp repo setup-commands ls    <repo>            List merged commands with provenance.
  wsp repo setup-commands add   <repo> <cmd>      Add (default scope: workspace or registry).
  wsp repo setup-commands rm    <repo> <cmd>      Remove from a scope.
  wsp repo setup-commands clear <repo>            Clear all commands at a scope.

  Scope flags: --registry, --workspace, --repo. Default: --repo if CWD is
  inside a repo clone, --workspace if inside a workspace, --registry
  otherwise.

    --registry   Personal override; runs in every workspace for this repo.
    --workspace  Local to the current workspace only.
    --repo       Writes to the repo's committed .wsp.yaml (all clones).

  For templates:

  wsp template setup-commands ls    <tmpl> <repo>        List.
  wsp template setup-commands add   <tmpl> <repo> <cmd>  Add.
  wsp template setup-commands rm    <tmpl> <repo> <cmd>  Remove.
  wsp template setup-commands clear <tmpl> <repo>        Clear.

RUNNING SETUP MANUALLY

  wsp repo setup               Run setup commands for all repos in the current
                               workspace (prompts for any unapproved).
  wsp repo setup <repo>        Run for a specific repo.
  wsp repo setup --force       Re-prompt even if previously approved.

  wsp doctor (W15) warns when a repo has unapproved setup commands.
  wsp doctor --fix invokes the interactive approval flow.
",
    ),
];

#[derive(Serialize)]
struct HelpTopicOutput {
    name: String,
    summary: String,
    text: String,
}

#[derive(Serialize)]
struct HelpTopicListOutput {
    topics: Vec<HelpTopicSummary>,
}

#[derive(Serialize)]
struct HelpTopicSummary {
    name: String,
    summary: String,
}

fn complete_help_topics() -> Vec<CompletionCandidate> {
    let mut candidates: Vec<CompletionCandidate> = TOPICS
        .iter()
        .map(|(name, summary, _)| CompletionCandidate::new(*name).help(Some((*summary).into())))
        .collect();

    // Also complete subcommand names (wsp help <command> shows --help)
    let cli = super::build_cli();
    for sub in cli.get_subcommands() {
        let name = sub.get_name();
        if name == "help" || sub.is_hide_set() {
            continue;
        }
        let about = sub.get_about().map(|a| a.to_string()).unwrap_or_default();
        candidates.push(CompletionCandidate::new(name).help(Some(about.into())));
    }

    candidates
}

pub fn cmd() -> Command {
    Command::new("help")
        .about("Display help for a command or topic [read-only]")
        .long_about(
            "Display help for a command or topic.\n\n\
             Without arguments, shows the top-level help. With a topic argument, shows \
             detailed documentation for that topic. Use `wsp help -g` to list \
             available guides.",
        )
        .arg(
            Arg::new("topic")
                .help("Command name or help topic")
                .add(ArgValueCandidates::new(complete_help_topics)),
        )
        .arg(
            Arg::new("guides")
                .short('g')
                .long("guides")
                .action(clap::ArgAction::SetTrue)
                .help("List available concept guides"),
        )
}

pub fn run(matches: &clap::ArgMatches, cli: &mut Command, json: bool) -> anyhow::Result<Output> {
    if matches.get_flag("guides") {
        if json {
            let out = HelpTopicListOutput {
                topics: TOPICS
                    .iter()
                    .map(|(name, desc, _)| HelpTopicSummary {
                        name: name.to_string(),
                        summary: desc.to_string(),
                    })
                    .collect(),
            };
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            println!("Available guides:\n");
            for (name, desc, _) in TOPICS {
                println!("  {:16}{}", name, desc);
            }
            println!("\nUse `wsp help <guide>` for details.");
        }
        return Ok(Output::None);
    }

    let topic = match matches.get_one::<String>("topic") {
        Some(t) => t,
        None => {
            cli.print_long_help()?;
            eprintln!(
                "\n'wsp help -g' lists available concept guides.\n\
                 See 'wsp help <command>' or 'wsp help <guide>' for details."
            );
            return Ok(Output::None);
        }
    };

    // Check built-in topics first
    for (name, summary, text) in TOPICS {
        if *name == topic.as_str() {
            if json {
                let out = HelpTopicOutput {
                    name: name.to_string(),
                    summary: summary.to_string(),
                    text: text.to_string(),
                };
                println!("{}", serde_json::to_string_pretty(&out)?);
            } else {
                print!("{}", text);
            }
            return Ok(Output::None);
        }
    }

    // Fall back to subcommand --help (text only — clap doesn't support JSON help)
    if let Some(mut sub) = cli.find_subcommand(topic).cloned() {
        sub.print_long_help()?;
        return Ok(Output::None);
    }

    anyhow::bail!(
        "no help topic or command named {:?}. Use `wsp help -g` to list guides.",
        topic
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_topics_have_content() {
        for (name, summary, text) in TOPICS {
            assert!(!name.is_empty(), "topic name must not be empty");
            assert!(!summary.is_empty(), "topic {:?} has empty summary", name);
            assert!(!text.is_empty(), "topic {:?} has empty text", name);
        }
    }

    #[test]
    fn test_topic_lookup() {
        let found = TOPICS.iter().find(|(name, _, _)| *name == "wspignore");
        assert!(found.is_some(), "wspignore topic should exist");
        let (_, _, text) = found.unwrap();
        assert!(text.contains("PATTERN SYNTAX"));
        assert!(text.contains("IGNORE FILES"));
        assert!(text.contains("EXAMPLES"));
    }

    #[test]
    fn test_topic_not_found() {
        let found = TOPICS.iter().find(|(name, _, _)| *name == "nonexistent");
        assert!(found.is_none());
    }

    #[test]
    fn test_help_topic_json_serialization() {
        let out = HelpTopicOutput {
            name: "test".to_string(),
            summary: "A test topic".to_string(),
            text: "Full text here.".to_string(),
        };
        let json = serde_json::to_string_pretty(&out).unwrap();
        assert!(json.contains("\"name\": \"test\""));
        assert!(json.contains("\"summary\": \"A test topic\""));
        assert!(json.contains("\"text\": \"Full text here.\""));
    }

    #[test]
    fn test_help_topic_list_json_serialization() {
        let out = HelpTopicListOutput {
            topics: vec![HelpTopicSummary {
                name: "foo".to_string(),
                summary: "bar".to_string(),
            }],
        };
        let json = serde_json::to_string_pretty(&out).unwrap();
        assert!(json.contains("\"name\": \"foo\""));
    }
}
