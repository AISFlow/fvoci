//! Host facts and filesystem primitives with the semantics the former Python
//! helpers used (`os.getuid`, `platform.machine`, `os.path.abspath`,
//! `shutil.which`, `tempfile.mkdtemp`/`mkstemp`).

use sha2::{Digest, Sha256};
use std::collections::hash_map::RandomState;
use std::ffi::{CStr, OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Real user id, like Python `os.getuid()`.
pub fn getuid() -> u32 {
    // SAFETY: getuid has no preconditions and cannot fail.
    unsafe { libc::getuid() }
}

/// `(sysname, machine)` from uname(2), like `platform.system()`/`platform.machine()`.
pub fn uname() -> io::Result<(String, String)> {
    // SAFETY: utsname is plain old data; uname fills it or returns -1.
    let mut name: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut name) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let field = |raw: &[libc::c_char]| {
        // SAFETY: uname NUL-terminates every field within its array.
        unsafe { CStr::from_ptr(raw.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    };
    Ok((field(&name.sysname), field(&name.machine)))
}

/// Lexical absolute path, like Python `os.path.abspath` (no symlink resolution).
pub fn abspath(path: &Path) -> io::Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut out = PathBuf::from("/");
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            Component::RootDir | Component::CurDir | Component::Prefix(_) => {}
        }
    }
    Ok(out)
}

/// First symlink among `path` and its lexical parents, if any.
pub fn first_symlink(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .filter(|part| !part.as_os_str().is_empty())
        .find(|part| part.is_symlink())
        .map(Path::to_path_buf)
}

/// `shutil.which(name, path=PATH)`: an executable non-directory file.
pub fn which(name: &str, path_var: Option<&OsStr>) -> Option<PathBuf> {
    let executable = |candidate: &Path| {
        let Ok(c_path) = std::ffi::CString::new(candidate.as_os_str().as_bytes()) else {
            return false;
        };
        // SAFETY: c_path is a valid NUL-terminated string.
        let access = unsafe { libc::access(c_path.as_ptr(), libc::X_OK) } == 0;
        access && !candidate.is_dir()
    };
    if name.contains('/') {
        let candidate = PathBuf::from(name);
        return executable(&candidate).then_some(candidate);
    }
    let default_path = OsString::from("/bin:/usr/bin");
    let search = path_var.unwrap_or(&default_path);
    std::env::split_paths(search)
        .map(|dir| dir.join(name))
        .find(|candidate| executable(candidate))
}

fn random_suffix() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default(),
    );
    hasher.write_u32(std::process::id());
    let mut bits = hasher.finish();
    (0..8)
        .map(|_| {
            let c = ALPHABET[(bits % ALPHABET.len() as u64) as usize] as char;
            bits /= ALPHABET.len() as u64;
            c
        })
        .collect()
}

/// `tempfile.mkdtemp(prefix=..., dir=...)`: a new 0700 directory.
pub fn mkdtemp(prefix: &str, dir: &Path) -> io::Result<PathBuf> {
    for _ in 0..100 {
        let candidate = dir.join(format!("{prefix}{}", random_suffix()));
        match DirBuilder::new().mode(0o700).create(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no usable temporary directory name",
    ))
}

/// `tempfile.mkstemp(prefix=..., dir=...)`: a new empty 0600 file.
pub fn mkstemp(prefix: &str, dir: &Path) -> io::Result<PathBuf> {
    for _ in 0..100 {
        let candidate = dir.join(format!("{prefix}{}", random_suffix()));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&candidate)
        {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no usable temporary file name",
    ))
}

/// `tempfile.gettempdir()` for the common case: TMPDIR, TEMP, TMP, then /tmp.
pub fn temp_root(env: impl Fn(&str) -> Option<OsString>) -> PathBuf {
    for key in ["TMPDIR", "TEMP", "TMP"] {
        if let Some(value) = env(key) {
            let path = PathBuf::from(value);
            if path.is_dir() {
                return path;
            }
        }
    }
    PathBuf::from("/tmp")
}

/// Append text to a file, creating it like Python `open(path, 'a')`.
pub fn append(path: &Path, text: &str) -> io::Result<()> {
    use std::io::Write;
    OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?
        .write_all(text.as_bytes())
}

/// Owner of `path` (following symlinks), like `path.stat().st_uid`.
pub fn owner(path: &Path) -> io::Result<u32> {
    use std::os::unix::fs::MetadataExt;
    Ok(fs::metadata(path)?.uid())
}

/// `time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())`.
pub fn utc_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (H. Hinnant), proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abspath_is_lexical() {
        assert_eq!(
            abspath(Path::new("/a/b/../c/./d")).unwrap(),
            Path::new("/a/c/d")
        );
        assert_eq!(abspath(Path::new("/../x")).unwrap(), Path::new("/x"));
        assert_eq!(
            abspath(Path::new("rel")).unwrap(),
            std::env::current_dir().unwrap().join("rel")
        );
    }

    #[test]
    fn timestamp_has_python_gmtime_shape() {
        let stamp = utc_timestamp();
        assert_eq!(stamp.len(), 20);
        assert!(stamp.ends_with('Z'));
        assert_eq!(&stamp[10..11], "T");
    }

    #[test]
    fn temporaries_are_private_and_unique() {
        use std::os::unix::fs::PermissionsExt;
        let base = mkdtemp("xtask-host-test-", &std::env::temp_dir()).unwrap();
        let dir = mkdtemp("d-", &base).unwrap();
        let file = mkstemp("f-", &base).unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_ne!(mkstemp("f-", &base).unwrap(), file);
        fs::remove_dir_all(base).unwrap();
    }
}
