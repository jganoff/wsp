use std::fs;
use std::io::{Read as _, Write};

use anyhow::Result;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;

use wsp_core::config::{Config, Paths};
use wsp_core::filelock;
use wsp_core::giturl;
use wsp_core::output::{
    ConfigGetOutput, MutationOutput, Output, TemplateListEntry, TemplateListOutput,
    TemplateShowOutput,
};
use wsp_core::template as tmpl;

use super::completers;

pub fn cmd() -> Command {
    Command::new("template")
        .about("Manage workspace templates")
        .long_about(
            "Manage workspace templates.\n\n\
             Templates define reusable workspace configurations: a set of repos, optional \
             config overrides, and optional AGENTS.md content for AI coding assistants. \
             Create workspaces from templates with `wsp new -t <name>`.",
        )
        .subcommand(new_cmd())
        .subcommand(import_cmd())
        .subcommand(list_cmd())
        .subcommand(show_cmd())
        .subcommand(rm_cmd())
        .subcommand(rename_cmd())
        .subcommand(export_cmd())
        .subcommand(repo_cmd())
        .subcommand(config_cmd())
        .subcommand(agent_md_cmd())
        .subcommand(setup_commands_cmd())
}

pub fn dispatch(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    match matches.subcommand() {
        Some(("new", m)) => run_new(m, paths),
        Some(("import", m)) => run_import(m, paths),
        Some(("ls", m)) => run_list(m, paths),
        Some(("show", m)) => run_show(m, paths),
        Some(("rm", m)) => run_rm(m, paths),
        Some(("rename", m)) => run_rename(m, paths),
        Some(("export", m)) => run_export(m, paths),
        Some(("repo", m)) => dispatch_repo(m, paths),
        Some(("config", m)) => dispatch_config(m, paths),
        Some(("agent-md", m)) => dispatch_agent_md(m, paths),
        Some(("setup-commands", m)) => dispatch_setup_commands(m, paths),
        None => run_list(matches, paths),
        _ => unreachable!(),
    }
}

fn new_cmd() -> Command {
    Command::new("new")
        .about("Create a new template")
        .arg(Arg::new("name").required(true))
        .arg(
            Arg::new("repos")
                .num_args(1..)
                .help("Repo URLs or shortnames for the template")
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
        .arg(
            Arg::new("from-workspace")
                .short('w')
                .long("workspace")
                .help("Create from an existing workspace")
                .add(ArgValueCandidates::new(completers::complete_workspaces)),
        )
        .arg(
            Arg::new("file")
                .short('f')
                .long("file")
                .help("Create from a template file (.yaml)")
                .value_hint(clap::ValueHint::FilePath),
        )
        .arg(
            Arg::new("description")
                .short('d')
                .long("description")
                .help("Human-readable description of the template"),
        )
        .group(
            clap::ArgGroup::new("source")
                .args(["repos", "from-workspace", "file"])
                .required(true),
        )
}

fn import_cmd() -> Command {
    Command::new("import")
        .about("Import a template from a .wsp.yaml file")
        .long_about(
            "Import a template from a .wsp.yaml file.\n\n\
             Saves the template to the local template store so it can be used with \
             `wsp new -t <name>`. The template name is derived from --name, the file's \
             `name` field, or the filename stem, in that order.",
        )
        .arg(
            Arg::new("file")
                .required(true)
                .help("Path to a .wsp.yaml or template file")
                .value_hint(clap::ValueHint::FilePath),
        )
        .arg(
            Arg::new("name")
                .long("name")
                .help("Override the template name"),
        )
        .arg(
            Arg::new("update")
                .long("update")
                .action(clap::ArgAction::SetTrue)
                .help("Re-import (overwrite if source path matches)"),
        )
        .arg(
            Arg::new("force")
                .long("force")
                .action(clap::ArgAction::SetTrue)
                .help("Overwrite existing template regardless of source"),
        )
}

fn run_import(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let file_arg = matches.get_one::<String>("file").unwrap();
    let name_override = matches.get_one::<String>("name");
    let update = matches.get_flag("update");
    let force = matches.get_flag("force");

    let file_path = std::path::Path::new(file_arg)
        .canonicalize()
        .map_err(|e| anyhow::anyhow!("cannot resolve path {:?}: {}", file_arg, e))?;

    let template = tmpl::load_from_file(&file_path)?;

    // Derive name: --name flag > YAML name field > filename stem
    let name = if let Some(n) = name_override {
        n.clone()
    } else {
        tmpl::derive_name_from_file(&file_path, &template)
    };
    tmpl::validate_name(&name)?;

    let source_str = file_path.to_string_lossy().to_string();

    // Check for conflicts
    if tmpl::exists(&paths.templates_dir, &name) && !update && !force {
        anyhow::bail!(
            "template {:?} already exists (use --update to replace, or --name for a different name)",
            name
        );
    }

    if update
        && !force
        && tmpl::exists(&paths.templates_dir, &name)
        && let Ok(Some(existing_source)) = tmpl::load_source(&paths.templates_dir, &name)
        && existing_source.source_path != source_str
    {
        anyhow::bail!(
            "template {:?} was imported from a different source (use --force to overwrite)",
            name
        );
    }

    tmpl::save(&paths.templates_dir, &name, &template)?;
    tmpl::save_source(
        &paths.templates_dir,
        &name,
        &tmpl::ImportSource {
            source_path: source_str,
            imported_at: chrono::Utc::now(),
        },
    )?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "Imported template {:?} ({} repos)",
        name,
        template.repos.len()
    ))))
}

