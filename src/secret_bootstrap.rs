//! `fvoci-migrate --bootstrap-secrets` (standalone Compose install): generates
//! the install's secrets once, before PostgreSQL or Meilisearch first start,
//! into one directory per audience so each container mounts only what it
//! reads. Values use the same formats as `--init-env`.
//!
//! Layout under `<root>` (each subdirectory is a separate volume that must
//! already exist):
//!
//! | audience       | owner uid | files |
//! | -------------- | --------- | ----- |
//! | `postgres`     | 999       | `postgres_password` |
//! | `meilisearch`  | 0         | `master_key` |
//! | `init`         | 1000      | `postgres_password`, `app_password`, `meili_master_key` |
//! | `server`       | 1000      | `database_app_url`, `password_pepper_keys`, `password_pepper_active_key_id`, `encryption_keys`, `encryption_active_key_id` |
//!
//! Files are mode 0600. Every audience gets a `.fvoci-bootstrap-complete`
//! marker holding one install id, written only after all secret files are in
//! place, so an interrupted run is never taken as complete. With all markers
//! and files present the command changes nothing. Otherwise it regenerates
//! everything, but only while every `--data-dir` is empty: once PostgreSQL,
//! storage or search data exists, missing secrets mean a lost or partial
//! restore, and new keys would lock that data out, so it fails instead.
//! Secret values are never printed.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{fchown, OpenOptionsExt};
use std::path::{Path, PathBuf};

use crate::init_env::hex_secret;

pub const USAGE: &str = "usage: fvoci-migrate --bootstrap-secrets <root> [--data-dir <path>]...";
pub const MARKER: &str = ".fvoci-bootstrap-complete";
const TEMP_PREFIX: &str = ".fvoci-bootstrap-tmp-";
/// Key id written with fresh pepper and encryption keyrings (same as `--init-env`).
const KEY_ID: &str = "install";
/// Compose service address of PostgreSQL in the standalone install.
const DB_HOST: &str = "postgres:5432";

const POSTGRES_UID: u32 = 999;
const MEILI_UID: u32 = 0;
const FVOCI_UID: u32 = 1000;

#[derive(Debug, PartialEq)]
pub struct BootstrapFlags {
    pub root: PathBuf,
    pub data_dirs: Vec<PathBuf>,
}

pub fn parse_flags(args: &[String]) -> Result<BootstrapFlags, String> {
    let mut it = args.iter();
    let root = it.next().filter(|v| !v.starts_with("--")).ok_or(USAGE)?;
    let mut data_dirs = Vec::new();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--data-dir" => {
                let dir = it.next().filter(|v| !v.starts_with("--")).ok_or(USAGE)?;
                data_dirs.push(PathBuf::from(dir));
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(BootstrapFlags {
        root: PathBuf::from(root),
        data_dirs,
    })
}

/// Which audience directory receives which file, and its owner.
pub struct Audience {
    pub dir: &'static str,
    pub uid: u32,
    pub files: &'static [&'static str],
}

pub const AUDIENCES: &[Audience] = &[
    Audience {
        dir: "postgres",
        uid: POSTGRES_UID,
        files: &["postgres_password"],
    },
    Audience {
        dir: "meilisearch",
        uid: MEILI_UID,
        files: &["master_key"],
    },
    Audience {
        dir: "init",
        uid: FVOCI_UID,
        files: &["postgres_password", "app_password", "meili_master_key"],
    },
    Audience {
        dir: "server",
        uid: FVOCI_UID,
        files: &[
            "database_app_url",
            "password_pepper_keys",
            "password_pepper_active_key_id",
            "encryption_keys",
            "encryption_active_key_id",
        ],
    },
];

#[derive(Debug, PartialEq)]
pub enum Outcome {
    AlreadyComplete,
    Generated,
}

/// Database names that end up in `database_app_url`; restricted so they need
/// no URL escaping.
pub struct DbNames {
    pub database: String,
    pub app_role: String,
}

impl DbNames {
    pub fn from_env() -> Result<Self, String> {
        let get = |name: &str, default: &str| {
            let value = std::env::var(name).unwrap_or_else(|_| default.to_string());
            let valid = !value.is_empty()
                && value.len() <= 63
                && value
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                && !value.as_bytes()[0].is_ascii_digit();
            if valid {
                Ok(value)
            } else {
                Err(format!(
                    "{name} must be a lowercase identifier ([a-z_][a-z0-9_]*)"
                ))
            }
        };
        Ok(Self {
            database: get("POSTGRES_DB", "fvoci")?,
            app_role: get("FVOCI_APP_ROLE", "fvoci_app")?,
        })
    }
}

