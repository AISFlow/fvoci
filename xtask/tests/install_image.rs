//! `xtask install-image` against a fake `docker` on PATH and a throwaway git
//! checkout: the refusal paths (label, ID, architecture, unreadable git or
//! docker, docker killed by a signal, tampered or foreign hand-off, wrong
//! loaded ref), that verify never builds, and the stdout/stderr split.

use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const FAKE_DOCKER: &str = r#"#!/bin/sh
printf '%s\n' "$*" >>"$FAKE_DIR/calls"
case "$1 $2" in
  "version -f")
    [ -n "$FAKE_VERSION_SIGNAL" ] && kill -9 $$
    [ -n "$FAKE_VERSION_HOLD" ] && { sleep 1.2 & printf amd64; exit 0; }
    [ -n "$FAKE_VERSION_FAIL" ] && { echo "cannot connect" >&2; exit 3; }
    case "$3" in *Arch*) echo "${FAKE_ARCH:-amd64}" ;; *) echo 29.9.0 ;; esac ;;
  "buildx version") echo "github.com/docker/buildx v0.38.0 abc" ;;
  "image inspect") [ -f "$FAKE_DIR/inspect" ] || { echo "No such image" >&2; exit 1; }; cat "$FAKE_DIR/inspect" ;;
  "save -o") printf 'TARDATA' >"$3" ;;
  "load -i") echo "Loaded image: ${FAKE_LOADED:-fvoci-rust-install:test}" ;;
  "ps -q") printf '%s\n' $FAKE_RUNNING ;;
  "inspect -f") echo "$FAKE_IMAGE" ;;
  "ps -a"|"volume ls"|"network ls") [ -n "$FAKE_LS_FAIL" ] && exit 1; [ -n "$FAKE_LEFT" ] && echo "$FAKE_LEFT"; exit 0 ;;
  build*) exit 0 ;;
  *) echo "fake docker: unexpected $*" >&2; exit 99 ;;
esac
"#;

