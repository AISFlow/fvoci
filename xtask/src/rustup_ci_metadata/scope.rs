//! Admission scope: an allocated GitHub-hosted collaboration job on Ubuntu
//! 26.04 x64, the ephemeral runner home and its own Rustup, a clean checkout
//! at `GITHUB_SHA`, and the fixed receipt directory under `RUNNER_TEMP`.
//! Checks run in this order and stop at the first refusal.

use super::guard::Identity;
use super::{require, Refusal, RECEIPT_DIRECTORY, TOOLCHAIN};
use serde_json::{json, Map, Value};
use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

pub const JOBS: [&str; 9] = [
    "collaboration-build",
    "collaboration-flow",
    "collaboration-install-on",
    "collaboration-postgres-on",
    "collaboration-sqlite-on",
    "collaboration-postgres-off",
    "collaboration-sqlite-off",
    "workspace-browser-build",
    "workspace-browser-shard",
];
const RUNNER_HOME: &str = "/home/runner";

/// Host facts the scope reads; the process implementation is `super::System`.
pub trait Host {
    fn env(&self, name: &str) -> Option<String>;
    fn uname(&self) -> io::Result<(String, String)>;
    fn os_release(&self) -> Result<String, Refusal>;
    /// Canonical current directory.
    fn cwd(&self) -> io::Result<PathBuf>;
    fn which(&self, name: &str) -> Option<PathBuf>;
    /// Stripped stdout of a read-only inspection command.
    fn command(&mut self, argv: &[&OsStr], cwd: Option<&Path>) -> Result<String, Refusal>;
    fn owned_directory(&self, path: &Path) -> Result<PathBuf, Refusal>;
    fn physical(&self, path: &Path) -> Result<PathBuf, Refusal>;
    /// A regular, non-hardlinked 0755 file.
    fn executable(&self, path: &Path) -> Result<(), Refusal>;
    fn file_facts(&self, path: &Path) -> Result<(String, Identity), Refusal>;
}

#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub source_head: String,
    pub source_tree: String,
    pub job: String,
    pub rustup_version: String,
    pub rustup_sha256: String,
    pub rustup_identity: Identity,
}

impl Context {
    pub fn fields(&self) -> Map<String, Value> {
        let value = json!({
            "source_head": self.source_head,
            "source_tree": self.source_tree,
            "job": self.job,
            "rustup_version": self.rustup_version,
            "rustup_sha256": self.rustup_sha256,
            "rustup_identity": self.rustup_identity,
        });
        let Value::Object(map) = value else {
            unreachable!()
        };
        map
    }
}

#[derive(Debug)]
pub struct Scope {
    pub root: PathBuf,
    pub rustup: PathBuf,
    pub context: Context,
}