pub fn run(flags: &BootstrapFlags) -> Result<Outcome, String> {
    bootstrap(flags, &DbNames::from_env()?, |uid| uid)
}

/// `owner` maps an audience uid to the uid actually written (tests run
/// unprivileged and map everything to their own uid).
pub fn bootstrap(
    flags: &BootstrapFlags,
    names: &DbNames,
    owner: impl Fn(u32) -> u32,
) -> Result<Outcome, String> {
    for audience in AUDIENCES {
        let dir = flags.root.join(audience.dir);
        if !dir.is_dir() {
            return Err(format!(
                "secret directory {} is missing (mount its volume)",
                dir.display()
            ));
        }
    }
    let missing = incomplete_items(&flags.root)?;
    if missing.is_empty() {
        return Ok(Outcome::AlreadyComplete);
    }
    let occupied = occupied_data_dirs(&flags.data_dirs)?;
    if !occupied.is_empty() {
        return Err(format!(
            "refusing to generate new secrets: data already exists in {} but these secrets are missing or incomplete: {}. \
             Restore the secret volumes from the backup taken with this data; new keys cannot open it.",
            occupied.join(", "),
            missing.join(", ")
        ));
    }
    generate(&flags.root, names, owner)?;
    Ok(Outcome::Generated)
}

/// Markers and secret files that are absent, empty or from another install.
fn incomplete_items(root: &Path) -> Result<Vec<String>, String> {
    let mut missing = Vec::new();
    let mut install_ids = Vec::new();
    for audience in AUDIENCES {
        let dir = root.join(audience.dir);
        match fs::read_to_string(dir.join(MARKER)) {
            Ok(id) => install_ids.push(id),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                missing.push(format!("{}/{MARKER}", audience.dir))
            }
            Err(err) => return Err(format!("read {}/{MARKER}: {err}", audience.dir)),
        }
        for file in audience.files {
            // metadata only: the files belong to other uids and stay unread.
            match fs::symlink_metadata(dir.join(file)) {
                Ok(meta) if meta.is_file() && meta.len() > 0 => {}
                Ok(_) => missing.push(format!("{}/{file}", audience.dir)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(format!("{}/{file}", audience.dir))
                }
                Err(err) => return Err(format!("stat {}/{file}: {err}", audience.dir)),
            }
        }
    }
    if missing.is_empty() && install_ids.windows(2).any(|w| w[0] != w[1]) {
        missing.push("markers from different installs".into());
    }
    Ok(missing)
}

fn occupied_data_dirs(dirs: &[PathBuf]) -> Result<Vec<String>, String> {
    let mut occupied = Vec::new();
    for dir in dirs {
        let mut entries = fs::read_dir(dir)
            .map_err(|e| format!("data directory {}: {e} (mount its volume)", dir.display()))?;
        if entries.next().is_some() {
            occupied.push(dir.display().to_string());
        }
    }
    Ok(occupied)
}

fn generate(root: &Path, names: &DbNames, owner: impl Fn(u32) -> u32) -> Result<(), String> {
    let owner_password = hex_secret();
    let app_password = hex_secret();
    let meili_master_key = hex_secret();
    let keyring = || format!(r#"{{"{KEY_ID}":"{}"}}"#, hex_secret());
    let pepper_keys = keyring();
    let encryption_keys = keyring();
    let app_url = format!(
        "postgres://{}:{app_password}@{DB_HOST}/{}",
        names.app_role, names.database
    );
    let value = |file: &str| -> &str {
        match file {
            "postgres_password" => &owner_password,
            "app_password" => &app_password,
            "master_key" | "meili_master_key" => &meili_master_key,
            "database_app_url" => &app_url,
            "password_pepper_keys" => &pepper_keys,
            "encryption_keys" => &encryption_keys,
            "password_pepper_active_key_id" | "encryption_active_key_id" => KEY_ID,
            other => unreachable!("no value for {other}"),
        }
    };
    let install_id = uuid::Uuid::now_v7().to_string();
    for audience in AUDIENCES {
        let dir = root.join(audience.dir);
        // A previous run's marker may survive in some directories; drop it
        // first so a crash below cannot leave old markers next to new files.
        remove_if_present(&dir.join(MARKER))?;
        remove_stale_temps(&dir)?;
        for file in audience.files {
            write_atomic(
                &dir,
                file,
                value(file).as_bytes(),
                0o600,
                owner(audience.uid),
            )?;
        }
    }
    // Markers last: until all four exist the install is not bootstrapped.
    for audience in AUDIENCES {
        let dir = root.join(audience.dir);
        write_atomic(&dir, MARKER, install_id.as_bytes(), 0o644, owner(0))?;
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("remove {}: {err}", path.display())),
    }
}

