//! Path and file guards: physical (symlink-free, normal-form) absolute paths,
//! directories and files owned by the expected user, and the identity tuple
//! that every race check compares.

use super::{require, Refusal};
use std::fs::{self, Metadata};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

/// `(st_dev, st_ino, st_mode, st_uid, st_gid, st_nlink)`; `st_mode` keeps the
/// file type bits.
pub type Identity = [u64; 6];

pub fn identity(info: &Metadata) -> Identity {
    [
        info.dev(),
        info.ino(),
        u64::from(info.mode()),
        u64::from(info.uid()),
        u64::from(info.gid()),
        info.nlink(),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Owner {
    pub uid: u32,
    pub gid: u32,
}

impl Owner {
    /// Real user and group of this process.
    pub fn current() -> Self {
        // SAFETY: getuid/getgid have no preconditions and cannot fail.
        unsafe {
            Self {
                uid: libc::getuid(),
                gid: libc::getgid(),
            }
        }
    }

    fn owns(self, info: &Metadata) -> bool {
        (info.uid(), info.gid()) == (self.uid, self.gid)
    }
}

/// An absolute path spelled in normal form (no `.`, `..`, repeated or
/// trailing separators) with no symlink at any existing ancestor.
pub fn physical(path: &Path) -> Result<PathBuf, Refusal> {
    let normal: PathBuf = path.components().collect();
    require(
        path.is_absolute()
            && normal.as_os_str() == path.as_os_str()
            && !path.components().any(|part| part == Component::ParentDir),
        "nonphysical-path",
    )?;
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(info) => require(!info.file_type().is_symlink(), "nonphysical-path")?,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(path.to_path_buf())
}

pub fn owned_directory(path: &Path, owner: Owner) -> Result<PathBuf, Refusal> {
    let path = physical(path)?;
    let info = fs::symlink_metadata(&path)?;
    require(info.is_dir() && owner.owns(&info), "unowned-directory")?;
    Ok(path)
}

/// A physical, non-hardlinked regular file, optionally with an exact
/// permission mode and owner.
pub fn regular(path: &Path, mode: Option<u32>, owner: Option<Owner>) -> Result<Metadata, Refusal> {
    physical(path)?;
    let info = fs::symlink_metadata(path)?;
    require(
        info.file_type().is_file() && info.nlink() == 1,
        "nonregular-or-hardlinked-file",
    )?;
    if let Some(mode) = mode {
        require(info.mode() & 0o7777 == mode, "unexpected-file-mode")?;
    }
    if let Some(owner) = owner {
        require(owner.owns(&info), "unowned-file")?;
    }
    Ok(info)
}

/// SHA-256 of the bytes at `path` and its `lstat` identity.
pub fn file_facts(path: &Path) -> Result<(String, Identity), Refusal> {
    let digest = crate::host::sha256_hex(&fs::read(path)?);
    Ok((digest, identity(&fs::symlink_metadata(path)?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn reason(result: Result<impl std::fmt::Debug, Refusal>) -> String {
        result.unwrap_err().to_string()
    }

    #[test]
    fn physical_requires_absolute_normal_symlink_free_paths() {
        let base = crate::host::mkdtemp("xtask-rustup-guard-", &std::env::temp_dir()).unwrap();
        let base = base.canonicalize().unwrap();
        fs::create_dir(base.join("real")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();
        assert_eq!(physical(&base.join("real")).unwrap(), base.join("real"));
        // A missing tail stays acceptable; its lstat refuses later.
        assert!(physical(&base.join("real/missing/deeper")).is_ok());
        let text = base.to_str().unwrap();
        for raw in [
            "relative".to_owned(),
            String::new(),
            format!("{text}/real/"),
            format!("{text}//real"),
            format!("{text}/./real"),
            format!("{text}/real/../real"),
            format!("/{text}/real"),
            format!("{text}/link"),
            format!("{text}/link/inner"),
        ] {
            assert_eq!(
                reason(physical(Path::new(&raw))),
                "nonphysical-path",
                "{raw}"
            );
        }
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn ownership_mode_and_link_count_are_exact() {
        let base = crate::host::mkdtemp("xtask-rustup-guard-", &std::env::temp_dir()).unwrap();
        let base = base.canonicalize().unwrap();
        let owner = Owner::current();
        let file = base.join("file");
        fs::write(&file, b"x").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(regular(&file, Some(0o644), Some(owner)).is_ok());
        assert_eq!(
            reason(regular(&file, Some(0o755), None)),
            "unexpected-file-mode"
        );
        let foreign = Owner {
            uid: owner.uid + 123,
            gid: owner.gid,
        };
        assert_eq!(reason(regular(&file, None, Some(foreign))), "unowned-file");
        assert_eq!(reason(owned_directory(&base, foreign)), "unowned-directory");
        assert_eq!(reason(owned_directory(&file, owner)), "unowned-directory");
        fs::hard_link(&file, base.join("second")).unwrap();
        assert_eq!(
            reason(regular(&file, None, None)),
            "nonregular-or-hardlinked-file"
        );
        assert_eq!(
            reason(regular(&base, None, None)),
            "nonregular-or-hardlinked-file"
        );
        assert_eq!(
            reason(regular(&base.join("absent"), None, None)),
            "FileNotFoundError"
        );
        fs::remove_dir_all(base).unwrap();
    }
}
