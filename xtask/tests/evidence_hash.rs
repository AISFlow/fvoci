//! Binary wiring. Hash oracles live next to the command.

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

#[test]
fn help_prints_the_raw_comment_command() {
    let help = xtask(&["evidence-hash", "--help"]);
    assert_eq!(help.status.code(), Some(0));
    let stdout = String::from_utf8(help.stdout).unwrap();
    assert!(stdout.contains("gh api repos/OWNER/REPO/issues/comments/<id> | jq -j .body"));
    assert!(stdout.contains("--binary --full-index"));

    let missing = xtask(&["evidence-hash"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(String::from_utf8(missing.stderr)
        .unwrap()
        .contains("missing --sha"));
}
