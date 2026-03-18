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

#[cfg(test)]
mod tests {
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

pub(crate) fn read_stdin_line() -> String {
    let stdin = std::io::stdin();
    let mut line = String::new();
    if let Err(e) = stdin.lock().read_line(&mut line) {
        eprintln!("warning: failed to read stdin: {}", e);
    }
    line
}
