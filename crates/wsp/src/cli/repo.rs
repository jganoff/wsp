use anyhow::{Result, bail};
use chrono::Utc;
use clap::{Arg, ArgMatches, Command};
use clap_complete::engine::ArgValueCandidates;
use url::Url;

use wsp_core::config::{self, Paths, RepoEntry};
use wsp_core::discovery;
use wsp_core::filelock;
use wsp_core::giturl;
use wsp_core::mirror;
use wsp_core::output::{
    ImportFailure, ImportOutput, MutationOutput, Output, RepoListEntry, RepoListOutput,
};

use super::completers;

pub fn add_cmd() -> Command {
    Command::new("add")
        .about("Register and bare-clone a repository")
        .long_about(
            "Register and bare-clone a repository.\n\n\
             Adds a repo to the global registry and creates a bare mirror clone. Supports \
             individual URLs or bulk import from a GitHub org/user with --from.",
        )
        .arg(Arg::new("url").required_unless_present("from"))
        .arg(
            Arg::new("from")
                .long("from")
                .help("Import repos from a GitHub org/user (e.g. github.com/acme)")
                .conflicts_with("url"),
        )
        .arg(
            Arg::new("pattern")
                .long("pattern")
                .help("Glob pattern(s) to filter repo names, comma-separated")
                .requires("from"),
        )
        .arg(
            Arg::new("all")
                .long("all")
                .action(clap::ArgAction::SetTrue)
                .help("Import all repos (required if --pattern not given)")
                .requires("from")
                .conflicts_with("pattern"),
        )
        .arg(
            Arg::new("no-discover")
                .long("no-discover")
                .action(clap::ArgAction::SetTrue)
                .help("Skip template discovery in cloned repos"),
        )
}

pub fn list_cmd() -> Command {
    Command::new("ls")
        .visible_alias("list")
        .about("List registered repositories [read-only]")
}

pub fn rm_cmd() -> Command {
    Command::new("rm")
        .visible_alias("remove")
        .about("Remove a repository and its mirror")
        .arg(
            Arg::new("name")
                .required(true)
                .add(ArgValueCandidates::new(completers::complete_repos)),
        )
}

/// Strip user credentials from an HTTPS URL before persisting.
///
/// Returns the sanitised URL and a boolean indicating whether credentials were
/// present. SSH URLs (`git@host:org/repo.git`) and plain HTTPS URLs without
/// credentials are returned unchanged.
fn strip_url_credentials(url_str: &str) -> (String, bool) {
    if let Ok(mut u) = Url::parse(url_str) {
        let has_creds = !u.username().is_empty() || u.password().is_some();
        if has_creds {
            let _ = u.set_username("");
            let _ = u.set_password(None);
            return (u.to_string(), true);
        }
    }
    (url_str.to_string(), false)
}

pub fn run_add(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    if matches.get_one::<String>("from").is_some() {
        return run_add_from(matches, paths);
    }

    let raw_url = matches.get_one::<String>("url").unwrap();
    let parsed = giturl::parse(raw_url)?;
    let identity = parsed.identity();

    // Strip credentials from the URL before persisting; keep raw_url for the
    // actual clone so git can still authenticate if credentials were embedded.
    let (store_url, had_creds) = strip_url_credentials(raw_url);
    if had_creds {
        eprintln!(
            "warning: credentials stripped from URL; use SSH or git-credential-helper instead"
        );
    }

    // Phase 1: pre-check under lock (fast, read-only)
    let snapshot = filelock::read_config(&paths.config_path)?;
    if snapshot.repos.contains_key(&identity) {
        bail!("repo {} already registered", identity);
    }
    if mirror::exists(&paths.mirrors_dir, &parsed) {
        bail!("mirror already exists for {}", identity);
    }

    // Phase 2: clone mirror + initial fetch (slow, no lock held)
    eprintln!("Cloning {}...", raw_url);
    mirror::clone(&paths.mirrors_dir, &parsed, raw_url)
        .map_err(|e| anyhow::anyhow!("cloning: {}", e))?;
    mirror::fetch(&paths.mirrors_dir, &parsed)
        .map_err(|e| anyhow::anyhow!("initial fetch: {}", e))?;

    // Phase 3: register under lock (fast, re-check for concurrent add)
    let result = filelock::with_config(&paths.config_path, |cfg| {
        if cfg.repos.contains_key(&identity) {
            bail!(
                "repo {} was registered by another process during clone",
                identity
            );
        }
        cfg.repos.insert(
            identity.clone(),
            RepoEntry {
                url: store_url.clone(),
                added: Utc::now(),
                setup_commands: None,
            },
        );
        Ok(())
    });

    if result.is_err() {
        // Clean up the orphaned mirror we cloned in phase 2
        let _ = mirror::remove(&paths.mirrors_dir, &parsed);
    }
    result?;

    // Template discovery: scan the bare mirror for .wsp.yaml files
    let no_discover = matches.get_flag("no-discover");
    if !no_discover {
        let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
        let discovered = discovery::scan_bare_mirror(&mirror_dir, &identity, &paths.templates_dir);
        if let Err(e) = discovery::prompt_and_import(&discovered, &paths.templates_dir) {
            eprintln!("warning: template discovery failed: {}", e);
        }
    }

    Ok(Output::Mutation(MutationOutput::new(format!(
        "Registered {}",
        identity
    ))))
}

