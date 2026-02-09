use clap_complete::engine::CompletionCandidate;

use crate::config::Config;
use crate::giturl;
use crate::group;
use crate::workspace;

pub fn complete_groups() -> Vec<CompletionCandidate> {
    let Ok(cfg) = Config::load() else {
        return Vec::new();
    };
    group::list(&cfg)
        .into_iter()
        .map(CompletionCandidate::new)
        .collect()
}

pub fn complete_repos() -> Vec<CompletionCandidate> {
    let Ok(cfg) = Config::load() else {
        return Vec::new();
    };
    let identities: Vec<String> = cfg.repos.keys().cloned().collect();
    let shortnames = giturl::shortnames(&identities);
    shortnames
        .into_iter()
        .map(|(identity, short)| CompletionCandidate::new(short).help(Some(identity.into())))
        .collect()
}

pub fn complete_workspaces() -> Vec<CompletionCandidate> {
    let Ok(names) = workspace::list_all() else {
        return Vec::new();
    };
    names
        .into_iter()
        .map(CompletionCandidate::new)
        .collect()
}