struct Fixture {
    dir: PathBuf,
    repo: PathBuf,
}

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args([
            "-C",
            repo.to_str().unwrap(),
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

impl Fixture {
    fn new() -> Self {
        let dir = xtask::host::mkdtemp("fvoci-install-image-test-", &std::env::temp_dir()).unwrap();
        let bin = dir.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("docker"), FAKE_DOCKER).unwrap();
        fs::set_permissions(bin.join("docker"), fs::Permissions::from_mode(0o755)).unwrap();
        let repo = dir.join("repo");
        fs::create_dir_all(repo.join("infra/rust")).unwrap();
        fs::write(repo.join("infra/rust/Dockerfile"), "FROM scratch\n").unwrap();
        fs::write(repo.join(".dockerignore"), "target\n").unwrap();
        fs::write(
            repo.join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.98.1\"\n",
        )
        .unwrap();
        fs::write(repo.join(".bun-version"), "1.4.2\n").unwrap();
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "fixture"]);
        Self { dir, repo }
    }

    fn labels(&self) -> [String; 4] {
        let mut inputs = fs::read(self.repo.join("infra/rust/Dockerfile")).unwrap();
        inputs.extend(fs::read(self.repo.join(".dockerignore")).unwrap());
        [
            git(&self.repo, &["rev-parse", "HEAD^{tree}"]),
            "amd64".to_owned(),
            xtask::host::hex(&Sha256::digest(&inputs)),
            "rust=1.98.1;bun=1.4.2".to_owned(),
        ]
    }

    /// What `docker image inspect -f <template>` prints for this image.
    fn inspect(&self, id: &str, arch: &str, labels: &[String; 4]) {
        let text = format!("{id}\n{arch}\n{}\ncommit\nbuilder\n", labels.join("\n"));
        fs::write(self.dir.join("inspect"), text).unwrap();
    }

    fn good_image(&self) {
        self.inspect("sha256:good", "amd64", &self.labels());
    }

    fn xtask(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let path = format!(
            "{}:{}",
            self.dir.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command
            .arg("install-image")
            .args(args)
            .args(["--root", self.repo.to_str().unwrap()])
            .env("PATH", path)
            .env("FAKE_DIR", &self.dir)
            .env_remove("GITHUB_REPOSITORY")
            .env_remove("GITHUB_RUN_ID")
            .env_remove("GITHUB_RUN_ATTEMPT")
            .stdin(Stdio::null());
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.dir.join("calls")).unwrap_or_default()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn refused(out: &Output, needle: &str) {
    assert_eq!(out.status.code(), Some(1), "{}", stderr(out));
    assert!(out.stdout.is_empty(), "no env lines on refusal");
    assert!(
        stderr(out).contains(needle),
        "missing {needle:?} in {}",
        stderr(out)
    );
}

#[test]
fn verify_accepts_the_matching_image_and_prints_only_env_lines() {
    let f = Fixture::new();
    f.good_image();
    let out = f.xtask(&["verify", "fvoci-rust-install:test", "sha256:good"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "FVOCI_INSTALL_IMAGE=fvoci-rust-install:test\nFVOCI_INSTALL_IMAGE_ID=sha256:good\n"
    );
}

#[test]
fn verify_refuses_each_mismatch_and_never_builds() {
    let f = Fixture::new();
    let good = f.labels();
    for (i, needle) in [
        "label io.fvoci.install-image.source-tree=",
        "label io.fvoci.install-image.arch=",
        "label io.fvoci.install-image.dockerfile-sha256=",
        "label io.fvoci.install-image.toolchain=",
    ]
    .iter()
    .enumerate()
    {
        let mut labels = good.clone();
        labels[i] = String::new();
        f.inspect("sha256:good", "amd64", &labels);
        refused(
            &f.xtask(&["verify", "fvoci-rust-install:test"], &[]),
            needle,
        );
    }
    f.inspect("sha256:good", "arm64", &good);
    refused(
        &f.xtask(&["verify", "fvoci-rust-install:test"], &[]),
        "image architecture arm64",
    );
    f.good_image();
    refused(
        &f.xtask(&["verify", "fvoci-rust-install:test", "sha256:other"], &[]),
        "image ID sha256:good, expected sha256:other",
    );
    fs::remove_file(f.dir.join("inspect")).unwrap();
    refused(
        &f.xtask(&["verify", "fvoci-rust-install:test"], &[]),
        "No such image",
    );
    assert!(
        !f.calls().lines().any(|l| l.starts_with("build")),
        "{}",
        f.calls()
    );
}

#[test]
fn a_dirty_checkout_is_a_different_tree() {
    let f = Fixture::new();
    f.good_image();
    fs::write(f.repo.join("new-file"), "x").unwrap();
    refused(
        &f.xtask(&["verify", "fvoci-rust-install:test"], &[]),
        "source-tree=",
    );
}

#[test]
fn unreadable_docker_or_git_fails_closed_and_signal_differs_from_exit() {
    let f = Fixture::new();
    f.good_image();
    refused(
        &f.xtask(
            &["verify", "fvoci-rust-install:test"],
            &[("FAKE_VERSION_FAIL", "1")],
        ),
        "exited 3: cannot connect",
    );
    refused(
        &f.xtask(
            &["verify", "fvoci-rust-install:test"],
            &[("FAKE_VERSION_SIGNAL", "1")],
        ),
        "killed by signal 9",
    );
    fs::remove_dir_all(f.repo.join(".git")).unwrap();
    refused(
        &f.xtask(&["verify", "fvoci-rust-install:test"], &[]),
        "git -C",
    );
}

#[test]
fn the_read_deadline_covers_a_descendant_holding_the_output_pipe() {
    let f = Fixture::new();
    f.good_image();
    // docker exits 0 at once but leaves `sleep 1.2` holding stdout.
    let started = std::time::Instant::now();
    let out = f.xtask(
        &["verify", "fvoci-rust-install:test"],
        &[
            ("FAKE_VERSION_HOLD", "1"),
            ("FVOCI_INSTALL_IMAGE_READ_MS", "100"),
        ],
    );
    refused(&out, "timed out after 100 ms; killed");
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1000),
        "returned after {:?}",
        started.elapsed()
    );
    for bad in ["0", "300001", "-1", "1e3", " 100", ""] {
        refused(
            &f.xtask(
                &["verify", "fvoci-rust-install:test"],
                &[("FVOCI_INSTALL_IMAGE_READ_MS", bad)],
            ),
            "FVOCI_INSTALL_IMAGE_READ_MS must be 1..=300000 milliseconds",
        );
    }
}