fn run_add_from(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let from = matches.get_one::<String>("from").unwrap();
    let patterns: Vec<&str> = matches
        .get_one::<String>("pattern")
        .map(|p| p.split(',').map(|s| s.trim()).collect())
        .unwrap_or_default();
    let all = matches.get_flag("all");

    if !all && patterns.is_empty() {
        bail!("--from requires either --pattern or --all");
    }

    let (host, owner) = parse_from_arg(from)?;
    if host != "github.com" {
        bail!("only github.com is supported (got {})", host);
    }

    let repos = gh_list_repos(&owner)?;

    let filtered: Vec<_> = if all {
        repos
    } else {
        repos
            .into_iter()
            .filter(|(name, _url)| patterns.iter().any(|p| glob_match(p, name)))
            .collect()
    };

    if filtered.is_empty() {
        bail!("no repos matched");
    }

    let no_discover = matches.get_flag("no-discover");
    let result = import_repos(paths, &filtered, no_discover)?;
    Ok(Output::Import(result))
}

/// Clone and register a list of repos using the three-phase lock pattern.
/// Reused by `wsp repo add --from` and `wsp setup`.
fn import_repos(
    paths: &Paths,
    repos: &[(String, String)],
    no_discover: bool,
) -> Result<ImportOutput> {
    // Phase 1: snapshot current config to know which repos to skip (fast lock)
    let snapshot = filelock::read_config(&paths.config_path)?;
    let existing_identities: std::collections::HashSet<String> =
        snapshot.repos.keys().cloned().collect();

    // Phase 2: clone mirrors outside the lock (slow network I/O)
    struct CloneResult {
        identity: String,
        url: String,
    }
    let mut cloned = Vec::new();
    let mut skipped = Vec::new();
    let mut failed = Vec::new();

    for (name, url) in repos {
        let parsed = match giturl::parse(url) {
            Ok(p) => p,
            Err(e) => {
                failed.push(ImportFailure {
                    name: name.clone(),
                    error: e.to_string(),
                });
                continue;
            }
        };
        let identity = parsed.identity();

        if existing_identities.contains(&identity) {
            skipped.push(identity);
            continue;
        }

        // Mirror exists on disk but not in config (e.g. crash recovery) — re-register
        if mirror::exists(&paths.mirrors_dir, &parsed) {
            let (store_url, had_creds) = strip_url_credentials(url);
            if had_creds {
                eprintln!(
                    "warning: credentials stripped from URL; use SSH or git-credential-helper instead"
                );
            }
            cloned.push(CloneResult {
                identity,
                url: store_url,
            });
            continue;
        }

        eprintln!("Cloning {}...", url);
        if let Err(e) = mirror::clone(&paths.mirrors_dir, &parsed, url)
            .and_then(|_| mirror::fetch(&paths.mirrors_dir, &parsed))
        {
            failed.push(ImportFailure {
                name: name.clone(),
                error: e.to_string(),
            });
            continue;
        }

        let (store_url, had_creds) = strip_url_credentials(url);
        if had_creds {
            eprintln!(
                "warning: credentials stripped from URL; use SSH or git-credential-helper instead"
            );
        }
        cloned.push(CloneResult {
            identity,
            url: store_url,
        });
    }

    // Phase 3: register all cloned repos under a single short lock
    let mut registered = Vec::new();
    if !cloned.is_empty() {
        filelock::with_config(&paths.config_path, |cfg| {
            for cr in &cloned {
                if cfg.repos.contains_key(&cr.identity) {
                    // Concurrently registered by another process — skip
                    skipped.push(cr.identity.clone());
                    continue;
                }
                cfg.repos.insert(
                    cr.identity.clone(),
                    RepoEntry {
                        url: cr.url.clone(),
                        added: Utc::now(),
                        setup_commands: None,
                    },
                );
                registered.push(cr.identity.clone());
            }
            Ok(())
        })?;
    }

    // Template discovery: scan newly registered bare mirrors for .wsp.yaml files
    if !no_discover && !registered.is_empty() {
        let mut all_discovered = Vec::new();
        for identity in &registered {
            if let Ok(parsed) = giturl::Parsed::from_identity(identity) {
                let mirror_dir = mirror::dir(&paths.mirrors_dir, &parsed);
                let discovered =
                    discovery::scan_bare_mirror(&mirror_dir, identity, &paths.templates_dir);
                all_discovered.extend(discovered);
            }
        }
        if let Err(e) = discovery::prompt_and_import(&all_discovered, &paths.templates_dir) {
            eprintln!("warning: template discovery failed: {}", e);
        }
    }

    Ok(ImportOutput {
        registered,
        skipped,
        failed,
    })
}

