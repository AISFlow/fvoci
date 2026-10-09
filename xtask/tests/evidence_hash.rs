//! CLI wiring for `evidence-hash`. Hash oracles live in the unit tests.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

struct Repo {
    root: PathBuf,
    origin: PathBuf,
}

impl Repo {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "fvoci-evidence-hash-cli-{}-{n}",
            std::process::id()
        ));
        let origin = root.join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        let repo = Self { root, origin };
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.email", "evidence@example.com"]);
        repo.git(&["config", "user.name", "Evidence"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo
    }

    fn commit(&self, rel: &str, bytes: &[u8], message: &str) -> String {
        std::fs::write(self.origin.join(rel), bytes).unwrap();
        self.git(&["add", "--", rel]);
        self.git(&["commit", "-m", message]);
        let output = self.git_output(&["rev-parse", "HEAD"]);
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn git(&self, args: &[&str]) {
        let output = self.git_output(args);
        assert!(
            output.status.success(),
            "git {args:?}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_output(&self, args: &[&str]) -> std::process::Output {
        Command::new("git")
            .arg("-C")
            .arg(&self.origin)
            .args(args)
            .env("LC_ALL", "C")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn xtask(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap()
}

#[test]
fn cli_commit_matches_sha256sum_and_rejects_bad_input() {
    let repo = Repo::new();
    repo.commit("a.txt", b"one\n", "root");
    let head = repo.commit("a.txt", b"two\n", "edit");
    let origin = repo.origin.to_str().unwrap();

    let output = xtask(&["evidence-hash", "--repo", origin, "--sha", &head]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let oracle = Command::new("bash")
        .arg("-c")
        .arg("set -o pipefail; git diff --binary --full-index HEAD^ HEAD | sha256sum")
        .current_dir(&repo.origin)
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(oracle.status.success());
    let digest = String::from_utf8(oracle.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    assert!(stdout.contains(&format!("patch-sha256: {digest}")));
    assert_eq!(stdout.lines().count(), 5);

    let missing = xtask(&["evidence-hash"]);
    assert_eq!(missing.status.code(), Some(2));
    let stderr = String::from_utf8(missing.stderr).unwrap();
    assert!(stderr.contains("missing commit"), "{stderr}");

    let unknown = xtask(&["evidence-hash", "--repo", origin, "definitely-not-a-commit"]);
    assert_eq!(unknown.status.code(), Some(1));
    assert!(String::from_utf8(unknown.stderr)
        .unwrap()
        .contains("unknown commit"));
    assert!(unknown.stdout.is_empty());

    let help = xtask(&["evidence-hash", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .contains("patch-sha256"));
}
