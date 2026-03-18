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

    #[test]
    fn test_parse_gh_login_output_success() {
        assert_eq!(
            parse_gh_login_output("octocat\n", true),
            Some("octocat".to_string())
        );
    }

    #[test]
    fn test_parse_gh_login_output_trims_whitespace() {
        assert_eq!(
            parse_gh_login_output("  myuser  \n", true),
            Some("myuser".to_string())
        );
    }

    #[test]
    fn test_parse_gh_login_output_failure() {
        assert_eq!(parse_gh_login_output("octocat\n", false), None);
    }

    #[test]
    fn test_parse_gh_login_output_empty() {
        assert_eq!(parse_gh_login_output("", true), None);
        assert_eq!(parse_gh_login_output("\n", true), None);
        assert_eq!(parse_gh_login_output("  ", true), None);
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

/// Returns the currently signed-in GitHub username by running `gh api user -q .login`.
/// Returns `None` if `gh` is not installed, not authenticated, or the call fails.
pub(crate) fn gh_login() -> Option<String> {
    let out = std::process::Command::new("gh")
        .args(["api", "user", "-q", ".login"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let login = String::from_utf8(out.stdout).ok()?;
    let login = login.trim().to_string();
    if login.is_empty() { None } else { Some(login) }
}

/// Parse the output of `gh api user -q .login` into a login string.
/// Exposed for testing; production code calls `gh_login()` directly.
pub(crate) fn parse_gh_login_output(raw: &str, success: bool) -> Option<String> {
    if !success {
        return None;
    }
    let login = raw.trim().to_string();
    if login.is_empty() { None } else { Some(login) }
}