#[test]
fn build_labels_then_verifies() {
    let f = Fixture::new();
    f.good_image();
    let out = f.xtask(&["build", "--tag", "fvoci-rust-install:test"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let calls = f.calls();
    let build = calls.lines().find(|l| l.starts_with("build ")).unwrap();
    for (key, value) in ["source-tree", "arch", "dockerfile-sha256", "toolchain"]
        .iter()
        .zip(f.labels())
    {
        assert!(
            build.contains(&format!("--label io.fvoci.install-image.{key}={value}")),
            "{build}"
        );
    }
    assert!(build.contains("-t fvoci-rust-install:test"), "{build}");
}

#[test]
fn save_then_load_round_trips_and_refuses_a_tampered_or_foreign_hand_off() {
    let f = Fixture::new();
    f.good_image();
    let handoff = f.dir.join("handoff");
    let handoff_str = handoff.to_str().unwrap();
    let run = [
        ("GITHUB_REPOSITORY", "o/r"),
        ("GITHUB_RUN_ID", "7"),
        ("GITHUB_RUN_ATTEMPT", "2"),
    ];
    let out = f.xtask(&["save", "fvoci-rust-install:test", handoff_str], &run);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let manifest = fs::read_to_string(handoff.join("image.manifest")).unwrap();
    assert!(
        manifest.contains("repository=o/r\nrun-id=7\nrun-attempt=2\n"),
        "{manifest}"
    );

    let out = f.xtask(&["load", handoff_str], &run);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(String::from_utf8_lossy(&out.stdout).contains("FVOCI_INSTALL_IMAGE_ID=sha256:good\n"));

    refused(
        &f.xtask(
            &["load", handoff_str],
            &[
                ("GITHUB_REPOSITORY", "o/r"),
                ("GITHUB_RUN_ID", "7"),
                ("GITHUB_RUN_ATTEMPT", "3"),
            ],
        ),
        "manifest run-attempt=2, this job has 3",
    );
    refused(
        &f.xtask(&["load", handoff_str], &[]),
        "manifest repository=o/r, this job has ",
    );
    refused(
        &f.xtask(
            &["load", handoff_str],
            &[("FAKE_LOADED", "other:tag")]
                .iter()
                .chain(&run)
                .copied()
                .collect::<Vec<_>>(),
        ),
        "docker load did not load fvoci-rust-install:test",
    );
    fs::write(handoff.join("image.tar"), "TAMPERED").unwrap();
    let calls_before = f.calls().lines().filter(|l| l.starts_with("load")).count();
    refused(
        &f.xtask(&["load", handoff_str], &run),
        "image.tar sha256 does not match the manifest",
    );
    assert_eq!(
        f.calls().lines().filter(|l| l.starts_with("load")).count(),
        calls_before,
        "tampered tar must not be loaded"
    );
    fs::write(
        handoff.join("image.manifest"),
        format!("{manifest}id=again\n"),
    )
    .unwrap();
    refused(
        &f.xtask(&["load", handoff_str], &run),
        "manifest repeats id",
    );
}

#[test]
fn leftovers_reports_owned_resources_and_fails_closed() {
    let f = Fixture::new();
    let out = f.xtask(&["leftovers", "--project", "p1"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(f
        .calls()
        .contains("ps -a -q --filter label=com.docker.compose.project=p1"));
    refused(
        &f.xtask(&["leftovers", "--project", "p1"], &[("FAKE_LEFT", "abc")]),
        "p1 still has: container abc, volume abc, network abc",
    );
    refused(
        &f.xtask(&["leftovers", "--project", "p1"], &[("FAKE_LS_FAIL", "1")]),
        "cannot list the resources of p1",
    );
}

#[test]
fn usage_errors_exit_2() {
    let f = Fixture::new();
    for args in [
        &[][..],
        &["verify"],
        &["leftovers"],
        &["save", "r"],
        &["verify", "r", "--tag", "x"],
        &["nope"],
    ] {
        assert_eq!(f.xtask(args, &[]).status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn acquire_verifies_the_given_image_and_refuses_to_build_in_ci() {
    let f = Fixture::new();
    f.good_image();
    let given = [
        ("FVOCI_INSTALL_IMAGE", "fvoci-rust-install:test"),
        ("FVOCI_INSTALL_IMAGE_ID", "sha256:good"),
    ];
    let out = f.xtask(&["acquire"], &given);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    refused(
        &f.xtask(
            &["acquire"],
            &[
                ("FVOCI_INSTALL_IMAGE", "fvoci-rust-install:test"),
                ("FVOCI_INSTALL_IMAGE_ID", "sha256:bad"),
            ],
        ),
        "expected sha256:bad",
    );
    refused(
        &f.xtask(
            &["acquire"],
            &[("FVOCI_INSTALL_IMAGE", ""), ("GITHUB_ACTIONS", "true")],
        ),
        "FVOCI_INSTALL_IMAGE is required in CI",
    );
    assert!(
        !f.calls().lines().any(|l| l.starts_with("build")),
        "{}",
        f.calls()
    );
    let out = f.xtask(
        &["acquire"],
        &[("FVOCI_INSTALL_IMAGE", ""), ("GITHUB_ACTIONS", "")],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        f.calls().lines().any(|l| l.starts_with("build ")),
        "a local run without an image builds"
    );
}

#[test]
fn running_requires_one_container_on_the_verified_image() {
    let f = Fixture::new();
    let args = ["running", "p1", "server", "sha256:good"];
    let ok = f.xtask(
        &args,
        &[("FAKE_RUNNING", "c1"), ("FAKE_IMAGE", "sha256:good")],
    );
    assert_eq!(ok.status.code(), Some(0), "{}", stderr(&ok));
    refused(
        &f.xtask(
            &args,
            &[("FAKE_RUNNING", "c1"), ("FAKE_IMAGE", "sha256:moved")],
        ),
        "p1/server runs image sha256:moved, not sha256:good",
    );
    refused(
        &f.xtask(
            &args,
            &[("FAKE_RUNNING", "c1 c2"), ("FAKE_IMAGE", "sha256:good")],
        ),
        "expected one running container",
    );
    refused(
        &f.xtask(&args, &[("FAKE_RUNNING", "")]),
        "expected one running container",
    );
}