fn list_cmd() -> Command {
    Command::new("ls")
        .visible_alias("list")
        .about("List all templates [read-only]")
}

fn show_cmd() -> Command {
    Command::new("show")
        .about("Show template contents [read-only]")
        .arg(
            Arg::new("name")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_templates)),
        )
}

fn rm_cmd() -> Command {
    Command::new("rm")
        .visible_alias("remove")
        .about("Remove a template")
        .arg(
            Arg::new("name")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_templates)),
        )
}

fn rename_cmd() -> Command {
    Command::new("rename")
        .about("Rename a template")
        .arg(
            Arg::new("old")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_templates)),
        )
        .arg(Arg::new("new").required(true))
        .arg(
            Arg::new("force")
                .long("force")
                .action(clap::ArgAction::SetTrue)
                .help("Overwrite if the target name already exists"),
        )
}

fn run_rename(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let old_name = matches.get_one::<String>("old").unwrap();
    let new_name = matches.get_one::<String>("new").unwrap();
    let force = matches.get_flag("force");

    tmpl::rename(&paths.templates_dir, old_name, new_name, force)?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "Renamed template {:?} to {:?}",
        old_name, new_name
    ))))
}

fn export_cmd() -> Command {
    Command::new("export")
        .about("Export a template to a file or stdout [read-only]")
        .arg(
            Arg::new("name")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_templates)),
        )
        .arg(
            Arg::new("stdout")
                .long("stdout")
                .action(clap::ArgAction::SetTrue)
                .help("Print to stdout instead of writing a file"),
        )
}

