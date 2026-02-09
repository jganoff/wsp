use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git;
use crate::giturl::Parsed;

pub fn dir(mirrors_dir: &Path, parsed: &Parsed) -> PathBuf {
    mirrors_dir.join(parsed.mirror_path())
}

pub fn clone(mirrors_dir: &Path, parsed: &Parsed, url: &str) -> Result<()> {
    let dest = dir(mirrors_dir, parsed);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    git::clone_bare(url, &dest)?;
    git::configure_fetch_refspec(&dest)
}

pub fn fetch(mirrors_dir: &Path, parsed: &Parsed) -> Result<()> {
    let d = dir(mirrors_dir, parsed);
    git::fetch(&d)
}

pub fn remove(mirrors_dir: &Path, parsed: &Parsed) -> Result<()> {
    let d = dir(mirrors_dir, parsed);
    fs::remove_dir_all(d)?;
    Ok(())
}

pub fn exists(mirrors_dir: &Path, parsed: &Parsed) -> bool {
    dir(mirrors_dir, parsed).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn create_test_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().to_str().unwrap();
        let cmds: Vec<Vec<&str>> = vec![
            vec!["git", "init", "--initial-branch=main"],
            vec!["git", "config", "user.email", "test@test.com"],
            vec!["git", "config", "user.name", "Test"],
            vec!["git", "config", "commit.gpgsign", "false"],
            vec!["git", "commit", "--allow-empty", "-m", "initial"],
        ];
        for args in cmds {
            let output = Command::new(args[0])
                .args(&args[1..])
                .current_dir(d)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "command {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        tmp
    }

    #[test]
    fn test_clone_and_exists() {
        let tmp_data = tempfile::tempdir().unwrap();
        let mirrors_dir = tmp_data.path().join("mirrors");

        let repo = create_test_repo();
        let parsed = Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "test-repo".into(),
        };

        clone(&mirrors_dir, &parsed, repo.path().to_str().unwrap()).unwrap();

        assert!(exists(&mirrors_dir, &parsed));

        let d = dir(&mirrors_dir, &parsed);
        assert!(d.exists());

        let refspec = git::run(Some(&d), &["config", "--get", "remote.origin.fetch"]).unwrap();
        assert_eq!(refspec, "+refs/heads/*:refs/remotes/origin/*");
    }

    #[test]
    fn test_fetch() {
        let tmp_data = tempfile::tempdir().unwrap();
        let mirrors_dir = tmp_data.path().join("mirrors");

        let repo = create_test_repo();
        let parsed = Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "test-repo".into(),
        };

        clone(&mirrors_dir, &parsed, repo.path().to_str().unwrap()).unwrap();

        // Remove refspec to simulate a pre-fix bare clone
        let d = dir(&mirrors_dir, &parsed);
        git::run(Some(&d), &["config", "--unset", "remote.origin.fetch"]).unwrap();
        assert!(git::run(Some(&d), &["config", "--get", "remote.origin.fetch"]).is_err());

        // Fetch should auto-configure the missing refspec
        fetch(&mirrors_dir, &parsed).unwrap();

        let refspec = git::run(Some(&d), &["config", "--get", "remote.origin.fetch"]).unwrap();
        assert_eq!(refspec, "+refs/heads/*:refs/remotes/origin/*");
    }

    #[test]
    fn test_remove() {
        let tmp_data = tempfile::tempdir().unwrap();
        let mirrors_dir = tmp_data.path().join("mirrors");

        let repo = create_test_repo();
        let parsed = Parsed {
            host: "test.local".into(),
            owner: "user".into(),
            repo: "test-repo".into(),
        };

        clone(&mirrors_dir, &parsed, repo.path().to_str().unwrap()).unwrap();
        assert!(exists(&mirrors_dir, &parsed));

        remove(&mirrors_dir, &parsed).unwrap();
        assert!(!exists(&mirrors_dir, &parsed));
    }

    #[test]
    fn test_dir() {
        let mirrors_dir = Path::new("/data/ws/mirrors");
        let parsed = Parsed {
            host: "github.com".into(),
            owner: "user".into(),
            repo: "repo-a".into(),
        };
        let d = dir(mirrors_dir, &parsed);
        assert_eq!(
            d,
            PathBuf::from("/data/ws/mirrors/github.com/user/repo-a.git")
        );
    }
}
