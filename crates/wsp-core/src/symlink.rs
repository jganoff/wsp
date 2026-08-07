//! Cross-platform symlink helpers.
//!
//! Unless Developer Mode is enabled (or the process is elevated), Windows
//! refuses to create a symlink with `ERROR_PRIVILEGE_NOT_HELD` (os error 1314).
//! These helpers centralize the "degrade gracefully when symlinks are
//! unavailable" policy so callers don't each re-implement it. `gc` handles its
//! own file-vs-directory symlink replication inline (the only caller that needs
//! it) and uses `is_dev_mode_error` to decide whether to skip.

use std::io;
use std::path::Path;

/// Windows `ERROR_PRIVILEGE_NOT_HELD`: creating a symlink requires Developer
/// Mode or an elevated process.
const ERROR_PRIVILEGE_NOT_HELD: i32 = 1314;

/// True if `err` is the Windows "symlinks need Developer Mode" error. Always
/// false on Unix, where symlink creation needs no special privilege. The
/// `cfg!(windows)` short-circuits so `raw_os_error()` is only meaningful on
/// Windows.
pub(crate) fn is_dev_mode_error(err: &io::Error) -> bool {
    cfg!(windows) && err.raw_os_error() == Some(ERROR_PRIVILEGE_NOT_HELD)
}

/// Create a symlink at `link` pointing to `original`, where `original` is a file.
pub(crate) fn symlink_file<P: AsRef<Path>, Q: AsRef<Path>>(original: P, link: Q) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(original, link)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(original, link)
    }
}

/// Create a file symlink, treating "Developer Mode not enabled" on Windows as a
/// no-op success. Returns `true` if the link was created, `false` if it was
/// skipped because the platform won't allow it.
pub fn symlink_file_or_skip<P: AsRef<Path>, Q: AsRef<Path>>(
    original: P,
    link: Q,
) -> io::Result<bool> {
    match symlink_file(original, link) {
        Ok(()) => Ok(true),
        Err(e) if is_dev_mode_error(&e) => Ok(false),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn symlink_file_creates_link_where_supported() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("target.txt"), "hi").unwrap();
        let link = tmp.path().join("link.txt");

        match symlink_file_or_skip("target.txt", &link) {
            Ok(true) => {
                let meta = fs::symlink_metadata(&link).unwrap();
                assert!(meta.file_type().is_symlink());
            }
            // Windows without Developer Mode: creation is skipped, link absent.
            Ok(false) => assert!(!link.exists()),
            Err(e) => panic!("unexpected symlink error: {e}"),
        }
    }

    #[test]
    fn is_dev_mode_error_false_for_other_errors() {
        let err = io::Error::new(io::ErrorKind::NotFound, "nope");
        assert!(!is_dev_mode_error(&err));
    }

    /// CI's Windows runners are elevated, so a real 1314 never occurs there and
    /// the skip path is never exercised end-to-end. Synthesize the error to pin
    /// both the constant and the platform gating.
    #[test]
    fn is_dev_mode_error_matches_1314_only_on_windows() {
        let err = io::Error::from_raw_os_error(ERROR_PRIVILEGE_NOT_HELD);
        assert_eq!(is_dev_mode_error(&err), cfg!(windows));

        // A neighbouring os error must never be mistaken for it.
        let other = io::Error::from_raw_os_error(ERROR_PRIVILEGE_NOT_HELD + 1);
        assert!(!is_dev_mode_error(&other));
    }
}