fn run_new(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let from_workspace = matches.get_one::<String>("from-workspace");
    let from_file = matches.get_one::<String>("file");
    let description = matches.get_one::<String>("description").cloned();

    if tmpl::exists(&paths.templates_dir, name) {
        anyhow::bail!("template {:?} already exists", name);
    }

    let mut template = if let Some(ws_name) = from_workspace {
        tmpl::from_workspace(paths, ws_name)?
    } else if let Some(file_path) = from_file {
        tmpl::load_from_file(std::path::Path::new(file_path))?
    } else {
        // Safe to unwrap: clap ArgGroup ensures repos, --workspace, or --file is present
        let repo_args: Vec<String> = matches
            .get_many::<String>("repos")
            .unwrap()
            .cloned()
            .collect();

        let cfg = Config::load_from(&paths.config_path)
            .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;
        let registered: Vec<String> = cfg.repos.keys().cloned().collect();

        let mut repo_urls = Vec::new();
        for rn in &repo_args {
            let repo_name = giturl::parse_repo_ref(rn);
            match giturl::resolve(repo_name, &registered) {
                Ok(id) => {
                    let url = cfg
                        .upstream_url(&id)
                        .ok_or_else(|| anyhow::anyhow!("repo {:?} has no URL in registry", id))?
                        .to_string();
                    repo_urls.push(url);
                }
                Err(resolve_err) => match giturl::parse(repo_name) {
                    Ok(_) => repo_urls.push(repo_name.to_string()),
                    Err(_) => return Err(resolve_err),
                },
            }
        }

        tmpl::Template {
            name: None,
            description: None,
            wsp_version: None,
            repos: repo_urls
                .into_iter()
                .map(|url| tmpl::TemplateRepo {
                    url,
                    setup_commands: None,
                })
                .collect(),
            config: None,
            agent_md: None,
            setup_commands: None,
        }
    };

    if description.is_some() {
        template.description = description;
    }

    template.print_customizations();

    let repo_count = template.repos.len();
    tmpl::save(&paths.templates_dir, name, &template)?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "Created template {:?} with {} repos",
        name, repo_count
    ))))
}

fn run_list(_matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let names = tmpl::list(&paths.templates_dir)?;

    let mut templates = Vec::new();
    for name in &names {
        match tmpl::load(&paths.templates_dir, name) {
            Ok(t) => templates.push(TemplateListEntry {
                name: name.clone(),
                repo_count: t.repos.len(),
            }),
            Err(e) => {
                eprintln!("warning: skipping template {:?}: {}", name, e);
            }
        }
    }

    Ok(Output::TemplateList(TemplateListOutput { templates }))
}

fn run_show(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let t = tmpl::load(&paths.templates_dir, name)?;

    let repos: Vec<wsp_core::output::TemplateShowRepo> = t
        .repos
        .iter()
        .map(|r| {
            let identity = giturl::parse(&r.url)
                .map(|p| p.identity())
                .unwrap_or_default();
            wsp_core::output::TemplateShowRepo {
                url: r.url.clone(),
                identity,
            }
        })
        .collect();

    Ok(Output::TemplateShow(TemplateShowOutput {
        name: name.clone(),
        repos,
    }))
}

fn run_rm(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap().clone();

    tmpl::delete(&paths.templates_dir, &name)?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "Removed template {:?}",
        name
    ))))
}

fn run_export(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let to_stdout = matches.get_flag("stdout");

    let mut t = tmpl::load(&paths.templates_dir, name)?;

    // Populate name field in exported file so importers get a default name
    if t.name.is_none() {
        t.name = Some(name.clone());
    }

    // Report what's being exported
    if !to_stdout {
        eprintln!("Exporting template {:?} ({} repos):", name, t.repos.len());
        t.print_customizations();
        eprintln!("  note: custom skills (.claude/skills/) are not included in exports");
    }

    let yaml = tmpl::to_yaml(&t)?;

    if to_stdout {
        print!("{}", yaml);
        Ok(Output::None)
    } else {
        let filename = format!("{}.wsp.yaml", name);
        let dest = std::env::current_dir()?.join(&filename);
        if dest.exists() {
            anyhow::bail!("{:?} already exists", filename);
        }
        let mut f = fs::File::create(&dest)?;
        f.write_all(yaml.as_bytes())?;
        Ok(Output::Mutation(MutationOutput::new(format!(
            "Exported template to {}",
            dest.display()
        ))))
    }
}

// ---------------------------------------------------------------------------
// template repo add/rm
// ---------------------------------------------------------------------------

