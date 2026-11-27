//! Whether the server's image is built from the source it is given, also when every file
//! of that source is older than what the build cache holds. `git archive` leaves files
//! with the time of the commit, and a worktree keeps the time it was checked out at,
//! while the image's build keeps cargo's target directory in a cache that every build
//! on the machine shares. cargo takes a source that is older than its last build for
//! unchanged, so without care such a source gets an earlier build's executable.
//!
//! The probe unpacks the checkout's commit with one line changed (a mark in the
//! server's version), dates every file in 2001, builds the build stage from that with
//! the checkout's Dockerfile, and looks for the mark in the executable. It builds with
//! a cargo cache of its own and the same build arguments every time, so that what its
//! cache holds is always its previous run's build, of another mark and newer than 2001:
//! only the files' times can then make cargo build anew (the server reads the commit
//! and its time at build time, and cargo builds anew when they change). The first run
//! on a machine fills that cache and proves nothing. Each run unpacks into a directory
//! of its own: Docker sends again only the files of a context whose time or size
//! changed since the same directory was last sent, and the probe's files keep both.
//! The layers of each run stay in Docker's build cache until `docker builder prune`.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail, ensure};

use crate::steps::{self, capture, say};

/// The file that is changed, and the version that ping reports in it, which gets the
/// mark in front.
const MARKED_FILE: &str = "crates/server/src/app/mod.rs";
const VERSION: &str = "env!(\"CARGO_PKG_VERSION\")";

/// The files of the image's build that come from the checkout rather than the commit,
/// so that what is tried is the Dockerfile as it is now.
const BUILD_FILES: [&str; 2] = [
    "docker/fvoci/Dockerfile",
    "docker/fvoci/Dockerfile.dockerignore",
];

/// The build arguments of every probe: its own cargo cache, and a commit and time that
/// do not change from one run to the next.
const BUILD_ARGUMENTS: [&str; 6] = [
    "--build-arg",
    "FVOCI_CARGO_TARGET_CACHE=fvoci-cargo-target-probe",
    "--build-arg",
    "FVOCI_BUILD_HASH=probe",
    "--build-arg",
    "FVOCI_BUILD_DATE=probe",
];

/// The time every file of the probe's source is given, in seconds after 1970: in 2001,
/// before any build this probe can meet.
const OLD_SECONDS: u64 = 1_000_000_000;

/// Builds the build stage from a marked source older than the cache, and checks that its
/// executable is that source's. The probe's image and files are removed afterwards.
///
/// # Errors
///
/// Fails if the executable lacks the mark, or a step of the probe fails.
pub(super) fn probe(root: &Path) -> Result<()> {
    say("\n==> building from a source older than the build cache");
    let parent = root.join("target/docker-smoke");
    remove_earlier(&parent)?;
    let mark = format!("fresh-{}", super::hex(&super::random::<8>()?));
    let directory = parent.join(&mark);
    let tag = format!("fvoci-fresh-probe:{}", super::checkout_key(root));
    let outcome = build_and_look(root, &directory, &tag, &mark);
    // Cleaning up is tried in full whatever happened, and says nothing over the outcome.
    let removed_image = capture("docker", &["image", "rm", "--force", &tag]);
    let removed_files = match fs::remove_dir_all(&directory) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("removing {}", directory.display()))
        }
        _ => Ok(()),
    };
    outcome?;
    let removed_image = removed_image?;
    ensure!(
        removed_image.succeeded,
        "removing the probe's image {tag}: {}",
        removed_image.error
    );
    removed_files
}

/// Removes what a killed run's probe left in `parent`.
fn remove_earlier(parent: &Path) -> Result<()> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry?.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("fresh-"))
        {
            fs::remove_dir_all(&path).with_context(|| format!("removing {}", path.display()))?;
        }
    }
    Ok(())
}

fn build_and_look(root: &Path, directory: &Path, tag: &str, mark: &str) -> Result<()> {
    fs::create_dir_all(directory).with_context(|| format!("creating {}", directory.display()))?;
    unpack_commit(root, directory)?;
    for file in BUILD_FILES {
        fs::copy(root.join(file), directory.join(file))
            .with_context(|| format!("copying {file}"))?;
    }
    put_mark(&directory.join(MARKED_FILE), mark)?;
    date_tree(
        directory,
        SystemTime::UNIX_EPOCH + Duration::from_secs(OLD_SECONDS),
    )?;

    let mut arguments = vec![
        "build",
        "--target",
        "build",
        "--file",
        BUILD_FILES[0],
        "--tag",
        tag,
    ];
    arguments.extend(BUILD_ARGUMENTS);
    arguments.push(".");
    steps::run(directory, "docker", &arguments, &[("DOCKER_BUILDKIT", "1")])?;

    let status = Command::new("docker")
        .args(["run", "--rm", "--network", "none", "--entrypoint", "grep"])
        .args([
            tag,
            "--quiet",
            "--text",
            "--fixed-strings",
            mark,
            "/fvoci-server",
        ])
        .stdout(Stdio::null())
        .status()
        .context("starting docker run")?;
    match status.code() {
        Some(0) => {
            say("    the executable is the one of the source it was given");
            Ok(())
        }
        Some(1) => bail!(
            "the executable is an earlier build's: cargo took a source older than its \
             cache for unchanged (docker/fvoci/Dockerfile must not let file times decide)"
        ),
        _ => bail!("looking into the probe's executable ended with {status}"),
    }
}

/// Unpacks the checkout's commit into `directory`, as `git archive` leaves it.
fn unpack_commit(root: &Path, directory: &Path) -> Result<()> {
    let mut archive = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["archive", "--format=tar", "HEAD"])
        .stdout(Stdio::piped())
        .spawn()
        .context("starting git archive")?;
    let tar_input = archive
        .stdout
        .take()
        .context("git archive gave no output to read")?;
    let unpacked = Command::new("tar")
        .arg("-x")
        .arg("-C")
        .arg(directory)
        .stdin(tar_input)
        .status()
        .context("starting tar");
    let archived = archive.wait().context("waiting for git archive")?;
    let unpacked = unpacked?;
    ensure!(
        archived.success() && unpacked.success(),
        "unpacking the commit: git archive ended with {archived}, tar with {unpacked}"
    );
    Ok(())
}

/// Puts `mark` at the front of the server's version in `file`.
fn put_mark(file: &Path, mark: &str) -> Result<()> {
    let text = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    ensure!(
        text.matches(VERSION).count() == 1,
        "{MARKED_FILE} has not one `{VERSION}`, which the probe marks"
    );
    let marked = text.replacen(VERSION, &format!("concat!(\"{mark}-\", {VERSION})"), 1);
    fs::write(file, marked).with_context(|| format!("writing {}", file.display()))
}

/// Gives `path` and every file and directory under it the time `time`; links are left
/// as they are.
fn date_tree(path: &Path, time: SystemTime) -> Result<()> {
    let kind = fs::symlink_metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .file_type();
    if kind.is_symlink() {
        return Ok(());
    }
    if kind.is_dir() {
        for entry in fs::read_dir(path).with_context(|| format!("reading {}", path.display()))? {
            date_tree(&entry?.path(), time)?;
        }
    }
    fs::File::open(path)
        .and_then(|file| file.set_times(fs::FileTimes::new().set_accessed(time).set_modified(time)))
        .with_context(|| format!("dating {}", path.display()))
}