fn hex40(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `rustup 1.29.1 (<lowercase hex> YYYY-MM-DD)`.
fn supported_rustup_version(text: &str) -> bool {
    let Some(inner) = text
        .strip_prefix("rustup 1.29.1 (")
        .and_then(|rest| rest.strip_suffix(')'))
    else {
        return false;
    };
    let Some((commit, date)) = inner.split_once(' ') else {
        return false;
    };
    let digits =
        |part: &str, len: usize| part.len() == len && part.bytes().all(|b| b.is_ascii_digit());
    let mut date = date.split('-');
    !commit.is_empty()
        && commit
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && date.next().is_some_and(|part| digits(part, 4))
        && date.next().is_some_and(|part| digits(part, 2))
        && date.next().is_some_and(|part| digits(part, 2))
        && date.next().is_none()
}

/// `ID` and `VERSION_ID` of an os-release file; later lines win and
/// surrounding double quotes are dropped.
fn os_release(text: &str) -> (String, String) {
    let (mut id, mut version) = (String::new(), String::new());
    for (key, value) in text.lines().filter_map(|line| line.split_once('=')) {
        match key {
            "ID" => id = value.trim_matches('"').to_owned(),
            "VERSION_ID" => version = value.trim_matches('"').to_owned(),
            _ => {}
        }
    }
    (id, version)
}

pub fn scope(output: &str, host: &mut dyn Host) -> Result<Scope, Refusal> {
    for name in ["CI", "GITHUB_ACTIONS"] {
        require(host.env(name).as_deref() == Some("true"), "not-github-ci")?;
    }
    let job = host.env("GITHUB_JOB").unwrap_or_default();
    require(JOBS.contains(&job.as_str()), "unallocated-job")?;
    require(
        host.env("RUNNER_ENVIRONMENT").as_deref() == Some("github-hosted")
            && host.env("RUNNER_OS").as_deref() == Some("Linux")
            && host.env("RUNNER_ARCH").as_deref() == Some("X64"),
        "unsupported-runner",
    )?;
    let (system, machine) = host.uname()?;
    require(
        system == "Linux" && machine == "x86_64",
        "unsupported-platform",
    )?;
    let (id, version_id) = os_release(&host.os_release()?);
    require(id == "ubuntu" && version_id == "26.04", "unsupported-os")?;
    let home = host.owned_directory(Path::new(&host.env("HOME").unwrap_or_default()))?;
    require(home.as_os_str() == RUNNER_HOME, "not-ephemeral-runner-home")?;
    let rustup_home = host.owned_directory(&home.join(".rustup"))?;
    let default_home = rustup_home.to_string_lossy();
    require(
        host.env("RUSTUP_HOME")
            .unwrap_or_else(|| default_home.to_string())
            == default_home,
        "rustup-home-override",
    )?;
    host.owned_directory(&rustup_home.join("toolchains"))?;
    let root = host.owned_directory(&rustup_home.join("toolchains").join(TOOLCHAIN))?;
    let temporary =
        host.owned_directory(Path::new(&host.env("RUNNER_TEMP").unwrap_or_default()))?;
    // The exact spelling the workflows pass: "$RUNNER_TEMP/<receipt directory>".
    require(
        output.as_bytes()
            == temporary
                .join(RECEIPT_DIRECTORY)
                .as_os_str()
                .as_encoded_bytes(),
        "receipt-prefix-override",
    )?;
    let workspace = host.physical(Path::new(&host.env("GITHUB_WORKSPACE").unwrap_or_default()))?;
    require(workspace == host.cwd()?, "checkout-path-mismatch")?;
    let git = OsStr::new("git");
    let rev_parse = OsStr::new("rev-parse");
    let head = host.command(&[git, rev_parse, OsStr::new("HEAD")], Some(&workspace))?;
    require(
        hex40(&head) && Some(&head) == host.env("GITHUB_SHA").as_ref(),
        "source-head-mismatch",
    )?;
    let tree = host.command(
        &[git, rev_parse, OsStr::new("HEAD^{tree}")],
        Some(&workspace),
    )?;
    require(hex40(&tree), "source-tree-format")?;
    let status = [
        git,
        OsStr::new("status"),
        OsStr::new("--porcelain=v1"),
        OsStr::new("--untracked-files=all"),
    ];
    require(
        host.command(&status, Some(&workspace))?.is_empty(),
        "source-drift",
    )?;
    let rustup = host.physical(&home.join(".cargo/bin/rustup"))?;
    host.executable(&rustup)?;
    require(
        host.which("rustup").as_deref() == Some(rustup.as_path()),
        "rustup-executable-mismatch",
    )?;
    let version = host.command(&[rustup.as_os_str(), OsStr::new("--version")], None)?;
    require(
        supported_rustup_version(&version),
        "unsupported-rustup-version",
    )?;
    let (rustup_sha256, rustup_identity) = host.file_facts(&rustup)?;
    Ok(Scope {
        root,
        rustup,
        context: Context {
            source_head: head,
            source_tree: tree,
            job,
            rustup_version: version,
            rustup_sha256,
            rustup_identity,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsString;

    const OUTPUT: &str = "/home/runner/work/_temp/fvoci-rustup-ci-metadata";
    const WORKSPACE: &str = "/home/runner/work/fvoci/fvoci";

    struct Fake {
        env: BTreeMap<&'static str, String>,
        uname: (String, String),
        release: String,
        version: String,
        head: String,
        tree: String,
        status: String,
        which: Option<PathBuf>,
        calls: Vec<Vec<OsString>>,
    }

    impl Default for Fake {
        fn default() -> Self {
            let env = [
                ("CI", "true"),
                ("GITHUB_ACTIONS", "true"),
                ("GITHUB_JOB", "collaboration-build"),
                ("RUNNER_ENVIRONMENT", "github-hosted"),
                ("RUNNER_OS", "Linux"),
                ("RUNNER_ARCH", "X64"),
                ("HOME", "/home/runner"),
                ("RUNNER_TEMP", "/home/runner/work/_temp"),
                ("GITHUB_WORKSPACE", WORKSPACE),
                ("GITHUB_SHA", "5555555555555555555555555555555555555555"),
            ];
            Self {
                env: env.into_iter().map(|(k, v)| (k, v.to_owned())).collect(),
                uname: ("Linux".into(), "x86_64".into()),
                release: "ID=ubuntu\nVERSION_ID=\"26.04\"\n".into(),
                version: "rustup 1.29.1 (d95a37b6a 2026-08-13)".into(),
                head: "5".repeat(40),
                tree: "6".repeat(40),
                status: String::new(),
                which: Some("/home/runner/.cargo/bin/rustup".into()),
                calls: Vec::new(),
            }
        }
    }

    impl Host for Fake {
        fn env(&self, name: &str) -> Option<String> {
            self.env.get(name).cloned()
        }
        fn uname(&self) -> io::Result<(String, String)> {
            Ok(self.uname.clone())
        }
        fn os_release(&self) -> Result<String, Refusal> {
            Ok(self.release.clone())
        }
        fn cwd(&self) -> io::Result<PathBuf> {
            Ok(WORKSPACE.into())
        }
        fn which(&self, _name: &str) -> Option<PathBuf> {
            self.which.clone()
        }
        fn command(&mut self, argv: &[&OsStr], _cwd: Option<&Path>) -> Result<String, Refusal> {
            self.calls
                .push(argv.iter().map(|a| a.to_os_string()).collect());
            let argv: Vec<&str> = argv.iter().map(|a| a.to_str().unwrap()).collect();
            Ok(match argv[..] {
                ["git", "rev-parse", "HEAD"] => self.head.clone(),
                ["git", "rev-parse", _] => self.tree.clone(),
                ["git", "status", ..] => self.status.clone(),
                [_, "--version"] => self.version.clone(),
                _ => panic!("unexpected command {argv:?}"),
            })
        }
        fn owned_directory(&self, path: &Path) -> Result<PathBuf, Refusal> {
            super::super::guard::physical(path).map(drop)?;
            Ok(path.to_path_buf())
        }
        fn physical(&self, path: &Path) -> Result<PathBuf, Refusal> {
            super::super::guard::physical(path)
        }
        fn executable(&self, _path: &Path) -> Result<(), Refusal> {
            Ok(())
        }
        fn file_facts(&self, _path: &Path) -> Result<(String, Identity), Refusal> {
            Ok(("a".repeat(64), [1, 2, 0o100755, 1001, 1001, 1]))
        }
    }

    fn reason(fake: &mut Fake, output: &str) -> String {
        scope(output, fake).unwrap_err().to_string()
    }

    #[test]
    fn every_allocated_job_is_admitted() {
        for job in JOBS {
            let mut fake = Fake::default();
            fake.env.insert("GITHUB_JOB", job.to_owned());
            let found = scope(OUTPUT, &mut fake).unwrap();
            assert_eq!(
                found.root,
                Path::new("/home/runner/.rustup/toolchains/1.98.1-x86_64-unknown-linux-gnu")
            );
            assert_eq!(found.rustup, Path::new("/home/runner/.cargo/bin/rustup"));
            assert_eq!(found.context.source_head, "5".repeat(40));
            assert_eq!(found.context.source_tree, "6".repeat(40));
            assert_eq!(found.context.job, job);
            assert_eq!(fake.calls.len(), 4);
        }
        let mut fake = Fake::default();
        fake.env
            .insert("RUSTUP_HOME", "/home/runner/.rustup".into());
        assert!(scope(OUTPUT, &mut fake).is_ok());
    }

    #[test]
    fn unallocated_environment_refusals() {
        let cases = [
            ("CI", "false", "not-github-ci"),
            ("GITHUB_ACTIONS", "false", "not-github-ci"),
            ("GITHUB_JOB", "web-checks", "unallocated-job"),
            ("GITHUB_JOB", "", "unallocated-job"),
            ("RUNNER_ENVIRONMENT", "self-hosted", "unsupported-runner"),
            ("RUNNER_OS", "Windows", "unsupported-runner"),
            ("RUNNER_ARCH", "ARM64", "unsupported-runner"),
            ("HOME", "/home/other", "not-ephemeral-runner-home"),
            ("HOME", "/home/runner/", "nonphysical-path"),
            ("HOME", "", "nonphysical-path"),
            ("RUSTUP_HOME", "/shared/rustup", "rustup-home-override"),
            (
                "RUSTUP_HOME",
                "/home/runner/.rustup/",
                "rustup-home-override",
            ),
            ("RUNNER_TEMP", "relative", "nonphysical-path"),
            ("GITHUB_SHA", &"7".repeat(40), "source-head-mismatch"),
            (
                "GITHUB_WORKSPACE",
                "/other/checkout",
                "checkout-path-mismatch",
            ),
            ("GITHUB_WORKSPACE", "", "nonphysical-path"),
        ];
        for (name, value, expected) in cases {
            let mut fake = Fake::default();
            fake.env.insert(name, value.to_owned());
            assert_eq!(reason(&mut fake, OUTPUT), expected, "{name}={value}");
        }
        for name in ["CI", "GITHUB_JOB", "RUNNER_OS", "GITHUB_SHA"] {
            let mut fake = Fake::default();
            fake.env.remove(name);
            assert!(scope(OUTPUT, &mut fake).is_err(), "{name} unset");
        }
    }

    #[test]
    fn source_version_platform_and_output_refusals() {
        type Edit = fn(&mut Fake);
        let cases: [(Edit, &str); 13] = [
            (
                |f| f.version = "rustup 1.29.0 (abc 2026-01-01)".into(),
                "unsupported-rustup-version",
            ),
            (
                |f| f.version = "rustup 1.29.1 (ABC 2026-01-01)".into(),
                "unsupported-rustup-version",
            ),
            (
                |f| f.version = "rustup 1.29.1 (abc 2026-1-01)".into(),
                "unsupported-rustup-version",
            ),
            (|f| f.head = "notasha".into(), "source-head-mismatch"),
            (|f| f.tree = "invalid".into(), "source-tree-format"),
            (|f| f.status = "?? drift.rs".into(), "source-drift"),
            (
                |f| f.which = Some("/shared/rustup".into()),
                "rustup-executable-mismatch",
            ),
            (|f| f.which = None, "rustup-executable-mismatch"),
            (
                |f| f.release = "ID=ubuntu\nVERSION_ID=\"24.04\"".into(),
                "unsupported-os",
            ),
            (
                |f| f.release = "ID=debian\nVERSION_ID=\"26.04\"".into(),
                "unsupported-os",
            ),
            (
                |f| f.release = "ID=ubuntu\nVERSION_ID=\"26.04\"\nID=debian".into(),
                "unsupported-os",
            ),
            (|f| f.uname.0 = "Darwin".into(), "unsupported-platform"),
            (|f| f.uname.1 = "aarch64".into(), "unsupported-platform"),
        ];
        for (edit, expected) in cases {
            let mut fake = Fake::default();
            edit(&mut fake);
            assert_eq!(reason(&mut fake, OUTPUT), expected);
        }
        for output in [
            "/shared/receipts",
            "/home/runner/work/_temp/fvoci-rustup-ci-metadata/",
            "/home/runner/work/_temp//fvoci-rustup-ci-metadata",
            "/home/runner/work/_temp/./fvoci-rustup-ci-metadata",
            "//home/runner/work/_temp/fvoci-rustup-ci-metadata",
            "fvoci-rustup-ci-metadata",
            "",
        ] {
            assert_eq!(
                reason(&mut Fake::default(), output),
                "receipt-prefix-override",
                "{output:?}"
            );
        }
    }

    #[test]
    fn os_release_parsing() {
        assert_eq!(
            os_release("NAME=\"Ubuntu\"\nID=ubuntu\nVERSION_ID=\"26.04\"\nnoequals\n"),
            ("ubuntu".into(), "26.04".into())
        );
        assert_eq!(
            os_release("ID=\"\"ubuntu\""),
            ("ubuntu".into(), String::new())
        );
    }

    #[test]
    fn context_fields_are_the_receipt_keys() {
        let context = Context {
            source_head: "h".into(),
            source_tree: "t".into(),
            job: "j".into(),
            rustup_version: "v".into(),
            rustup_sha256: "s".into(),
            rustup_identity: [1, 2, 3, 4, 5, 6],
        };
        assert_eq!(
            super::super::json::compact(&Value::Object(context.fields())),
            r#"{"job":"j","rustup_identity":[1,2,3,4,5,6],"rustup_sha256":"s","rustup_version":"v","source_head":"h","source_tree":"t"}"#
        );
    }
}
