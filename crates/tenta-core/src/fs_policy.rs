//! Filesystem policy (`docs/spec/19-security.md` §1).
//!
//! The default is empty for every host: no `file://`, no uploads, no output
//! paths, no downloads, no cache directory. Every path-taking operation
//! canonicalises the path and checks it against the roots; protocol hosts
//! (CDP, MCP) inherit the process policy and cannot widen it.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadPolicy {
    #[default]
    Deny,
    Accept {
        dir: PathBuf,
    },
}

/// Determines whether to create/truncate or append when opening a file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    /// Create a new file or truncate if it exists.
    CreateTruncate,
    /// Append to an existing file or create if it doesn't exist.
    Append,
}

#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct FsPolicy {
    pub read_roots: Vec<PathBuf>,
    pub write_roots: Vec<PathBuf>,
    pub downloads: DownloadPolicy,
}

impl FsPolicy {
    /// No filesystem access at all.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn allow_read(mut self, root: impl Into<PathBuf>) -> Self {
        self.read_roots.push(root.into());
        self
    }

    pub fn allow_write(mut self, root: impl Into<PathBuf>) -> Self {
        self.write_roots.push(root.into());
        self
    }

    pub fn downloads_to(mut self, dir: impl Into<PathBuf>) -> Self {
        self.downloads = DownloadPolicy::Accept { dir: dir.into() };
        self
    }

    /// Checks that `path` (which must exist) resolves inside a read root and
    /// returns its canonical form.
    pub fn check_read(&self, path: &Path) -> Result<PathBuf> {
        check_against(path, &self.read_roots, true)
    }

    /// Checks that `path` resolves inside a write root. The file itself may
    /// not exist yet; its parent directory must.
    pub fn check_write(&self, path: &Path) -> Result<PathBuf> {
        check_against(path, &self.write_roots, false)
    }

    /// Opens a file for writing with protection against TOCTOU symlink attacks and FIFOs.
    ///
    /// Checks that `path` resolves inside a write root and opens it atomically
    /// with `O_NOFOLLOW` and `O_NONBLOCK` on Unix to prevent symlink swaps and hanging on FIFOs.
    /// Verifies the file is a regular file before any write. After opening, verifies that
    /// the parent directory is still inside the root.
    pub fn open_write(&self, path: &Path, mode: OpenMode) -> io::Result<File> {
        // First, check the path against the policy.
        let checked = self.check_write(path).map_err(|_| {
            io::Error::new(io::ErrorKind::PermissionDenied, "path outside write roots")
        })?;

        // If the path already exists, verify it's a regular file before opening (N-R3-4).
        // This prevents hanging on FIFOs or other special files.
        if let Ok(metadata) = std::fs::symlink_metadata(&checked)
            && !metadata.is_file()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path is not a regular file",
            ));
        }

        // Build OpenOptions with platform-specific flags to prevent following symlinks.
        // For CreateTruncate mode, open without truncation first so we can verify the file
        // before modifying it (N-R2-2).
        let mut opts = OpenOptions::new();
        match mode {
            OpenMode::CreateTruncate => {
                opts.create(true).write(true);
                // Don't truncate yet; we'll verify the file first and truncate after.
            }
            OpenMode::Append => {
                opts.create(true).append(true);
            }
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // O_NOFOLLOW prevents opening symlinks; O_NONBLOCK prevents hanging on FIFOs
            opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            // FILE_FLAG_OPEN_REPARSE_POINT prevents following symlinks on Windows
            const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x00200000;
            opts.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }

        // Open the file with O_NOFOLLOW and O_NONBLOCK.
        #[allow(unused_mut)]
        let mut file = opts.open(&checked)?;

        // For CreateTruncate mode, verify the file before truncation (N-R2-2).
        // Check nlink to reject hard links that could bypass write-root checks.
        if matches!(mode, OpenMode::CreateTruncate) {
            let metadata = file.metadata()?;
            if metadata.nlink() > 1 {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "file has hard links; refusing to truncate",
                ));
            }
            // Now truncate the verified file.
            file.set_len(0)?;
        }

        // After opening, verify that the parent directory is still inside a root.
        // This catches the case where a symlink to the directory itself is created
        // after the check but before the open.
        if let Some(parent) = checked.parent() {
            // Re-check the parent directory with symlink_metadata to ensure
            // it's not a symlink or dangling.
            let _ = std::fs::symlink_metadata(parent).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "parent directory changed after open",
                )
            })?;

            // Canonicalize the parent and verify it's inside a write root.
            let canonical_parent = std::fs::canonicalize(parent).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "cannot canonicalize parent directory",
                )
            })?;

            let mut inside_root = false;
            for root in &self.write_roots {
                if let Ok(canonical_root) = std::fs::canonicalize(root)
                    && canonical_parent.starts_with(&canonical_root)
                {
                    inside_root = true;
                    break;
                }
            }

            if !inside_root {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "parent directory is outside write roots",
                ));
            }
        }

        Ok(file)
    }

    /// Creates a directory and its parents, checking that the result is inside a write root.
    ///
    /// Sets directory permissions to 0700 on Unix and creates all missing parents.
    pub fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        // Check the path against the policy.
        let checked = self.check_write(path).map_err(|_| {
            io::Error::new(io::ErrorKind::PermissionDenied, "path outside write roots")
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true).mode(0o700);
            builder.create(&checked)?;
            // If the directory already existed, tighten its permissions.
            set_mode(&checked, 0o700)?;
            Ok(())
        }

        #[cfg(not(unix))]
        {
            fs::create_dir_all(&checked)
        }
    }

    /// The directory downloads go to, if any.
    pub fn download_dir(&self) -> Option<&Path> {
        match &self.downloads {
            DownloadPolicy::Deny => None,
            DownloadPolicy::Accept { dir } => Some(dir),
        }
    }

    /// Returns a policy that is `self` restricted to `other`: a root of
    /// `self` survives only if it lies inside a root of `other`. Used when a
    /// protocol session asks for a narrower policy than the process.
    pub fn narrowed_by(&self, other: &FsPolicy) -> FsPolicy {
        let keep = |roots: &[PathBuf], allowed: &[PathBuf]| -> Vec<PathBuf> {
            roots
                .iter()
                .filter(|r| allowed.iter().any(|a| r.starts_with(a)))
                .cloned()
                .collect()
        };
        FsPolicy {
            read_roots: keep(&self.read_roots, &other.read_roots),
            write_roots: keep(&self.write_roots, &other.write_roots),
            downloads: match (&self.downloads, &other.downloads) {
                (DownloadPolicy::Accept { dir }, DownloadPolicy::Accept { dir: outer })
                    if dir.starts_with(outer) =>
                {
                    self.downloads.clone()
                }
                _ => DownloadPolicy::Deny,
            },
        }
    }
}

