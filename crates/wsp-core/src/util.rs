use std::io::{BufRead, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};

/// Maximum size for YAML files (1 MiB). Any config, metadata, template,
/// or gc entry file larger than this is rejected before deserialization.
pub(crate) const MAX_YAML_BYTES: u64 = 1_048_576;

/// Read a file to string, rejecting files larger than `MAX_YAML_BYTES`.
/// Uses `Read::take()` to enforce the limit in a single pass, avoiding
/// a TOCTOU gap between a metadata check and the actual read.
pub(crate) fn read_yaml_file(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut buf = String::new();
    let bytes_read = file
        .take(MAX_YAML_BYTES + 1)
        .read_to_string(&mut buf)
        .with_context(|| format!("reading {}", path.display()))?;
    if bytes_read as u64 > MAX_YAML_BYTES {
        bail!(
            "{} is too large (>{} bytes)",
            path.display(),
            MAX_YAML_BYTES
        );
    }
    Ok(buf)
}

pub(crate) fn read_stdin_line() -> String {
    let stdin = std::io::stdin();
    let mut line = String::new();
    if let Err(e) = stdin.lock().read_line(&mut line) {
        eprintln!("warning: failed to read stdin: {}", e);
    }
    line
}

/// Total size of every file under `path`, following no symlinks.
///
/// `DirEntry::file_type()` does not follow them, so a symlink counts as its own
/// metadata size rather than its target's. That keeps the walk from escaping the
/// directory or looping on a cycle.
///
/// Unreadable entries count as zero: a size report is not the place to fail.
pub fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            total += if file_type.is_dir() {
                dir_size(&entry.path())
            } else {
                entry.metadata().map(|m| m.len()).unwrap_or(0)
            };
        }
    }
    total
}

#[cfg(test)]
mod tests {
    #[test]
    fn dir_size_sums_files_and_subdirectories() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(super::dir_size(tmp.path()), 0, "an empty tree is zero");

        std::fs::write(tmp.path().join("a"), vec![0u8; 100]).unwrap();
        std::fs::create_dir(tmp.path().join("sub")).unwrap();
        std::fs::write(tmp.path().join("sub/b"), vec![0u8; 250]).unwrap();
        assert_eq!(super::dir_size(tmp.path()), 350);
    }

    /// The walk must not follow symlinks: doing so lets it escape the directory
    /// it was asked about, double count, or loop forever on a cycle.
    #[cfg(unix)]
    #[test]
    fn dir_size_counts_a_symlink_not_its_target() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("big"), vec![0u8; 100_000]).unwrap();

        let measured = tmp.path().join("measured");
        std::fs::create_dir(&measured).unwrap();
        std::os::unix::fs::symlink(outside.join("big"), measured.join("link")).unwrap();

        let size = super::dir_size(&measured);
        assert!(
            size < 1_000,
            "followed the symlink and counted its 100KB target: {size}"
        );
    }

    /// A cycle would hang the walk if symlinks were followed.
    #[cfg(unix)]
    #[test]
    fn dir_size_terminates_on_a_symlink_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("loop");
        std::fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(&dir, dir.join("self")).unwrap();
        let _ = super::dir_size(&dir);
    }

    use super::*;

    #[test]
    fn test_read_yaml_file_ok() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "key: value\n").unwrap();
        let content = read_yaml_file(tmp.path()).unwrap();
        assert_eq!(content, "key: value\n");
    }

    #[test]
    fn test_read_yaml_file_rejects_oversized() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let big = vec![b'x'; (MAX_YAML_BYTES + 1) as usize];
        std::fs::write(tmp.path(), &big).unwrap();
        let err = read_yaml_file(tmp.path()).unwrap_err();
        assert!(err.to_string().contains("too large"), "{}", err);
    }

    #[test]
    fn test_read_yaml_file_accepts_exactly_max() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let exact = vec![b'x'; MAX_YAML_BYTES as usize];
        std::fs::write(tmp.path(), &exact).unwrap();
        let content = read_yaml_file(tmp.path()).unwrap();
        assert_eq!(content.len(), MAX_YAML_BYTES as usize);
    }

    #[test]
    fn test_read_yaml_file_missing() {
        let result = read_yaml_file(Path::new("/nonexistent/file.yaml"));
        assert!(result.is_err());
    }
}
