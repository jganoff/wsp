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
}