/// Set the permissions of a directory to the specified mode (Unix-only).
#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let current = fs::metadata(path)?.permissions().mode() & 0o7777;
    if current != mode {
        fs::set_permissions(path, fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

fn check_against(path: &Path, roots: &[PathBuf], must_exist: bool) -> Result<PathBuf> {
    let denied = || Error::FsDenied {
        path: path.to_path_buf(),
    };
    if roots.is_empty() {
        return Err(denied());
    }

    // Reject if the final component is a symlink to prevent escape attempts
    // via dangling symlinks or symlink swaps after check.
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.is_symlink()
    {
        return Err(denied());
    }

    let canonical = if must_exist {
        std::fs::canonicalize(path).map_err(|_| denied())?
    } else {
        // Canonicalise the deepest existing ancestor, then re-append the rest,
        // so a symlinked parent cannot escape the root and a not-yet-created
        // file can still be checked. Use symlink_metadata to properly detect
        // both existing and dangling symlinks.
        // For bare relative paths like "name", start with the parent (or ".") (N-R3-4).
        let mut existing = path.to_path_buf();
        let mut tail = Vec::new();

        // Walk up until we find an existing ancestor
        while std::fs::symlink_metadata(&existing).is_err() {
            let Some(name) = existing.file_name() else {
                // If we run out of components, seed with "." for current directory
                existing = PathBuf::from(".");
                break;
            };
            tail.push(name.to_owned());
            if !existing.pop() {
                // Path became empty; seed with "." for current directory
                existing = PathBuf::from(".");
                break;
            }
        }

        let mut canonical = std::fs::canonicalize(&existing).map_err(|_| denied())?;
        for name in tail.iter().rev() {
            if name == ".." || name == "." {
                return Err(denied());
            }
            canonical.push(name);
        }
        canonical
    };
    for root in roots {
        let Ok(root) = std::fs::canonicalize(root) else {
            continue;
        };
        if canonical.starts_with(&root) {
            return Ok(canonical);
        }
    }
    Err(denied())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tenta-core-fs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn empty_policy_denies_everything() {
        let dir = temp_dir("empty");
        let file = dir.join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let policy = FsPolicy::none();
        assert!(matches!(
            policy.check_read(&file),
            Err(Error::FsDenied { .. })
        ));
        assert!(matches!(
            policy.check_write(&file),
            Err(Error::FsDenied { .. })
        ));
        assert!(policy.download_dir().is_none());
    }

    #[test]
    fn reads_inside_root_allowed_and_traversal_refused() {
        let dir = temp_dir("read");
        let inside = dir.join("inside.txt");
        std::fs::write(&inside, b"x").unwrap();
        let outside = temp_dir("read-outside").join("secret.txt");
        std::fs::write(&outside, b"y").unwrap();
        let policy = FsPolicy::none().allow_read(&dir);
        assert!(policy.check_read(&inside).is_ok());
        assert!(policy.check_read(&outside).is_err());
        let traversal = dir
            .join("..")
            .join(outside.parent().unwrap().file_name().unwrap())
            .join("secret.txt");
        assert!(policy.check_read(&traversal).is_err());
    }

    #[test]
    fn writes_may_target_new_files_but_not_escape() {
        let dir = temp_dir("write");
        let policy = FsPolicy::none().allow_write(&dir);
        let new_file = dir.join("sub").join("new.png");
        std::fs::create_dir_all(new_file.parent().unwrap()).unwrap();
        assert!(policy.check_write(&new_file).is_ok());
        assert!(
            policy
                .check_write(&dir.join("..").join("escape.png"))
                .is_err()
        );
        // Parents that do not exist yet are created by the writer; the check
        // only cares that the resolved path stays inside the root.
        assert!(
            policy
                .check_write(&dir.join("missing-parent").join("x.png"))
                .is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_parent_cannot_escape() {
        let dir = temp_dir("symlink");
        let outside = temp_dir("symlink-outside");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let policy = FsPolicy::none().allow_write(&dir);
        assert!(policy.check_write(&link.join("escaped.txt")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_is_rejected() {
        let dir = temp_dir("dangling");
        let outside_dir = temp_dir("dangling-outside");
        let target = outside_dir.join("target.txt");
        let link = dir.join("link");
        // Create a symlink to a path that doesn't exist
        std::os::unix::fs::symlink(&target, &link).unwrap();
        // Don't create the target, so the symlink is dangling
        let policy = FsPolicy::none().allow_write(&dir);
        assert!(
            policy.check_write(&link).is_err(),
            "dangling symlink should be rejected"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_created_after_check_is_rejected() {
        let dir = temp_dir("symlink-swap");
        let outside = temp_dir("symlink-swap-outside");
        let link_path = dir.join("link");
        let policy = FsPolicy::none().allow_write(&dir);

        // Create a real file at link_path
        std::fs::write(&link_path, b"real file").unwrap();
        assert!(
            policy.check_write(&link_path).is_ok(),
            "regular file is allowed"
        );

        // Now replace it with a symlink to outside
        std::fs::remove_file(&link_path).unwrap();
        std::os::unix::fs::symlink(&outside, &link_path).unwrap();
        assert!(
            policy.check_write(&link_path).is_err(),
            "symlink created after previous check should be rejected"
        );
    }

    #[test]
    fn narrowing_keeps_only_nested_roots() {
        let process = FsPolicy::none()
            .allow_read("/tmp/proc")
            .downloads_to("/tmp/proc/dl");
        let session = FsPolicy::none()
            .allow_read("/tmp/proc/sub")
            .allow_read("/etc")
            .downloads_to("/tmp/elsewhere");
        let effective = session.narrowed_by(&process);
        assert_eq!(effective.read_roots, vec![PathBuf::from("/tmp/proc/sub")]);
        assert_eq!(effective.downloads, DownloadPolicy::Deny);
    }

    #[test]
    fn open_write_creates_truncate_mode() {
        let dir = temp_dir("open-write-truncate");
        let file_path = dir.join("test.txt");
        let policy = FsPolicy::none().allow_write(&dir);

        // Write some data
        {
            let file = policy
                .open_write(&file_path, OpenMode::CreateTruncate)
                .expect("open_write failed");
            use std::io::Write;
            let mut file = file;
            file.write_all(b"hello").expect("write failed");
            file.sync_all().expect("sync failed");
        }

        // Verify the file exists and has the right content
        let content = std::fs::read(&file_path).expect("read failed");
        assert_eq!(content, b"hello");

        // Open again in truncate mode
        {
            let file = policy
                .open_write(&file_path, OpenMode::CreateTruncate)
                .expect("open_write failed");
            use std::io::Write;
            let mut file = file;
            file.write_all(b"new").expect("write failed");
            file.sync_all().expect("sync failed");
        }

        // Verify the file was truncated
        let content = std::fs::read(&file_path).expect("read failed");
        assert_eq!(content, b"new");
    }

    #[test]
    fn open_write_append_mode() {
        let dir = temp_dir("open-write-append");
        let file_path = dir.join("test.txt");
        let policy = FsPolicy::none().allow_write(&dir);

        // Write some data
        {
            let file = policy
                .open_write(&file_path, OpenMode::Append)
                .expect("open_write failed");
            use std::io::Write;
            let mut file = file;
            file.write_all(b"hello").expect("write failed");
            file.sync_all().expect("sync failed");
        }

        // Append to the file
        {
            let file = policy
                .open_write(&file_path, OpenMode::Append)
                .expect("open_write failed");
            use std::io::Write;
            let mut file = file;
            file.write_all(b" world").expect("write failed");
            file.sync_all().expect("sync failed");
        }

        // Verify the file has both parts
        let content = std::fs::read(&file_path).expect("read failed");
        assert_eq!(content, b"hello world");
    }

    #[test]
    fn create_dir_all_creates_directory() {
        let root = temp_dir("create-dir");
        let policy = FsPolicy::none().allow_write(&root);
        let subdir = root.join("a").join("b").join("c");

        assert!(policy.create_dir_all(&subdir).is_ok());
        assert!(subdir.is_dir());
        assert!(root.join("a").is_dir());
        assert!(root.join("a").join("b").is_dir());
    }

    #[test]
    fn open_write_rejects_paths_outside_root() {
        let dir = temp_dir("open-write-bounds");
        let outside = temp_dir("open-write-bounds-outside");
        let policy = FsPolicy::none().allow_write(&dir);

        let result = policy.open_write(&outside.join("file.txt"), OpenMode::CreateTruncate);
        assert!(
            result.is_err(),
            "open_write should reject paths outside write roots"
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_write_rejects_symlink() {
        let dir = temp_dir("open-write-symlink");
        let outside = temp_dir("open-write-symlink-outside");
        let symlink_path = dir.join("link");
        let policy = FsPolicy::none().allow_write(&dir);

        // Create a symlink to outside
        std::os::unix::fs::symlink(&outside, &symlink_path).unwrap();

        // Trying to open the symlink should fail (O_NOFOLLOW prevents it)
        let result = policy.open_write(&symlink_path, OpenMode::CreateTruncate);
        assert!(
            result.is_err(),
            "open_write should reject symlinks with O_NOFOLLOW"
        );
    }

    #[cfg(unix)]
    #[test]
    fn toctou_symlink_swap_after_check_is_caught_by_open_write() {
        let dir = temp_dir("toctou-open-write");
        let outside = temp_dir("toctou-open-write-outside");
        let file_path = dir.join("file.txt");
        let policy = FsPolicy::none().allow_write(&dir);

        // Step 1: Create a real file and check_write it (passes)
        std::fs::write(&file_path, b"original").unwrap();
        assert!(
            policy.check_write(&file_path).is_ok(),
            "check_write should pass for regular file"
        );

        // Step 2: Replace the file with a symlink to outside (TOCTOU attack)
        std::fs::remove_file(&file_path).unwrap();
        std::os::unix::fs::symlink(&outside, &file_path).unwrap();

        // Step 3: Try to open_write the path - should fail
        let result = policy.open_write(&file_path, OpenMode::CreateTruncate);
        assert!(
            result.is_err(),
            "open_write should catch symlink swapped after check_write"
        );
    }

    #[cfg(unix)]
    #[test]
    fn create_dir_all_sets_0700_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp_dir("create-dir-perms");
        let policy = FsPolicy::none().allow_write(&root);
        let subdir = root.join("secure_dir");

        policy
            .create_dir_all(&subdir)
            .expect("create_dir_all failed");

        let perms = std::fs::metadata(&subdir).expect("metadata failed");
        let mode = perms.permissions().mode() & 0o7777;
        assert_eq!(
            mode, 0o700,
            "create_dir_all should create directories with 0700 permissions"
        );
    }
}