fn repo_cmd() -> Command {
    Command::new("repo")
        .about("Add or remove repos in a template")
        .long_about(
            "Add or remove repos in an existing template.\n\n\
             Mirrors `wsp repo add/rm` but operates on a stored template instead of \
             a workspace. `repo add` is idempotent — repos already present are skipped \
             with a warning.",
        )
        .subcommand_required(true)
        .subcommand(
            Command::new("add")
                .about("Add repos to a template")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                )
                .arg(
                    Arg::new("repos")
                        .required(true)
                        .num_args(1..)
                        .help("Repo URLs or shortnames to add")
                        .add(ArgValueCandidates::new(completers::complete_repos)),
                ),
        )
        .subcommand(
            Command::new("rm")
                .visible_alias("remove")
                .about("Remove repos from a template")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                )
                .arg(
                    Arg::new("repos")
                        .required(true)
                        .num_args(1..)
                        .help("Repo URLs, identities, or shortnames to remove")
                        .add(ArgValueCandidates::new(completers::complete_template_repos)),
                ),
        )
}

fn dispatch_repo(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    match matches.subcommand() {
        Some(("add", m)) => run_repo_add(m, paths),
        Some(("rm", m)) => run_repo_rm(m, paths),
        _ => unreachable!(),
    }
}

fn run_repo_add(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let repo_args: Vec<String> = matches
        .get_many::<String>("repos")
        .unwrap()
        .cloned()
        .collect();

    let cfg = Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;
    let registered: Vec<String> = cfg.repos.keys().cloned().collect();

    let mut resolved_urls = Vec::new();
    for rn in &repo_args {
        let repo_name = giturl::parse_repo_ref(rn);
        match giturl::resolve(repo_name, &registered) {
            Ok(id) => {
                let url = cfg
                    .upstream_url(&id)
                    .ok_or_else(|| anyhow::anyhow!("repo {:?} has no URL in registry", id))?
                    .to_string();
                resolved_urls.push(url);
            }
            Err(resolve_err) => match giturl::parse(repo_name) {
                Ok(_) => resolved_urls.push(repo_name.to_string()),
                Err(_) => return Err(resolve_err),
            },
        }
    }

    let template = filelock::with_template(&paths.templates_dir, name, |tmpl| {
        let skipped = tmpl::add_repos(tmpl, resolved_urls)?;
        for url in &skipped {
            eprintln!("warning: repo {:?} already in template, skipping", url);
        }
        Ok(())
    })?;

    let show = template_show_output(name, &template);
    Ok(Output::TemplateShow(show))
}

fn run_repo_rm(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let repos: Vec<String> = matches
        .get_many::<String>("repos")
        .unwrap()
        .cloned()
        .collect();

    let template = filelock::with_template(&paths.templates_dir, name, |tmpl| {
        tmpl::remove_repos(tmpl, repos)?;
        if tmpl.repos.is_empty() {
            anyhow::bail!("cannot remove all repos from template — use `wsp template rm` instead");
        }
        Ok(())
    })?;

    let show = template_show_output(name, &template);
    Ok(Output::TemplateShow(show))
}

// ---------------------------------------------------------------------------
// template config set/get/unset
// ---------------------------------------------------------------------------

fn config_cmd() -> Command {
    Command::new("config")
        .about("Manage template config overrides")
        .long_about(
            "Manage template-scoped config overrides.\n\n\
             Template config overrides global config when a workspace is created from the \
             template. Valid keys: lang.<name>, sync-strategy, git.<key>.",
        )
        .subcommand_required(true)
        .subcommand(
            Command::new("set")
                .about("Set a template config override")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                )
                .arg(Arg::new("key").required(true).add(ArgValueCandidates::new(
                    completers::complete_template_config_keys,
                )))
                .arg(Arg::new("value").required(true)),
        )
        .subcommand(
            Command::new("get")
                .about("Get a template config value [read-only]")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                )
                .arg(Arg::new("key").required(true).add(ArgValueCandidates::new(
                    completers::complete_template_config_keys,
                ))),
        )
        .subcommand(
            Command::new("unset")
                .about("Unset a template config override")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                )
                .arg(Arg::new("key").required(true).add(ArgValueCandidates::new(
                    completers::complete_template_config_keys,
                ))),
        )
}