fn remove_stale_temps(dir: &Path) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("read {}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read {}: {e}", dir.display()))?;
        if entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
            remove_if_present(&entry.path())?;
        }
    }
    Ok(())
}

/// Temp file (create_new, final mode and owner, fsync) renamed over `name`,
/// then the directory is fsynced.
fn write_atomic(dir: &Path, name: &str, body: &[u8], mode: u32, uid: u32) -> Result<(), String> {
    let temp = dir.join(format!("{TEMP_PREFIX}{}", uuid::Uuid::now_v7().simple()));
    let dest = dir.join(name);
    let written = (|| -> std::io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(&temp)?;
        file.write_all(body)?;
        fchown(&file, Some(uid), Some(uid))?;
        file.sync_all()?;
        fs::rename(&temp, &dest)?;
        fs::File::open(dir)?.sync_all()
    })();
    if let Err(err) = written {
        let _ = fs::remove_file(&temp);
        return Err(format!("write {}: {err}", dest.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    struct Fixture {
        base: PathBuf,
        flags: BootstrapFlags,
    }

    impl Fixture {
        fn new() -> Self {
            let base =
                std::env::temp_dir().join(format!("fvoci-bootstrap-{}", uuid::Uuid::now_v7()));
            let root = base.join("secrets");
            for audience in AUDIENCES {
                fs::create_dir_all(root.join(audience.dir)).unwrap();
            }
            let data_dirs = vec![base.join("pgdata"), base.join("storage")];
            for dir in &data_dirs {
                fs::create_dir_all(dir).unwrap();
            }
            Self {
                base,
                flags: BootstrapFlags { root, data_dirs },
            }
        }

        fn run(&self) -> Result<Outcome, String> {
            let names = DbNames {
                database: "fvoci".into(),
                app_role: "fvoci_app".into(),
            };
            let me = fs::metadata(&self.base).unwrap().uid();
            bootstrap(&self.flags, &names, |_| me)
        }

        fn read(&self, audience: &str, file: &str) -> String {
            fs::read_to_string(self.flags.root.join(audience).join(file)).unwrap()
        }

        fn snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
            let mut all = Vec::new();
            for audience in AUDIENCES {
                let dir = self.flags.root.join(audience.dir);
                let mut entries: Vec<_> = fs::read_dir(&dir)
                    .unwrap()
                    .map(|e| e.unwrap().path())
                    .collect();
                entries.sort();
                for path in entries {
                    let body = fs::read(&path).unwrap();
                    all.push((path, body));
                }
            }
            all
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn generates_consistent_secrets_once() {
        let fx = Fixture::new();
        assert_eq!(fx.run().unwrap(), Outcome::Generated);

        let owner = fx.read("postgres", "postgres_password");
        assert_eq!(owner.len(), 64);
        assert_eq!(fx.read("init", "postgres_password"), owner);
        let master = fx.read("meilisearch", "master_key");
        assert_eq!(fx.read("init", "meili_master_key"), master);
        let app = fx.read("init", "app_password");
        assert_ne!(app, owner);
        assert_ne!(app, master);
        assert_eq!(
            fx.read("server", "database_app_url"),
            format!("postgres://fvoci_app:{app}@postgres:5432/fvoci")
        );
        let pepper = fx.read("server", "password_pepper_keys");
        let encryption = fx.read("server", "encryption_keys");
        assert_ne!(pepper, encryption);
        crate::auth::password::Keyring::parse(
            &pepper,
            &fx.read("server", "password_pepper_active_key_id"),
        )
        .unwrap();
        crate::auth::password::Keyring::parse_named(
            &encryption,
            &fx.read("server", "encryption_active_key_id"),
            "ENCRYPTION_KEYS",
        )
        .unwrap();
        // The server audience holds neither the owner password nor the master key.
        for file in AUDIENCES.iter().find(|a| a.dir == "server").unwrap().files {
            let body = fx.read("server", file);
            assert!(!body.contains(&owner) && !body.contains(&master), "{file}");
        }
        for audience in AUDIENCES {
            for file in audience.files {
                let mode = fs::metadata(fx.flags.root.join(audience.dir).join(file))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o777, 0o600, "{}/{file}", audience.dir);
            }
        }

        let before = fx.snapshot();
        fs::write(fx.flags.data_dirs[0].join("PG_VERSION"), "18").unwrap();
        assert_eq!(fx.run().unwrap(), Outcome::AlreadyComplete);
        assert_eq!(fx.snapshot(), before);
    }

    #[test]
    fn missing_secrets_with_existing_data_fail_closed() {
        let fx = Fixture::new();
        fx.run().unwrap();
        fs::remove_file(fx.flags.root.join("server/password_pepper_keys")).unwrap();
        fs::write(fx.flags.data_dirs[1].join("object"), "x").unwrap();
        let before = fx.snapshot();
        let err = fx.run().unwrap_err();
        assert!(err.contains("refusing to generate"), "{err}");
        assert!(err.contains("server/password_pepper_keys"), "{err}");
        assert_eq!(fx.snapshot(), before);
        for (_, body) in &before {
            // No secret value leaks into the message.
            if body.len() >= 32 {
                assert!(!err.contains(std::str::from_utf8(body).unwrap()));
            }
        }

        // Losing a whole audience volume (marker included) is refused too.
        fs::write(fx.flags.data_dirs[0].join("PG_VERSION"), "18").unwrap();
        for entry in fs::read_dir(fx.flags.root.join("init")).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        assert!(fx
            .run()
            .unwrap_err()
            .contains("init/.fvoci-bootstrap-complete"));
    }

    #[test]
    fn interrupted_run_is_regenerated_while_data_is_empty() {
        let fx = Fixture::new();
        fx.run().unwrap();
        let old = fx.read("postgres", "postgres_password");
        // Simulate a crash before the markers: one marker gone, a stale temp left.
        fs::remove_file(fx.flags.root.join("server").join(MARKER)).unwrap();
        fs::write(
            fx.flags.root.join("init").join(format!("{TEMP_PREFIX}x")),
            "p",
        )
        .unwrap();
        assert_eq!(fx.run().unwrap(), Outcome::Generated);
        assert_ne!(fx.read("postgres", "postgres_password"), old);
        assert!(!fx
            .flags
            .root
            .join("init")
            .join(format!("{TEMP_PREFIX}x"))
            .exists());
        let ids: Vec<_> = AUDIENCES.iter().map(|a| fx.read(a.dir, MARKER)).collect();
        assert!(ids.windows(2).all(|w| w[0] == w[1]));
        assert_eq!(fx.run().unwrap(), Outcome::AlreadyComplete);
    }

    #[test]
    fn markers_from_different_installs_are_incomplete() {
        let fx = Fixture::new();
        fx.run().unwrap();
        fs::write(fx.flags.root.join("server").join(MARKER), "other").unwrap();
        fs::write(fx.flags.data_dirs[0].join("PG_VERSION"), "18").unwrap();
        assert!(fx.run().unwrap_err().contains("different installs"));
    }

    #[test]
    fn missing_mounts_fail_before_writing() {
        let fx = Fixture::new();
        fs::remove_dir(fx.flags.root.join("meilisearch")).unwrap();
        assert!(fx.run().unwrap_err().contains("meilisearch is missing"));
        fs::create_dir(fx.flags.root.join("meilisearch")).unwrap();
        fs::remove_dir(&fx.flags.data_dirs[1]).unwrap();
        assert!(fx.run().unwrap_err().contains("mount its volume"));
        assert_eq!(
            fs::read_dir(fx.flags.root.join("postgres"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn flags_and_names() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            parse_flags(&args(&["/s", "--data-dir", "/a", "--data-dir", "/b"])).unwrap(),
            BootstrapFlags {
                root: "/s".into(),
                data_dirs: vec!["/a".into(), "/b".into()]
            }
        );
        assert!(parse_flags(&args(&[])).is_err());
        assert!(parse_flags(&args(&["/s", "--data-dir"])).is_err());
        assert!(parse_flags(&args(&["/s", "--other"])).is_err());
    }
}
