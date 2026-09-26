//! `fvoci-migrate --init-env` (source `fvoci init`): writes the Compose install
//! env file from `infra/rust/.env.example` with fresh random secrets, so the
//! first install does not start from hand-edited placeholders.
//!
//! Non-interactive only: `--public-origin` and `--out` are required. The file
//! is created mode 0600 in the destination directory and renamed into place;
//! an existing file is replaced only with `--yes`. Secrets are never printed —
//! stdout gets the path only.

use std::io::Write;
use std::path::{Path, PathBuf};

use rand::RngCore;

const TEMPLATE: &str = include_str!("../infra/rust/.env.example");
pub const USAGE: &str =
    "usage: fvoci-migrate --init-env --public-origin <url> --out <path> [--yes]";

#[derive(Debug, PartialEq)]
pub struct InitEnvFlags {
    pub public_origin: String,
    pub out: PathBuf,
    pub yes: bool,
}

pub fn parse_init_env_flags(args: &[String]) -> Result<InitEnvFlags, String> {
    let mut origin = None;
    let mut out = None;
    let mut yes = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--yes" => yes = true,
            "--public-origin" | "--out" => {
                let value = it
                    .next()
                    .filter(|v| !v.starts_with("--"))
                    .ok_or(USAGE)?
                    .clone();
                if arg == "--out" {
                    out = Some(PathBuf::from(value));
                } else {
                    origin = Some(value);
                }
            }
            _ => return Err(USAGE.into()),
        }
    }
    Ok(InitEnvFlags {
        public_origin: origin.ok_or(USAGE)?,
        out: out.ok_or(USAGE)?,
        yes,
    })
}