fn dispatch_config(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    match matches.subcommand() {
        Some(("set", m)) => run_config_set(m, paths),
        Some(("get", m)) => run_config_get(m, paths),
        Some(("unset", m)) => run_config_unset(m, paths),
        _ => unreachable!(),
    }
}

fn run_config_set(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let key = matches.get_one::<String>("key").unwrap().clone();
    let value = matches.get_one::<String>("value").unwrap().clone();

    filelock::with_template(&paths.templates_dir, name, |tmpl| {
        tmpl::set_config(tmpl, &key, &value)
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "template {:?}: {} = {}",
        name, key, value
    ))))
}

fn run_config_get(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let key = matches.get_one::<String>("key").unwrap();

    // Read-only: no lock needed
    let template = tmpl::load(&paths.templates_dir, name)?;
    let value = tmpl::get_config(&template, key)?;

    Ok(Output::ConfigGet(ConfigGetOutput {
        key: key.clone(),
        value,
    }))
}

fn run_config_unset(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let key = matches.get_one::<String>("key").unwrap().clone();

    filelock::with_template(&paths.templates_dir, name, |tmpl| {
        tmpl::unset_config(tmpl, &key)
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "template {:?}: {} unset",
        name, key
    ))))
}

// ---------------------------------------------------------------------------
// template agent-md set/unset
// ---------------------------------------------------------------------------

fn agent_md_cmd() -> Command {
    Command::new("agent-md")
        .about("Manage template AGENTS.md content")
        .long_about(
            "Manage template AGENTS.md content.\n\n\
             Set custom AGENTS.md content that will be included in workspaces created from \
             this template. Use `-` as the path to read from stdin.",
        )
        .subcommand_required(true)
        .subcommand(
            Command::new("set")
                .about("Set AGENTS.md content from a file (use - for stdin)")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                )
                .arg(
                    Arg::new("path")
                        .required(true)
                        .help("File path (or - for stdin)")
                        .value_hint(clap::ValueHint::FilePath),
                ),
        )
        .subcommand(
            Command::new("unset")
                .about("Clear AGENTS.md content from a template")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                ),
        )
}

fn dispatch_agent_md(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    match matches.subcommand() {
        Some(("set", m)) => run_agent_md_set(m, paths),
        Some(("unset", m)) => run_agent_md_unset(m, paths),
        _ => unreachable!(),
    }
}

fn run_agent_md_set(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let path = matches.get_one::<String>("path").unwrap();

    // Read content before acquiring lock — don't hold lock during I/O
    let content = if path == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        fs::read_to_string(path).map_err(|e| anyhow::anyhow!("reading {:?}: {}", path, e))?
    };

    if content.trim().is_empty() {
        anyhow::bail!("agent-md content is empty — use `agent-md unset` to clear");
    }

    // Guard against accidentally loading huge files
    const MAX_AGENT_MD_BYTES: usize = 1_048_576; // 1 MiB
    if content.len() > MAX_AGENT_MD_BYTES {
        anyhow::bail!(
            "agent-md content is {} bytes, exceeds 1 MiB limit",
            content.len()
        );
    }

    // Validate no wsp markers
    if content.contains(wsp_core::agentmd::MARKER_BEGIN)
        || content.contains(wsp_core::agentmd::MARKER_END)
    {
        anyhow::bail!("agent_md content cannot contain wsp markers (<!-- wsp:begin/end -->)");
    }

    filelock::with_template(&paths.templates_dir, name, |tmpl| {
        tmpl.agent_md = Some(content);
        Ok(())
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "template {:?}: agent-md set",
        name
    ))))
}

