//! Binary wiring. Hash oracles live next to the command.

use std::path::Path;
use std::process::Command;

fn xtask(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .env("LC_ALL", "C")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap()
}

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap()
}

#[test]
fn help_prints_the_raw_comment_command() {
    let help = xtask(&["evidence-hash", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    let stdout = String::from_utf8(help.stdout).unwrap();
    assert!(stdout.contains("gh api repos/OWNER/REPO/issues/comments/<id> | jq -j .body"));
    assert!(stdout.contains("--binary --full-index"));
    assert!(stdout.contains("^1"));

    let missing = xtask(&["evidence-hash"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8(missing.stderr)
        .unwrap()
        .contains("missing --sha"));
}

#[test]
fn mismatch_exits_1() {
    let root = std::env::temp_dir().join(format!(
        "fvoci-evidence-hash-cli-mismatch-{}",
        std::process::id()
    ));
    let origin = root.join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    let cleanup = || {
        let _ = std::fs::remove_dir_all(&root);
    };
    assert!(git(&origin, &["init", "-b", "main"]).status.success());
    assert!(
        git(&origin, &["config", "user.email", "evidence@example.com"])
            .status
            .success()
    );
    assert!(git(&origin, &["config", "user.name", "Evidence"])
        .status
        .success());
    assert!(git(&origin, &["config", "commit.gpgsign", "false"])
        .status
        .success());
    std::fs::write(origin.join("a.txt"), b"one\n").unwrap();
    assert!(git(&origin, &["add", "--", "a.txt"]).status.success());
    assert!(git(&origin, &["commit", "-m", "root"]).status.success());
    std::fs::write(origin.join("a.txt"), b"two\n").unwrap();
    assert!(git(&origin, &["add", "--", "a.txt"]).status.success());
    assert!(git(&origin, &["commit", "-m", "edit"]).status.success());
    let head = String::from_utf8(git(&origin, &["rev-parse", "HEAD"]).stdout)
        .unwrap()
        .trim()
        .to_string();

    let mut body = "지시문 sha256 `".as_bytes().to_vec();
    body.extend(std::iter::repeat_n(b'a', 64));
    body.extend(b"`\n\npayload");
    let directive = root.join("directive.body");
    std::fs::write(&directive, &body).unwrap();

    let output = xtask(&[
        "evidence-hash",
        "--repo",
        origin.to_str().unwrap(),
        "--sha",
        &head,
        "--directive-file",
        directive.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(1), "{stderr}\n{stdout}");
    assert!(stdout.contains("directive: MISMATCH"), "{stdout}");
    assert!(stderr.contains("directive hash mismatch"), "{stderr}");
    cleanup();
}