fn hex_secret() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// The template with every secret and the origin filled in.
pub fn render_env(public_origin: &str) -> Result<String, String> {
    let origin = crate::http::guard::normalize_public_origin(public_origin)?;
    let url = url::Url::parse(&origin).map_err(|e| e.to_string())?;
    let https = url.scheme() == "https";
    let loopback_http = !https
        && match url.host() {
            Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
    let mut values: Vec<(&str, String)> = vec![
        ("POSTGRES_PASSWORD", hex_secret()),
        ("FVOCI_APP_PASSWORD", hex_secret()),
        (
            "PASSWORD_PEPPER_KEYS",
            format!(r#"{{"install":"{}"}}"#, hex_secret()),
        ),
        ("PASSWORD_PEPPER_ACTIVE_KEY_ID", "install".into()),
        ("FVOCI_PUBLIC_ORIGIN", origin.clone()),
        ("FVOCI_COOKIE_SECURE", https.to_string()),
        ("MEILI_MASTER_KEY", hex_secret()),
        (
            "ENCRYPTION_KEYS",
            format!(r#"{{"install":"{}"}}"#, hex_secret()),
        ),
        ("ENCRYPTION_ACTIVE_KEY_ID", "install".into()),
    ];
    // A plain-http loopback origin is published directly: the port must match.
    if loopback_http {
        if let Some(port) = url.port_or_known_default() {
            values.push(("FVOCI_PUBLISH_PORT", port.to_string()));
        }
    }
    let mut out = String::with_capacity(TEMPLATE.len() + 512);
    for line in TEMPLATE.lines() {
        let key = line.split_once('=').map(|(k, _)| k);
        match key.and_then(|k| values.iter().find(|(name, _)| *name == k)) {
            Some((name, value)) => out.push_str(&format!("{name}={value}")),
            None => out.push_str(line),
        }
        out.push('\n');
    }
    for (name, _) in &values {
        if !TEMPLATE
            .lines()
            .any(|l| l.split_once('=').is_some_and(|(k, _)| k == *name))
        {
            return Err(format!("template is missing {name}"));
        }
    }
    Ok(out)
}

/// Writes the rendered file; returns the destination path.
pub fn write_env(flags: &InitEnvFlags) -> Result<PathBuf, String> {
    let dest = &flags.out;
    if dest.exists() && !flags.yes {
        return Err(format!(
            "{} exists; pass --yes to overwrite",
            dest.display()
        ));
    }
    let content = render_env(&flags.public_origin)?;
    let dir = dest
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = dest
        .file_name()
        .ok_or_else(|| "--out must name a file".to_string())?;
    let staging = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        uuid::Uuid::now_v7().simple()
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let written = (|| {
        let mut file = opts.open(&staging)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        if flags.yes {
            std::fs::rename(&staging, dest)
        } else {
            // Without --yes, never replace a file that appeared after the
            // check: link() refuses an existing destination atomically.
            std::fs::hard_link(&staging, dest)?;
            std::fs::remove_file(&staging)
        }
    })();
    if let Err(err) = written {
        let _ = std::fs::remove_file(&staging);
        return Err(format!("write {}: {err}", dest.display()));
    }
    Ok(dest.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value<'a>(env: &'a str, key: &str) -> &'a str {
        env.lines()
            .find_map(|l| l.strip_prefix(&format!("{key}=")))
            .unwrap()
    }

    #[test]
    fn renders_fresh_secrets_and_origin() {
        let a = render_env("https://fvoci.example.com/").unwrap();
        let b = render_env("https://fvoci.example.com").unwrap();
        assert_eq!(
            value(&a, "FVOCI_PUBLIC_ORIGIN"),
            "https://fvoci.example.com"
        );
        assert_eq!(value(&a, "FVOCI_COOKIE_SECURE"), "true");
        assert_eq!(value(&a, "POSTGRES_PASSWORD").len(), 64);
        assert_ne!(
            value(&a, "POSTGRES_PASSWORD"),
            value(&b, "POSTGRES_PASSWORD")
        );
        assert_ne!(
            value(&a, "POSTGRES_PASSWORD"),
            value(&a, "FVOCI_APP_PASSWORD")
        );
        let pepper = value(&a, "PASSWORD_PEPPER_KEYS");
        crate::auth::password::Keyring::parse(pepper, value(&a, "PASSWORD_PEPPER_ACTIVE_KEY_ID"))
            .unwrap();
        assert!(value(&a, "MEILI_MASTER_KEY").len() >= 16);
        // Comments and optional settings stay as in the template.
        assert!(a.contains("# Never commit real secrets."));
        assert_eq!(value(&a, "SMTP_HOST"), "");
        assert_eq!(value(&a, "FVOCI_PUBLISH_PORT"), "8080");
    }

    #[test]
    fn loopback_http_publishes_the_origin_port() {
        let env = render_env("http://127.0.0.1:9090").unwrap();
        assert_eq!(value(&env, "FVOCI_COOKIE_SECURE"), "false");
        assert_eq!(value(&env, "FVOCI_PUBLISH_PORT"), "9090");
        assert!(render_env("ftp://x").is_err());
    }

    #[test]
    fn flags_require_origin_and_out() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(parse_init_env_flags(&args(&["--out", "x"])).is_err());
        assert!(parse_init_env_flags(&args(&["--public-origin", "--out", "x"])).is_err());
        assert_eq!(
            parse_init_env_flags(&args(&[
                "--public-origin",
                "https://a",
                "--out",
                "e",
                "--yes"
            ]))
            .unwrap(),
            InitEnvFlags {
                public_origin: "https://a".into(),
                out: "e".into(),
                yes: true
            }
        );
    }

    #[test]
    fn writes_0600_and_refuses_overwrite_without_yes() {
        let dir = std::env::temp_dir().join(format!("fvoci-init-env-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join(".env");
        let flags = InitEnvFlags {
            public_origin: "https://fvoci.example.com".into(),
            out: out.clone(),
            yes: false,
        };
        write_env(&flags).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&out).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let first = std::fs::read_to_string(&out).unwrap();
        assert!(write_env(&flags).unwrap_err().contains("--yes"));
        assert_eq!(std::fs::read_to_string(&out).unwrap(), first);
        write_env(&InitEnvFlags { yes: true, ..flags }).unwrap();
        assert_ne!(std::fs::read_to_string(&out).unwrap(), first);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