fn run_agent_md_unset(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();

    filelock::with_template(&paths.templates_dir, name, |tmpl| {
        tmpl.agent_md = None;
        Ok(())
    })?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "template {:?}: agent-md unset",
        name
    ))))
}

// ---------------------------------------------------------------------------
// template setup-commands ls/add/rm/clear
// ---------------------------------------------------------------------------

fn setup_commands_cmd() -> Command {
    Command::new("setup-commands")
        .about("Manage per-repo setup commands in a template")
        .long_about(
            "Manage per-repo setup commands in a template.\n\n\
             The <tmpl> argument is the template name; <repo> must match a repo URL or \
             identity already in that template.",
        )
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("ls")
                .visible_alias("list")
                .about("List setup commands for a repo in a template [read-only]")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .help("Template name")
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                )
                .arg(
                    Arg::new("repo")
                        .required(true)
                        .help("Repo URL or identity within the template")
                        .add(ArgValueCandidates::new(completers::complete_template_repos)),
                ),
        )
        .subcommand(
            Command::new("add")
                .about("Add a setup command for a repo in a template")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .help("Template name")
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                )
                .arg(
                    Arg::new("repo")
                        .required(true)
                        .help("Repo URL or identity within the template")
                        .add(ArgValueCandidates::new(completers::complete_template_repos)),
                )
                .arg(
                    Arg::new("cmd")
                        .required(true)
                        .num_args(1..)
                        .last(true)
                        .allow_hyphen_values(true)
                        .help("Shell command to add; use -- to pass multi-word commands"),
                ),
        )
        .subcommand(
            Command::new("rm")
                .visible_alias("remove")
                .about("Remove a setup command for a repo in a template")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .help("Template name")
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                )
                .arg(
                    Arg::new("repo")
                        .required(true)
                        .help("Repo URL or identity within the template")
                        .add(ArgValueCandidates::new(completers::complete_template_repos)),
                )
                .arg(
                    Arg::new("cmd")
                        .required(true)
                        .num_args(1..)
                        .last(true)
                        .allow_hyphen_values(true)
                        .help("Shell command to remove; use -- to pass multi-word commands")
                        .add(ArgValueCandidates::new(
                            completers::complete_template_repo_setup_commands,
                        )),
                ),
        )
        .subcommand(
            Command::new("clear")
                .about("Clear all setup commands for a repo in a template")
                .arg(
                    Arg::new("name")
                        .required(true)
                        .help("Template name")
                        .add(ArgValueCandidates::new(completers::complete_templates)),
                )
                .arg(
                    Arg::new("repo")
                        .required(true)
                        .help("Repo URL or identity within the template")
                        .add(ArgValueCandidates::new(completers::complete_template_repos)),
                ),
        )
}

fn dispatch_setup_commands(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    match matches.subcommand() {
        Some(("ls", m)) => run_setup_commands_ls(m, paths),
        Some(("add", m)) => run_setup_commands_add_cmd(m, paths),
        Some(("rm", m)) => run_setup_commands_rm_cmd(m, paths),
        Some(("clear", m)) => run_setup_commands_clear_cmd(m, paths),
        _ => unreachable!(),
    }
}

fn run_setup_commands_ls(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let repo_arg = matches.get_one::<String>("repo").unwrap();
    let template = tmpl::load(&paths.templates_dir, name)?;
    let repo = find_template_repo(&template, repo_arg)?;
    let cmds = repo.setup_commands.as_deref().unwrap_or(&[]);
    let commands = cmds
        .iter()
        .map(|c| wsp_core::output::SetupCommandEntry {
            command: c.clone(),
            source: "template".to_string(),
        })
        .collect();
    let identity = giturl::parse(&repo.url)
        .map(|p| p.identity())
        .unwrap_or_else(|_| repo.url.clone());
    Ok(Output::SetupCommands(
        wsp_core::output::SetupCommandsOutput {
            repo: identity,
            commands,
        },
    ))
}

