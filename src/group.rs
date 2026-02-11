use std::collections::HashSet;

use anyhow::{Result, bail};

use crate::config::{Config, GroupEntry};

pub fn create(cfg: &mut Config, name: &str, repos: Vec<String>) -> Result<()> {
    if cfg.groups.contains_key(name) {
        bail!("group {:?} already exists", name);
    }
    cfg.groups.insert(name.to_string(), GroupEntry { repos });
    Ok(())
}

pub fn delete(cfg: &mut Config, name: &str) -> Result<()> {
    if !cfg.groups.contains_key(name) {
        bail!("group {:?} not found", name);
    }
    cfg.groups.remove(name);
    Ok(())
}

pub fn get(cfg: &Config, name: &str) -> Result<Vec<String>> {
    match cfg.groups.get(name) {
        Some(g) => Ok(g.repos.clone()),
        None => bail!("group {:?} not found", name),
    }
}

pub fn list(cfg: &Config) -> Vec<String> {
    cfg.groups.keys().cloned().collect()
}

pub fn add_repos(cfg: &mut Config, name: &str, repos: Vec<String>) -> Result<()> {
    let group = cfg
        .groups
        .get_mut(name)
        .ok_or_else(|| anyhow::anyhow!("group {:?} not found", name))?;

    let mut seen = HashSet::new();
    for repo in &repos {
        if !seen.insert(repo.as_str()) {
            bail!("duplicate repo {:?} in add list", repo);
        }
        if group.repos.contains(repo) {
            bail!("repo {:?} already in group {:?}", repo, name);
        }
    }

    group.repos.extend(repos);
    Ok(())
}

