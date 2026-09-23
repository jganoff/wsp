//! Workspace-owned repository publication, shared by host and isolated adds.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::output::RepoAddResult;
use crate::workspace::{Metadata, WorkspaceRepoRef};
use crate::{filelock, git, giturl, workspace};

pub enum Source<'a> {
    Direct,
    Mirror(&'a Path),
}

fn branch<'a>(meta: &'a Metadata, override_branch: &'a str) -> &'a str {
    if override_branch.is_empty() {
        &meta.branch
    } else {
        override_branch
    }
}

/// Existing members and complete unpublished clones are handled before callers
/// do any global registration, mirror fetch, discovery, or setup work.
pub fn member(ws: &Path, identity: &str, requested_branch: &str) -> Result<Option<RepoAddResult>> {
    member_with_lock_state(ws, identity, requested_branch, false)
}

fn member_with_lock_state(
    ws: &Path,
    identity: &str,
    requested_branch: &str,
    lock_held: bool,
) -> Result<Option<RepoAddResult>> {
    // The disabled crash-barrier macro intentionally does not evaluate its
    // arguments, while this helper's lock state remains part of its API.
    let _ = lock_held;
    let meta = workspace::load_metadata(ws)?;
    if !meta.repos.contains_key(identity) {
        return Ok(None);
    }
    let path = ws.join(meta.dir_name(identity)?);
    validate_clone(ws, &path, identity, None)?;
    if !requested_branch.is_empty() {
        let stored = meta
            .repos
            .get(identity)
            .and_then(Option::as_ref)
            .map(|r| r.r#ref.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(&meta.branch);
        if stored != requested_branch {
            bail!(
                "{} is already a workspace member with branch intent {:?}, not {:?}",
                identity,
                stored,
                requested_branch
            );
        }
    }
    let mut result = RepoAddResult::pending(identity);
    result.path = path.display().to_string();
    result.clone = "already_present".into();
    result.membership = "unchanged".into();
    crate::crash_barrier!(
        crate::crash_barrier::Operation::Add,
        identity,
        crate::crash_barrier::Point::MemberAlreadyPresent,
        lock_held,
    )?;
    Ok(Some(result))
}

fn save_membership(ws: &Path, metadata: &Metadata, _identity: &str) -> Result<()> {
    workspace::save_metadata_before_persist(ws, metadata, || {
        crate::crash_barrier!(
            crate::crash_barrier::Operation::Add,
            _identity,
            crate::crash_barrier::Point::MetadataReplacePending,
            true,
        )
    })
}

pub fn existing(
    ws: &Path,
    identity: &str,
    requested_branch: &str,
) -> Result<Option<RepoAddResult>> {
    let _lock =
        filelock::FileLock::acquire(&ws.join(workspace::METADATA_FILE), Duration::from_secs(30))?;
    let mut meta = workspace::load_metadata(ws)?;
    if let Some(result) = member_with_lock_state(ws, identity, requested_branch, true)? {
        return Ok(Some(result));
    }
    let name = allocate_name(&meta, identity)?;
    let path = ws.join(&name);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            validate_clone(ws, &path, identity, Some(branch(&meta, requested_branch)))?;
            crate::crash_barrier!(
                crate::crash_barrier::Operation::Add,
                identity,
                crate::crash_barrier::Point::AdoptionValidated,
                true,
            )?;
            record(&mut meta, identity, &name, requested_branch);
            let mut result = RepoAddResult::pending(identity);
            result.path = path.display().to_string();
            result.clone = "adopted".into();
            match save_membership(ws, &meta, identity) {
                Ok(()) => {
                    crate::crash_barrier!(
                        crate::crash_barrier::Operation::Add,
                        identity,
                        crate::crash_barrier::Point::MembershipCommitted,
                        true,
                    )?;
                    result.membership = "updated".into();
                }
                Err(e) => {
                    result.membership = "failed".into();
                    result.error = Some(e.to_string());
                }
            }
            Ok(Some(result))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Existing mappings are fixed. Only the new member gets a disambiguated name.
fn allocate_name(meta: &Metadata, identity: &str) -> Result<String> {
    let parsed = giturl::Parsed::from_identity(identity)?;
    let used: BTreeSet<String> = meta
        .repos
        .keys()
        .map(|id| meta.dir_name(id).map(|s| s.to_lowercase()))
        .collect::<Result<_>>()?;
    let owner_name = format!("{}-{}", parsed.owner.replace('/', "-"), parsed.repo);
    let host_name = format!("{}-{}", parsed.host, owner_name);
    for name in [&parsed.repo, &owner_name, &host_name] {
        workspace::validate_dir_name(name)?;
        if !used.contains(&name.to_lowercase()) {
            return Ok(name.clone());
        }
    }
    for n in 2.. {
        let name = format!("{}-{}", host_name, n);
        if !used.contains(&name.to_lowercase()) {
            return Ok(name);
        }
    }
    unreachable!()
}

fn record(meta: &mut Metadata, identity: &str, name: &str, requested_branch: &str) {
    meta.dirs.insert(identity.into(), name.into());
    meta.repos.insert(
        identity.into(),
        if requested_branch.is_empty() {
            None
        } else {
            Some(WorkspaceRepoRef {
                r#ref: requested_branch.into(),
                url: None,
            })
        },
    );
}

pub fn validate_clone(
    ws: &Path,
    path: &Path,
    identity: &str,
    expected_branch: Option<&str>,
) -> Result<()> {
    validate_deletion_target(ws, path)?;
    let urls = git::run_sanitized(
        Some(path),
        &["config", "--local", "--get-all", "remote.origin.url"],
    )?;
    let values: Vec<_> = urls.lines().collect();
    if values.len() != 1 || giturl::parse(values[0])?.identity() != identity {
        bail!(
            "{} has an origin that does not uniquely match {}",
            path.display(),
            identity
        );
    }
    if let Some(expected) = expected_branch {
        let actual =
            git::run_sanitized(Some(path), &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        if actual != expected {
            bail!(
                "{} is on branch {:?}, expected {:?}; existing checkout was preserved",
                path.display(),
                actual,
                expected
            );
        }
    }
    Ok(())
}

/// Validate the structural properties required before recursively deleting a
/// workspace member. A clone's remote is intentionally excluded: clones are
/// the developer's space and may have a user-selected origin.
pub fn validate_deletion_target(ws: &Path, path: &Path) -> Result<()> {
    let entry = fs::symlink_metadata(path).with_context(|| {
        format!(
            "workspace clone {} is missing or inaccessible",
            path.display()
        )
    })?;
    if !entry.is_dir() || entry.file_type().is_symlink() {
        bail!("{} is not an ordinary clone directory", path.display());
    }
    if !path.canonicalize()?.starts_with(ws.canonicalize()?) {
        bail!("clone {} escapes workspace", path.display());
    }
    let git_dir = path.join(".git");
    let git_entry = fs::symlink_metadata(&git_dir)?;
    if !git_entry.is_dir() || git_entry.file_type().is_symlink() {
        bail!(
            "{} must have a local .git directory; linked worktrees are not portable",
            path.display()
        );
    }
    let common_dir = git_dir.join("commondir");
    match fs::read_to_string(&common_dir) {
        Ok(value) if !value.trim().is_empty() => bail!(
            "{} uses a shared Git common directory; portable workspace clones must keep Git storage local",
            path.display()
        ),
        Ok(_) => bail!(
            "{} has an invalid empty Git common-directory declaration",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    for component in [git_dir.join("objects"), git_dir.join("refs")] {
        if !component.canonicalize()?.starts_with(path.canonicalize()?) {
            bail!("Git storage escapes clone {}", path.display());
        }
    }
    match fs::read_to_string(git_dir.join("objects/info/alternates")) {
        Ok(value) if !value.trim().is_empty() => bail!(
            "{} depends on external Git object alternates",
            path.display()
        ),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn clone_direct(url: &str, dest: &Path, branch: &str) -> Result<()> {
    git::run(
        None,
        &["clone", "--no-local", "--", url, &dest.to_string_lossy()],
    )?;
    if git::run(Some(dest), &["symbolic-ref", "--quiet", "--short", "HEAD"])? == branch {
        return Ok(());
    }
    let remote = format!("origin/{}", branch);
    if git::ref_exists(dest, &format!("refs/remotes/{}", remote)) {
        git::checkout_new_branch_tracking(dest, branch, &remote)?;
    } else if git::ref_exists(dest, "HEAD") {
        git::checkout_new_branch(dest, branch, "HEAD")?;
    } else {
        git::checkout_orphan(dest, branch)?;
    }
    Ok(())
}

/// Atomically publish a complete clone without ever replacing a destination.
pub fn publish_exclusive(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        source,
        rustix::fs::CWD,
        destination,
        rustix::fs::RenameFlags::NOREPLACE,
    )?;
    #[cfg(windows)]
    {
        // Windows rename replaces an empty destination directory. Reject it
        // before asking the platform to move the staged clone, preserving an
        // independently-created workspace path.
        if destination.exists() {
            bail!("destination {} already exists", destination.display());
        }
        fs::rename(source, destination)?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    bail!("exclusive clone publication is not supported on this platform");
    Ok(())
}

pub fn add(
    ws: &Path,
    identity: &str,
    url: &str,
    requested_branch: &str,
    source: Source<'_>,
) -> RepoAddResult {
    let mut result = RepoAddResult::pending(identity);
    let attempt = (|| -> Result<()> {
        if let Some(present) = existing(ws, identity, requested_branch)? {
            result = present;
            return Ok(());
        }
        let snapshot = filelock::read_metadata(ws)?;
        let intended_branch = branch(&snapshot, requested_branch).to_string();
        git::validate_branch_name(&intended_branch)?;
        result.path = ws
            .join(allocate_name(&snapshot, identity)?)
            .display()
            .to_string();
        let staging = tempfile::Builder::new()
            .prefix(".wsp-add-")
            .tempdir_in(ws)?;
        crate::crash_barrier!(
            crate::crash_barrier::Operation::Add,
            identity,
            crate::crash_barrier::Point::StageCreated,
            false,
        )?;
        let staged = staging.path().join("clone");
        result.transport = match source {
            Source::Direct => "direct",
            Source::Mirror(_) => "mirror",
        }
        .into();
        match source {
            Source::Direct => clone_direct(url, &staged, &intended_branch)?,
            Source::Mirror(mirrors) => workspace::clone_from_mirror(
                mirrors,
                staging.path(),
                identity,
                "clone",
                &intended_branch,
                url,
                true,
            )?,
        }
        validate_clone(ws, &staged, identity, Some(&intended_branch))?;
        crate::crash_barrier!(
            crate::crash_barrier::Operation::Add,
            identity,
            crate::crash_barrier::Point::CloneStaged,
            false,
        )?;
        let _lock = filelock::FileLock::acquire(
            &ws.join(workspace::METADATA_FILE),
            Duration::from_secs(30),
        )?;
        let mut meta = workspace::load_metadata(ws)?;
        if branch(&meta, requested_branch) != intended_branch {
            bail!("workspace branch changed during clone; retry the add");
        }
        if meta.repos.contains_key(identity) {
            let path = ws.join(meta.dir_name(identity)?);
            validate_clone(ws, &path, identity, None)?;
            let stored_branch = meta
                .repos
                .get(identity)
                .and_then(Option::as_ref)
                .map(|r| r.r#ref.as_str())
                .filter(|s| !s.is_empty())
                .unwrap_or(&meta.branch);
            if stored_branch != intended_branch {
                bail!(
                    "concurrent add used different branch intent for {}",
                    identity
                );
            }
            result.path = path.display().to_string();
            result.clone = "already_present".into();
            result.membership = "unchanged".into();
            crate::crash_barrier!(
                crate::crash_barrier::Operation::Add,
                identity,
                crate::crash_barrier::Point::MemberAlreadyPresent,
                true,
            )?;
            return Ok(());
        }
        let name = allocate_name(&meta, identity)?;
        let destination: PathBuf = ws.join(&name);
        result.path = destination.display().to_string();
        let destination_exists = match fs::symlink_metadata(&destination) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(e.into()),
        };
        crate::crash_barrier!(
            crate::crash_barrier::Operation::Add,
            identity,
            crate::crash_barrier::Point::AddRechecked,
            true,
        )?;
        if destination_exists {
            validate_clone(ws, &destination, identity, Some(&intended_branch))?;
            crate::crash_barrier!(
                crate::crash_barrier::Operation::Add,
                identity,
                crate::crash_barrier::Point::AdoptionValidated,
                true,
            )?;
            result.clone = "adopted".into();
        } else {
            publish_exclusive(&staged, &destination)
                .context("publishing clone without replacement")?;
            crate::crash_barrier!(
                crate::crash_barrier::Operation::Add,
                identity,
                crate::crash_barrier::Point::ClonePublished,
                true,
            )?;
            result.clone = "created".into();
        }
        record(&mut meta, identity, &name, requested_branch);
        result.membership = "failed".into();
        save_membership(ws, &meta, identity)?;
        crate::crash_barrier!(
            crate::crash_barrier::Operation::Add,
            identity,
            crate::crash_barrier::Point::MembershipCommitted,
            true,
        )?;
        result.membership = "updated".into();
        Ok(())
    })();
    if let Err(e) = attempt {
        if result.clone == "not_attempted" {
            result.clone = "failed".into();
        }
        result.error = Some(format!("{e:#}"));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn workspace_fixture() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let meta = Metadata {
            version: 0,
            name: "mounted".into(),
            branch: "feature".into(),
            repos: BTreeMap::new(),
            created: chrono::Utc::now(),
            description: None,
            last_used: None,
            created_from: None,
            dirs: BTreeMap::new(),
            config: None,
            setup_commands: BTreeMap::new(),
        };
        workspace::save_metadata(temp.path(), &meta).unwrap();
        temp
    }

    fn existing_clone(ws: &Path, name: &str, origin: &str, branch: &str) -> PathBuf {
        let dest = ws.join(name);
        fs::create_dir(&dest).unwrap();
        git::run(Some(&dest), &["init"]).unwrap();
        git::run(
            Some(&dest),
            &["symbolic-ref", "HEAD", &format!("refs/heads/{branch}")],
        )
        .unwrap();
        crate::testutil::local_commit(&dest, "tracked", "original");
        git::run(Some(&dest), &["remote", "add", "origin", origin]).unwrap();
        dest
    }

    #[test]
    fn exclusive_publication_never_replaces_existing_directory() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let dest = temp.path().join("destination");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("owned"), "owned").unwrap();
        for occupied in [false, true] {
            fs::create_dir(&dest).unwrap();
            if occupied {
                fs::write(dest.join("peer"), "peer").unwrap();
            }
            assert!(publish_exclusive(&source, &dest).is_err());
            assert!(source.join("owned").exists());
            assert!(!dest.join("owned").exists());
            if occupied {
                assert_eq!(fs::read_to_string(dest.join("peer")).unwrap(), "peer");
            }
            fs::remove_dir_all(&dest).unwrap();
        }
        publish_exclusive(&source, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("owned")).unwrap(), "owned");
    }

    #[test]
    fn published_clone_is_adopted_and_host_retry_preserves_dirty_checkout() {
        let ws = workspace_fixture();
        let identity = "test.local/alice/api";
        let clone = existing_clone(ws.path(), "api", "git@test.local:alice/api.git", "feature");
        fs::write(clone.join("tracked"), "uncommitted").unwrap();
        let config = fs::read(clone.join(".git/config")).unwrap();
        let adopted = existing(ws.path(), identity, "").unwrap().unwrap();
        assert_eq!(adopted.clone, "adopted");
        assert_eq!(adopted.membership, "updated");
        // No mirror is needed for an idempotent retry, including after checkout
        // changes made by the developer in their ordinary clone.
        git::run(Some(&clone), &["checkout", "-b", "developer-branch"]).unwrap();
        let result = add(
            ws.path(),
            identity,
            "https://test.local/alice/api.git",
            "",
            Source::Mirror(&ws.path().join("missing-global-mirrors")),
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.clone, "already_present");
        assert_eq!(fs::read(clone.join(".git/config")).unwrap(), config);
        assert_eq!(
            fs::read_to_string(clone.join("tracked")).unwrap(),
            "uncommitted"
        );
        assert_eq!(git::branch_current(&clone).unwrap(), "developer-branch");
        assert!(!ws.path().join("missing-global-mirrors").exists());
    }

    #[test]
    fn collisions_keep_existing_mappings_and_adoption_rejects_mismatches() {
        let ws = workspace_fixture();
        existing_clone(ws.path(), "api", "git@test.local:alice/api.git", "feature");
        existing(ws.path(), "test.local/alice/api", "")
            .unwrap()
            .unwrap();
        let bob = existing_clone(
            ws.path(),
            "bob-api",
            "git@test.local:bob/api.git",
            "wrong-branch",
        );
        assert!(existing(ws.path(), "test.local/bob/api", "").is_err());
        assert_eq!(git::branch_current(&bob).unwrap(), "wrong-branch");
        git::run(Some(&bob), &["checkout", "-b", "feature"]).unwrap();
        existing(ws.path(), "test.local/bob/api", "")
            .unwrap()
            .unwrap();
        let meta = workspace::load_metadata(ws.path()).unwrap();
        assert_eq!(meta.dir_name("test.local/alice/api").unwrap(), "api");
        assert_eq!(meta.dir_name("test.local/bob/api").unwrap(), "bob-api");
        assert!(ws.path().join("api/tracked").exists());
    }

    #[test]
    fn missing_or_mismatched_member_blocks_retry() {
        for origin in [None, Some("git@test.local:mallory/api.git")] {
            let ws = workspace_fixture();
            filelock::with_metadata(ws.path(), |meta| {
                meta.repos.insert("test.local/alice/api".into(), None);
                Ok(())
            })
            .unwrap();
            if let Some(url) = origin {
                existing_clone(ws.path(), "api", url, "feature");
            }
            assert!(existing(ws.path(), "test.local/alice/api", "").is_err());
            assert_eq!(workspace::load_metadata(ws.path()).unwrap().repos.len(), 1);
        }
    }

    #[test]
    fn direct_clone_handles_fresh_tracked_and_empty_branches() {
        for (populated, remote_branch, requested) in [
            (true, "main", "feature"),
            (true, "feature", "feature"),
            (false, "main", "feature"),
        ] {
            let ws = workspace_fixture();
            let source = tempfile::tempdir().unwrap();
            git::run(Some(source.path()), &["init"]).unwrap();
            git::run(
                Some(source.path()),
                &[
                    "symbolic-ref",
                    "HEAD",
                    &format!("refs/heads/{remote_branch}"),
                ],
            )
            .unwrap();
            if populated {
                crate::testutil::local_commit(source.path(), "tracked", "source");
            }
            let dest = ws.path().join("api");
            clone_direct(source.path().to_str().unwrap(), &dest, requested).unwrap();
            git::remote_set_url(&dest, "origin", "git@test.local:alice/api.git").unwrap();
            validate_clone(ws.path(), &dest, "test.local/alice/api", Some(requested)).unwrap();
            if populated {
                assert_eq!(fs::read_to_string(dest.join("tracked")).unwrap(), "source");
                let upstream = git::run(Some(&dest), &["rev-parse", "--abbrev-ref", "@{upstream}"]);
                if remote_branch == requested {
                    assert_eq!(upstream.unwrap(), "origin/feature");
                } else {
                    assert!(upstream.is_err(), "fresh branch must not track origin/main");
                }
            }
        }
    }
}
