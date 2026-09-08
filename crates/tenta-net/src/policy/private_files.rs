//! Owner-only creation of the files this crate stores on disk.
//!
//! The session cache holds TLS resumption secrets and the HTTP cache holds
//! whole response bodies, including responses to `Authorization`-bearing
//! requests (`docs/spec/19-security.md` §7). Under the usual `umask 022` both
//! would be world-readable, so every directory this crate creates is `0700`
//! and every file `0600`, the way Chrome's profile directory is.
//!
//! On Windows there is no mode: the stores live under `%LOCALAPPDATA%`, whose
//! ACL already grants the owning user only, and tightening a per-file DACL
//! needs a Win32 dependency this crate does not carry. That is a documented
//! platform gap, asserted nowhere and claimed nowhere.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::Path;

/// Creates `dir` and its parents, then makes sure only the owner can enter it.
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(dir)?;
        // A directory that already existed keeps its old mode, so tighten it.
        harden(dir, 0o700)
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
    }
}

/// `OpenOptions` that create files no other user can read.
pub fn private_file() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

/// Tightens an existing path's permissions; a no-op off unix.
pub fn harden(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let current = fs::metadata(path)?.permissions().mode() & 0o7777;
        if current != mode {
            fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
}

/// Tightens a file this process opened, whether or not it created it.
pub fn harden_file(path: &Path) -> io::Result<()> {
    harden(path, 0o600)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o7777
    }

    #[test]
    fn directories_are_owner_only_even_when_they_already_existed() {
        let temp = tenta_testutil::temp_dir("private-files");
        let dir = temp.path().join("nested/store");
        // A directory left behind by an older build with the usual umask.
        fs::create_dir_all(&dir).expect("pre-existing dir");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("loose mode");

        create_private_dir(&dir).expect("create");
        assert_eq!(mode_of(&dir), 0o700);
    }

    #[test]
    fn files_are_created_readable_by_their_owner_only() {
        let temp = tenta_testutil::temp_dir("private-files");
        let path = temp.path().join("secret.bin");
        private_file()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .expect("create");
        assert_eq!(mode_of(&path), 0o600);

        // An existing file with a loose mode is tightened when reopened.
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("loose mode");
        harden_file(&path).expect("harden");
        assert_eq!(mode_of(&path), 0o600);
    }
}