pub fn remove_repos(cfg: &mut Config, name: &str, repos: Vec<String>) -> Result<()> {
    let group = cfg
        .groups
        .get_mut(name)
        .ok_or_else(|| anyhow::anyhow!("group {:?} not found", name))?;

    for repo in &repos {
        if !group.repos.contains(repo) {
            bail!("repo {:?} not in group {:?}", repo, name);
        }
    }

    group.repos.retain(|r| !repos.contains(r));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::collections::BTreeMap;

    fn new_config() -> Config {
        Config {
            branch_prefix: None,
            repos: BTreeMap::new(),
            groups: BTreeMap::new(),
            language_integrations: None,
            workspaces_dir: None,
        }
    }

    #[test]
    fn test_create_and_get() {
        let mut cfg = new_config();
        let repos = vec![
            "github.com/user/repo-a".into(),
            "github.com/user/repo-b".into(),
        ];
        create(&mut cfg, "backend", repos.clone()).unwrap();
        let got = get(&cfg, "backend").unwrap();
        assert_eq!(got, repos);
    }

    #[test]
    fn test_create_duplicate() {
        let mut cfg = new_config();
        create(&mut cfg, "backend", vec!["a".into()]).unwrap();
        assert!(create(&mut cfg, "backend", vec!["b".into()]).is_err());
    }

    #[test]
    fn test_delete() {
        let mut cfg = new_config();
        create(&mut cfg, "backend", vec!["a".into()]).unwrap();
        delete(&mut cfg, "backend").unwrap();
        assert!(get(&cfg, "backend").is_err());
    }

    #[test]
    fn test_delete_not_found() {
        let mut cfg = new_config();
        assert!(delete(&mut cfg, "nonexistent").is_err());
    }

    #[test]
    fn test_list() {
        let mut cfg = new_config();
        create(&mut cfg, "backend", vec!["a".into()]).unwrap();
        create(&mut cfg, "frontend", vec!["b".into()]).unwrap();
        let mut names = list(&cfg);
        names.sort();
        assert_eq!(names, vec!["backend", "frontend"]);
    }

    #[test]
    fn test_list_empty() {
        let cfg = new_config();
        assert!(list(&cfg).is_empty());
    }

    #[test]
    fn test_add_repos() {
        struct Case {
            name: &'static str,
            group: &'static str,
            initial: Vec<&'static str>,
            add: Vec<&'static str>,
            want_err: bool,
            want_repos: Vec<&'static str>,
        }

        let cases = vec![
            Case {
                name: "add to existing group",
                group: "backend",
                initial: vec!["repo-a"],
                add: vec!["repo-b", "repo-c"],
                want_err: false,
                want_repos: vec!["repo-a", "repo-b", "repo-c"],
            },
            Case {
                name: "duplicate repo already in group errors",
                group: "backend",
                initial: vec!["repo-a"],
                add: vec!["repo-a"],
                want_err: true,
                want_repos: vec!["repo-a"],
            },
            Case {
                name: "duplicate within add list errors",
                group: "backend",
                initial: vec!["repo-a"],
                add: vec!["repo-b", "repo-b"],
                want_err: true,
                want_repos: vec!["repo-a"],
            },
            Case {
                name: "group not found",
                group: "nonexistent",
                initial: vec![],
                add: vec!["repo-a"],
                want_err: true,
                want_repos: vec![],
            },
        ];

        for tc in cases {
            let mut cfg = new_config();
            if tc.group == "backend" {
                create(
                    &mut cfg,
                    "backend",
                    tc.initial.iter().map(|s| s.to_string()).collect(),
                )
                .unwrap();
            }

            let result = add_repos(
                &mut cfg,
                tc.group,
                tc.add.iter().map(|s| s.to_string()).collect(),
            );
            assert_eq!(result.is_err(), tc.want_err, "case: {}", tc.name);

            if !tc.want_err {
                let got = get(&cfg, tc.group).unwrap();
                let want: Vec<String> = tc.want_repos.iter().map(|s| s.to_string()).collect();
                assert_eq!(got, want, "case: {}", tc.name);
            }
        }
    }

    #[test]
    fn test_remove_repos() {
        struct Case {
            name: &'static str,
            group: &'static str,
            initial: Vec<&'static str>,
            remove: Vec<&'static str>,
            want_err: bool,
            want_repos: Vec<&'static str>,
        }

        let cases = vec![
            Case {
                name: "remove from existing group",
                group: "backend",
                initial: vec!["repo-a", "repo-b", "repo-c"],
                remove: vec!["repo-b"],
                want_err: false,
                want_repos: vec!["repo-a", "repo-c"],
            },
            Case {
                name: "remove absent repo errors",
                group: "backend",
                initial: vec!["repo-a"],
                remove: vec!["repo-z"],
                want_err: true,
                want_repos: vec!["repo-a"],
            },
            Case {
                name: "group not found",
                group: "nonexistent",
                initial: vec![],
                remove: vec!["repo-a"],
                want_err: true,
                want_repos: vec![],
            },
        ];

        for tc in cases {
            let mut cfg = new_config();
            if tc.group == "backend" {
                create(
                    &mut cfg,
                    "backend",
                    tc.initial.iter().map(|s| s.to_string()).collect(),
                )
                .unwrap();
            }

            let result = remove_repos(
                &mut cfg,
                tc.group,
                tc.remove.iter().map(|s| s.to_string()).collect(),
            );
            assert_eq!(result.is_err(), tc.want_err, "case: {}", tc.name);

            if !tc.want_err {
                let got = get(&cfg, tc.group).unwrap();
                let want: Vec<String> = tc.want_repos.iter().map(|s| s.to_string()).collect();
                assert_eq!(got, want, "case: {}", tc.name);
            }
        }
    }

    #[test]
    fn test_add_then_remove_sequence() {
        let mut cfg = new_config();
        create(&mut cfg, "backend", vec!["repo-a".into()]).unwrap();

        add_repos(&mut cfg, "backend", vec!["repo-b".into()]).unwrap();
        assert_eq!(
            get(&cfg, "backend").unwrap(),
            vec!["repo-a".to_string(), "repo-b".to_string()]
        );

        remove_repos(&mut cfg, "backend", vec!["repo-a".into()]).unwrap();
        assert_eq!(get(&cfg, "backend").unwrap(), vec!["repo-b".to_string()]);
    }
}