fn parse_from_arg(from: &str) -> Result<(String, String)> {
    // Strip protocol prefix if copy-pasted from browser
    let from = from
        .strip_prefix("https://")
        .or_else(|| from.strip_prefix("http://"))
        .unwrap_or(from);

    if from.is_empty() {
        bail!("--from value cannot be empty");
    }

    let (host, owner) = if let Some(idx) = from.find('/') {
        let h = &from[..idx];
        let o = &from[idx + 1..];
        // Trim trailing slash from owner (e.g. "github.com/acme/")
        let o = o.trim_end_matches('/');
        if h.is_empty() {
            bail!("missing host in --from");
        }
        if o.is_empty() {
            bail!("missing org/user name in --from");
        }
        (h.to_string(), o.to_string())
    } else {
        ("github.com".to_string(), from.to_string())
    };

    if owner.starts_with('-') {
        bail!("org/user name cannot start with '-': {:?}", owner);
    }

    Ok((host, owner))
}

fn gh_list_repos(owner: &str) -> Result<Vec<(String, String)>> {
    let limit = 1000;
    let output = std::process::Command::new("gh")
        .args([
            "repo",
            "list",
            "--json",
            "name,url",
            "--limit",
            &limit.to_string(),
            "--no-archived",
            "--", // end of flags — owner is always treated as positional
            owner,
        ])
        .output()
        .map_err(|e| anyhow::anyhow!("failed to run gh: {} (is gh installed?)", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("gh repo list failed: {}", stderr);
    }

    let entries: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)?;

    if entries.len() >= limit {
        eprintln!(
            "warning: gh returned {} repos; results may be truncated",
            entries.len()
        );
    }

    let repos: Vec<(String, String)> = entries
        .iter()
        .filter_map(|e| {
            let name = e["name"].as_str()?;
            let url = e["url"].as_str()?;
            Some((name.to_string(), url.to_string()))
        })
        .collect();

    Ok(repos)
}

fn glob_match(pattern: &str, name: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        match name[pos..].find(part) {
            Some(idx) => {
                if i == 0 && idx != 0 {
                    return false;
                }
                pos += idx + part.len();
            }
            None => return false,
        }
    }
    if !pattern.ends_with('*') {
        pos == name.len()
    } else {
        true
    }
}

pub fn run_list(_matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let cfg = config::Config::load_from(&paths.config_path)
        .map_err(|e| anyhow::anyhow!("loading config: {}", e))?;

    let mut identities: Vec<String> = cfg.repos.keys().cloned().collect();
    identities.sort();

    let shortnames = giturl::shortnames(&identities);

    let repos = identities
        .iter()
        .map(|id| {
            let entry = &cfg.repos[id];
            let short = shortnames.get(id).cloned().unwrap_or_else(|| id.clone());
            RepoListEntry {
                identity: id.clone(),
                shortname: short,
                url: entry.url.clone(),
            }
        })
        .collect();

    Ok(Output::RepoList(RepoListOutput { repos }))
}