fn run_setup_commands_add_cmd(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let repo_arg = matches.get_one::<String>("repo").unwrap();
    let cmd = matches
        .get_many::<String>("cmd")
        .unwrap()
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    if cmd.trim().is_empty() {
        anyhow::bail!("setup command cannot be empty or whitespace-only");
    }
    filelock::with_template(&paths.templates_dir, name, |tmpl| {
        let repo = find_template_repo_mut(tmpl, repo_arg)?;
        let cmds = repo.setup_commands.get_or_insert_with(Vec::new);
        if cmds.contains(&cmd) {
            anyhow::bail!("command already exists: {:?}", cmd);
        }
        cmds.push(cmd.clone());
        Ok(())
    })?;
    Ok(Output::Mutation(MutationOutput::new(format!(
        "template {:?}: added setup command for {}",
        name, repo_arg
    ))))
}

fn run_setup_commands_rm_cmd(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let repo_arg = matches.get_one::<String>("repo").unwrap();
    let cmd = matches
        .get_many::<String>("cmd")
        .unwrap()
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    filelock::with_template(&paths.templates_dir, name, |tmpl| {
        let repo = find_template_repo_mut(tmpl, repo_arg)?;
        let cmds = repo.setup_commands.get_or_insert_with(Vec::new);
        let before = cmds.len();
        cmds.retain(|c| *c != cmd);
        if cmds.len() == before {
            anyhow::bail!("command not found: {:?}", cmd);
        }
        if cmds.is_empty() {
            repo.setup_commands = None;
        }
        Ok(())
    })?;
    Ok(Output::Mutation(MutationOutput::new(format!(
        "template {:?}: removed setup command for {}",
        name, repo_arg
    ))))
}

fn run_setup_commands_clear_cmd(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();
    let repo_arg = matches.get_one::<String>("repo").unwrap();
    filelock::with_template(&paths.templates_dir, name, |tmpl| {
        let repo = find_template_repo_mut(tmpl, repo_arg)?;
        repo.setup_commands = None;
        Ok(())
    })?;
    Ok(Output::Mutation(MutationOutput::new(format!(
        "template {:?}: cleared setup commands for {}",
        name, repo_arg
    ))))
}

/// Find a TemplateRepo by URL or identity (immutable).
fn find_template_repo<'a>(
    template: &'a tmpl::Template,
    repo_arg: &str,
) -> Result<&'a tmpl::TemplateRepo> {
    template
        .repos
        .iter()
        .find(|r| {
            r.url == repo_arg
                || giturl::parse(&r.url)
                    .map(|p| p.identity())
                    .unwrap_or_default()
                    == repo_arg
        })
        .ok_or_else(|| anyhow::anyhow!("repo {:?} not found in template", repo_arg))
}

/// Find a TemplateRepo by URL or identity (mutable).
fn find_template_repo_mut<'a>(
    template: &'a mut tmpl::Template,
    repo_arg: &str,
) -> Result<&'a mut tmpl::TemplateRepo> {
    template
        .repos
        .iter_mut()
        .find(|r| {
            r.url == repo_arg
                || giturl::parse(&r.url)
                    .map(|p| p.identity())
                    .unwrap_or_default()
                    == repo_arg
        })
        .ok_or_else(|| anyhow::anyhow!("repo {:?} not found in template", repo_arg))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn template_show_output(name: &str, template: &tmpl::Template) -> TemplateShowOutput {
    let repos = template
        .repos
        .iter()
        .map(|r| {
            let identity = giturl::parse(&r.url)
                .map(|p| p.identity())
                .unwrap_or_default();
            wsp_core::output::TemplateShowRepo {
                url: r.url.clone(),
                identity,
            }
        })
        .collect();

    TemplateShowOutput {
        name: name.to_string(),
        repos,
    }
}