pub fn run_remove(matches: &ArgMatches, paths: &Paths) -> Result<Output> {
    let name = matches.get_one::<String>("name").unwrap();

    // Phase 1: resolve identity and URL under lock (fast, read-only)
    let snapshot = filelock::read_config(&paths.config_path)?;
    let identities: Vec<String> = snapshot.repos.keys().cloned().collect();
    let identity = giturl::resolve(name, &identities)?;
    let entry = &snapshot.repos[&identity];
    let parsed = giturl::parse(&entry.url)?;

    // Phase 2: unregister under lock (fast) — before mirror deletion so that
    // a crash between phases leaves config clean rather than orphaned.
    filelock::with_config(&paths.config_path, |cfg| {
        cfg.repos.remove(&identity);
        Ok(())
    })?;

    // Phase 3: remove mirror (no lock held, idempotent — tolerates already-removed)
    eprintln!("Removing mirror for {}...", identity);
    mirror::remove(&paths.mirrors_dir, &parsed)
        .map_err(|e| anyhow::anyhow!("removing mirror: {}", e))?;

    Ok(Output::Mutation(MutationOutput::new(format!(
        "Removed {}",
        identity
    ))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        let cases = vec![
            ("prefix wildcard", "api-*", "api-gateway", true),
            ("prefix wildcard 2", "api-*", "api-v2", true),
            ("prefix no match", "api-*", "user-api", false),
            ("suffix wildcard", "*-service", "user-service", true),
            ("suffix no match", "*-service", "service-mesh", false),
            ("contains wildcard", "*core*", "core", true),
            ("contains wildcard 2", "*core*", "core-lib", true),
            ("contains wildcard 3", "*core*", "my-core-service", true),
            ("exact match", "exact", "exact", true),
            ("exact no match", "exact", "exactly", false),
            ("match all", "*", "anything", true),
            ("match all empty", "*", "", true),
            ("empty pattern", "", "", true),
            ("empty pattern no match", "", "x", false),
            ("multi wildcard", "a*b*c", "aXbYc", true),
            ("multi wildcard no match", "a*b*c", "aXYc", false),
            ("anchored start", "api-*", "xapi-foo", false),
            ("anchored end", "*-api", "api-x", false),
        ];
        for (name, pattern, input, want) in cases {
            assert_eq!(
                glob_match(pattern, input),
                want,
                "{}: glob_match({:?}, {:?})",
                name,
                pattern,
                input
            );
        }
    }

    #[test]
    fn test_parse_from_arg() {
        let cases = vec![
            ("full host", "github.com/acme", Ok(("github.com", "acme"))),
            ("shorthand", "acme", Ok(("github.com", "acme"))),
            ("gitlab", "gitlab.com/team", Ok(("gitlab.com", "team"))),
            (
                "https prefix stripped",
                "https://github.com/acme",
                Ok(("github.com", "acme")),
            ),
            (
                "http prefix stripped",
                "http://github.com/acme",
                Ok(("github.com", "acme")),
            ),
            (
                "trailing slash trimmed",
                "github.com/acme/",
                Ok(("github.com", "acme")),
            ),
        ];
        for (name, input, want) in cases {
            let result = parse_from_arg(input);
            match want {
                Ok((host, owner)) => {
                    let (got_host, got_owner) =
                        result.unwrap_or_else(|e| panic!("{}: unexpected error: {}", name, e));
                    assert_eq!(got_host, host, "{}", name);
                    assert_eq!(got_owner, owner, "{}", name);
                }
                Err(()) => {
                    assert!(result.is_err(), "{}: expected error", name);
                }
            }
        }
    }

    #[test]
    fn test_parse_from_arg_errors() {
        let cases = vec![
            ("empty", ""),
            ("only slash", "github.com/"),
            ("leading slash", "/acme"),
            ("dash prefix", "--help"),
            ("dash org", "-R"),
        ];
        for (name, input) in cases {
            assert!(
                parse_from_arg(input).is_err(),
                "{}: expected error for {:?}",
                name,
                input
            );
        }
    }

    // R003: credential stripping

    #[test]
    fn test_strip_url_credentials_strips_user_and_password() {
        let (clean, had) = strip_url_credentials("https://user:secret@github.com/org/repo.git");
        assert!(had, "should report credentials were present");
        assert_eq!(clean, "https://github.com/org/repo.git");
    }

    #[test]
    fn test_strip_url_credentials_strips_user_only() {
        let (clean, had) = strip_url_credentials("https://user@github.com/org/repo.git");
        assert!(had, "should report credentials were present");
        assert_eq!(clean, "https://github.com/org/repo.git");
    }

    #[test]
    fn test_strip_url_credentials_plain_https_unchanged() {
        let (clean, had) = strip_url_credentials("https://github.com/org/repo.git");
        assert!(!had, "no credentials expected");
        assert_eq!(clean, "https://github.com/org/repo.git");
    }

    #[test]
    fn test_strip_url_credentials_ssh_url_unchanged() {
        // SSH URLs are not parseable by the url crate as standard URLs; they must
        // pass through untouched.
        let ssh = "git@github.com:org/repo.git";
        let (clean, had) = strip_url_credentials(ssh);
        assert!(!had, "SSH URLs should not be treated as having credentials");
        assert_eq!(clean, ssh);
    }
}
