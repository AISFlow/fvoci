//! `cargo xtask install-image`: build the infra/rust install image once,
//! prove its identity, and hand it between CI jobs; plus the container smokes'
//! leftover check.
//!
//!   install-image build [--tag REF]      build + label from this checkout
//!   install-image verify REF [ID]        refuse an image not built from this checkout
//!   install-image save REF DIR           DIR/image.tar + DIR/image.manifest
//!   install-image load DIR               check the hand-off, docker load, verify
//!   install-image acquire                the smokes' image: verify FVOCI_INSTALL_IMAGE[_ID],
//!                                        or build when unset (refused in CI)
//!   install-image running P S ID         exactly one running container of P/S, on image ID
//!   install-image leftovers --project P  fail if P still owns a container, volume or network
//!
//! `build`, `verify`, `save` and `load` print `FVOCI_INSTALL_IMAGE=<ref>` and
//! `FVOCI_INSTALL_IMAGE_ID=<id>` on stdout (CI appends them to `$GITHUB_ENV`);
//! everything else goes to stderr. Identity labels, all compared by verify:
//!   io.fvoci.install-image.source-tree        tree of the checkout (HEAD^{tree} when
//!                                             clean; else tracked + untracked, non-ignored)
//!   io.fvoci.install-image.arch               Docker daemon architecture (also the image's)
//!   io.fvoci.install-image.dockerfile-sha256  infra/rust/Dockerfile + .dockerignore
//!   io.fvoci.install-image.toolchain          rust-toolchain.toml channel + .bun-version
//! Recorded only: source-commit (HEAD) and builder (Docker engine/buildx), which
//! are not image inputs. Every git or docker read that fails stops the command
//! (exit 1); a mismatch is refused and nothing here rebuilds on refusal.
//! Limit: git-ignored files that .dockerignore does not exclude reach the build
//! context but not the tree hash; CI builds from a fresh checkout, which has none.
//!
//! `--root DIR` (default: this repository) selects the checkout.

use crate::host;
use crate::process::{self, RunError};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

const LABEL: &str = "io.fvoci.install-image";
const DEFAULT_TAG: &str = "fvoci-rust-install:local";
const LABEL_KEYS: [&str; 4] = ["source-tree", "arch", "dockerfile-sha256", "toolchain"];
const MANIFEST_KEYS: [&str; 6] = [
    "ref",
    "id",
    "tar-sha256",
    "repository",
    "run-id",
    "run-attempt",
];
/// Producer identity in the manifest, compared with the loading job's environment.
const RUN_KEYS: [(&str, &str); 3] = [
    ("repository", "GITHUB_REPOSITORY"),
    ("run-id", "GITHUB_RUN_ID"),
    ("run-attempt", "GITHUB_RUN_ATTEMPT"),
];

const READ: Duration = Duration::from_secs(300);
const BUILD: Duration = Duration::from_secs(4 * 3600);
const TRANSFER: Duration = Duration::from_secs(1800);

const USAGE: &str = "\
usage: cargo xtask install-image build [--tag REF] [--root DIR]
       cargo xtask install-image verify REF [ID] [--root DIR]
       cargo xtask install-image save REF DIR [--root DIR]
       cargo xtask install-image load DIR [--root DIR]
       cargo xtask install-image acquire [--root DIR]   (FVOCI_INSTALL_IMAGE[_ID] from the environment)
       cargo xtask install-image running PROJECT SERVICE IMAGE_ID
       cargo xtask install-image leftovers --project NAME
";

type Result<T> = std::result::Result<T, String>;

/// `exited N` or `killed by signal N`, never one for the other.
pub fn describe(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited {code}"),
        (None, Some(signal)) => format!("killed by signal {signal}"),
        (None, None) => "ended without a status".to_owned(),
    }
}

/// How a child's output is handled.
enum Output {
    /// Collect stdout and stderr; stderr is shown on failure.
    Capture,
    /// Child stdout and stderr both go to our stderr (our stdout stays the env lines).
    ToStderr,
}

fn child(program: &str, args: &[&str], timeout: Duration, output: Output) -> Result<Vec<u8>> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    let capture = matches!(output, Output::Capture);
    if !capture {
        let stderr = io::stderr()
            .as_fd()
            .try_clone_to_owned()
            .map_err(|e| format!("cannot pass stderr to {program}: {e}"))?;
        command.stdout(Stdio::from(stderr));
    }
    let shown = format!("{program} {}", args.join(" "));
    let done = process::run(&mut command, timeout, capture).map_err(|error| match error {
        RunError::Spawn(e) => format!("cannot start {shown}: {e}"),
        RunError::Wait(e) => format!("cannot wait for {shown}: {e}"),
        RunError::Timeout(_) => format!("{shown}: {error}; killed"),
    })?;
    if !done.status.success() {
        let detail = String::from_utf8_lossy(&done.stderr);
        return Err(format!(
            "{shown} {}{}",
            describe(done.status),
            if detail.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", detail.trim())
            }
        ));
    }
    Ok(done.stdout)
}

fn text(program: &str, args: &[&str], timeout: Duration) -> Result<String> {
    let out = child(program, args, timeout, Output::Capture)?;
    String::from_utf8(out).map_err(|_| format!("{program} {}: output is not UTF-8", args.join(" ")))
}

fn line(program: &str, args: &[&str]) -> Result<String> {
    let value = text(program, args, READ)?.trim().to_owned();
    if value.is_empty() {
        return Err(format!("{program} {}: empty output", args.join(" ")));
    }
    Ok(value)
}

fn root_str(root: &Path) -> Result<&str> {
    root.to_str()
        .ok_or_else(|| format!("checkout path is not UTF-8: {}", root.display()))
}

/// Tree id of the checkout: HEAD^{tree} when clean, else the working tree
/// hashed through a throwaway index (the real index is untouched).
pub fn source_tree(root: &Path) -> Result<String> {
    let r = root_str(root)?;
    let status = text(
        "git",
        &["-C", r, "status", "--porcelain", "--untracked-files=normal"],
        READ,
    )?;
    let tree = if status.is_empty() {
        line("git", &["-C", r, "rev-parse", "HEAD^{tree}"])?
    } else {
        let index_path = line(
            "git",
            &[
                "-C",
                r,
                "rev-parse",
                "--path-format=absolute",
                "--git-path",
                "index",
            ],
        )?;
        let dir = host::temp_root(|name| std::env::var_os(name));
        let index = host::mkstemp("fvoci-install-image-index.", &dir)
            .map_err(|e| format!("cannot create a temporary index: {e}"))?;
        let result = (|| {
            fs::copy(&index_path, &index).map_err(|e| format!("cannot copy {index_path}: {e}"))?;
            let index_str = index.to_str().ok_or("temporary index path is not UTF-8")?;
            let env_git = |args: &[&str]| -> Result<String> {
                let mut command = Command::new("git");
                command
                    .env("GIT_INDEX_FILE", index_str)
                    .args(["-C", r])
                    .args(args)
                    .stdin(Stdio::null());
                let done = process::run(&mut command, READ, true)
                    .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
                if !done.status.success() {
                    return Err(format!(
                        "git {} {}: {}",
                        args.join(" "),
                        describe(done.status),
                        String::from_utf8_lossy(&done.stderr).trim()
                    ));
                }
                Ok(String::from_utf8_lossy(&done.stdout).trim().to_owned())
            };
            env_git(&["add", "-A"])?;
            env_git(&["write-tree"])
        })();
        let _ = fs::remove_file(&index);
        result?
    };
    let valid = (40..=64).contains(&tree.len()) && tree.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid {
        return Err(format!("unexpected source tree id: {tree:?}"));
    }
    Ok(tree)
}

fn read_file(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))
}

/// The four identity labels of this checkout, in LABEL_KEYS order.
pub fn expected_labels(root: &Path) -> Result<Vec<(&'static str, String)>> {
    let tree = source_tree(root)?;
    let arch = line("docker", &["version", "-f", "{{.Server.Arch}}"])?;
    let mut inputs = read_file(&root.join("infra/rust/Dockerfile"))?;
    inputs.extend(read_file(&root.join(".dockerignore"))?);
    let toolchain_file =
        String::from_utf8_lossy(&read_file(&root.join("rust-toolchain.toml"))?).into_owned();
    let channel = toolchain_file
        .lines()
        .find_map(|l| l.strip_prefix("channel = \"")?.strip_suffix('"'))
        .filter(|c| !c.is_empty())
        .ok_or("no channel in rust-toolchain.toml")?
        .to_owned();
    let bun: String = String::from_utf8_lossy(&read_file(&root.join(".bun-version"))?)
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if bun.is_empty() {
        return Err("empty .bun-version".to_owned());
    }
    Ok(vec![
        ("source-tree", tree),
        ("arch", arch),
        ("dockerfile-sha256", host::sha256_hex(&inputs)),
        ("toolchain", format!("rust={channel};bun={bun}")),
    ])
}

fn emit(out: &mut dyn Write, reference: &str, id: &str) -> Result<()> {
    write!(
        out,
        "FVOCI_INSTALL_IMAGE={reference}\nFVOCI_INSTALL_IMAGE_ID={id}\n"
    )
    .and_then(|()| out.flush())
    .map_err(|e| format!("cannot write the image reference: {e}"))
}

/// Verify REF against this checkout (and ID when given); returns the image ID.
pub fn verify(root: &Path, reference: &str, want_id: Option<&str>) -> Result<String> {
    let expected = expected_labels(root)?;
    let mut template = String::from("{{.Id}}{{\"\\n\"}}{{.Architecture}}");
    for key in LABEL_KEYS.iter().chain(&["source-commit", "builder"]) {
        let _ = write!(
            template,
            "{{{{\"\\n\"}}}}{{{{index .Config.Labels \"{LABEL}.{key}\"}}}}"
        );
    }
    let fields = text(
        "docker",
        &["image", "inspect", "-f", &template, reference],
        READ,
    )
    .map_err(|e| format!("{e} (build it: cargo xtask install-image build)"))?;
    let got: Vec<&str> = fields
        .strip_suffix('\n')
        .unwrap_or(&fields)
        .split('\n')
        .collect();
    if got.len() != 2 + LABEL_KEYS.len() + 2 {
        return Err(format!(
            "docker image inspect {reference}: unexpected output {fields:?}"
        ));
    }
    let id = got[0];
    if let Some(want) = want_id {
        if id != want {
            return Err(format!(
                "refusing {reference}: image ID {id}, expected {want}"
            ));
        }
    }
    let mut refusals = Vec::new();
    for (i, (key, want)) in expected.iter().enumerate() {
        if got[2 + i] != want {
            refusals.push(format!(
                "label {LABEL}.{key}={:?}, this checkout has {want:?}",
                got[2 + i]
            ));
        }
    }
    if got[1] != expected[1].1 {
        refusals.push(format!(
            "image architecture {}, daemon {}",
            got[1], expected[1].1
        ));
    }
    if !refusals.is_empty() {
        return Err(format!(
            "refusing {reference}: {}; rebuild it from this checkout: cargo xtask install-image build --tag {reference}",
            refusals.join("; ")
        ));
    }
    eprintln!(
        "install-image: verified {reference} id={id} commit={} builder={}",
        got[6], got[7]
    );
    Ok(id.to_owned())
}

pub fn build(root: &Path, tag: &str) -> Result<String> {
    let started = std::time::Instant::now();
    let r = root_str(root)?;
    let mut labels = Vec::new();
    for (key, value) in expected_labels(root)? {
        labels.push(format!("{LABEL}.{key}={value}"));
    }
    let commit = line("git", &["-C", r, "rev-parse", "HEAD"])?;
    let engine = line("docker", &["version", "-f", "{{.Server.Version}}"])?;
    let buildx = line("docker", &["buildx", "version"])?;
    let buildx: String = buildx.split(' ').take(2).collect::<Vec<_>>().join(" ");
    labels.push(format!("{LABEL}.source-commit={commit}"));
    labels.push(format!("org.opencontainers.image.revision={commit}"));
    labels.push(format!("{LABEL}.builder=docker {engine}; {buildx}"));
    let dockerfile = root.join("infra/rust/Dockerfile");
    let dockerfile = root_str(&dockerfile)?.to_owned();
    let mut argv: Vec<&str> = vec!["build", "-f", &dockerfile];
    for label in &labels {
        argv.push("--label");
        argv.push(label);
    }
    argv.extend(["-t", tag, r]);
    eprintln!("install-image: building {tag} from {r}");
    child("docker", &argv, BUILD, Output::ToStderr)?;
    eprintln!(
        "install-image: built {tag} in {}s",
        started.elapsed().as_secs()
    );
    verify(root, tag, None)
}

fn file_sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(host::hex(&hasher.finalize()))
}

fn env_value(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

pub fn save(root: &Path, reference: &str, dir: &Path) -> Result<String> {
    let id = verify(root, reference, None)?;
    fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let tar = dir.join("image.tar");
    let started = std::time::Instant::now();
    child(
        "docker",
        &["save", "-o", root_str(&tar)?, reference],
        TRANSFER,
        Output::ToStderr,
    )?;
    let mut manifest = format!(
        "ref={reference}\nid={id}\ntar-sha256={}\n",
        file_sha256(&tar)?
    );
    for (key, var) in RUN_KEYS {
        let value = env_value(var);
        if value.contains('\n') {
            return Err(format!("{var} contains a newline"));
        }
        let _ = writeln!(manifest, "{key}={value}");
    }
    fs::write(dir.join("image.manifest"), manifest)
        .map_err(|e| format!("cannot write {}: {e}", dir.join("image.manifest").display()))?;
    let size = fs::metadata(&tar).map(|m| m.len()).unwrap_or(0);
    eprintln!(
        "install-image: saved {reference} ({size} bytes) in {}s",
        started.elapsed().as_secs()
    );
    Ok(id)
}

/// `key=value` lines; every known key exactly once, no other line.
pub fn parse_manifest(text: &str) -> Result<Vec<(String, String)>> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("manifest line without '=': {line:?}"))?;
        if !MANIFEST_KEYS.contains(&key) {
            return Err(format!("unknown manifest key {key:?}"));
        }
        if entries.iter().any(|(k, _)| k == key) {
            return Err(format!("manifest repeats {key}"));
        }
        entries.push((key.to_owned(), value.to_owned()));
    }
    for key in MANIFEST_KEYS {
        if !entries.iter().any(|(k, _)| k == key) {
            return Err(format!("manifest has no {key}"));
        }
    }
    Ok(entries)
}

pub fn load(root: &Path, dir: &Path) -> Result<(String, String)> {
    let tar = dir.join("image.tar");
    let manifest_path = dir.join("image.manifest");
    let manifest = String::from_utf8(read_file(&manifest_path)?)
        .map_err(|_| "image.manifest is not UTF-8".to_owned())?;
    let entries = parse_manifest(&manifest)?;
    let get = |key: &str| {
        entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or_default()
    };
    // The producer must be this repository, run and attempt (no foreign or stale artifact).
    for (key, var) in RUN_KEYS {
        let current = env_value(var);
        if get(key) != current {
            return Err(format!(
                "manifest {key}={}, this job has {current}",
                get(key)
            ));
        }
    }
    if file_sha256(&tar)? != get("tar-sha256") {
        return Err("image.tar sha256 does not match the manifest".to_owned());
    }
    let reference = get("ref").to_owned();
    let started = std::time::Instant::now();
    let loaded = text("docker", &["load", "-i", root_str(&tar)?], TRANSFER)?;
    eprint!("{loaded}");
    if !loaded
        .lines()
        .any(|l| l == format!("Loaded image: {reference}"))
    {
        return Err(format!("docker load did not load {reference}"));
    }
    eprintln!(
        "install-image: loaded {reference} in {}s",
        started.elapsed().as_secs()
    );
    let id = verify(root, &reference, Some(get("id")))?;
    Ok((reference, id))
}

/// The smokes' image: FVOCI_INSTALL_IMAGE (and FVOCI_INSTALL_IMAGE_ID when set)
/// must verify; unset, a local run builds it and CI (GITHUB_ACTIONS=true)
/// refuses, since CI builds it once per architecture.
pub fn acquire(root: &Path) -> Result<(String, String)> {
    let reference = env_value("FVOCI_INSTALL_IMAGE");
    if !reference.is_empty() {
        let want = env_value("FVOCI_INSTALL_IMAGE_ID");
        let id = verify(
            root,
            &reference,
            Some(want.as_str()).filter(|w| !w.is_empty()),
        )?;
        return Ok((reference, id));
    }
    if env_value("GITHUB_ACTIONS") == "true" {
        return Err("FVOCI_INSTALL_IMAGE is required in CI: the image is built once per arch by cargo xtask install-image build; this smoke does not build it".to_owned());
    }
    eprintln!("install-image: FVOCI_INSTALL_IMAGE unset: building the image for this checkout");
    build(root, DEFAULT_TAG).map(|id| (DEFAULT_TAG.to_owned(), id))
}

/// Exactly one running container of PROJECT/SERVICE, and it runs image ID
/// (so a tag moved after verify cannot put another image under test).
pub fn running(project: &str, service: &str, image: &str) -> Result<()> {
    let project_filter = format!("label=com.docker.compose.project={project}");
    let service_filter = format!("label=com.docker.compose.service={service}");
    let ids = text(
        "docker",
        &[
            "ps",
            "-q",
            "--filter",
            &project_filter,
            "--filter",
            &service_filter,
        ],
        READ,
    )?;
    let ids: Vec<&str> = ids.split_whitespace().collect();
    let [id] = ids.as_slice() else {
        return Err(format!(
            "{project}/{service}: expected one running container, got {ids:?}"
        ));
    };
    let got = line("docker", &["inspect", "-f", "{{.Image}}", id])?;
    if got != image {
        return Err(format!("{project}/{service} runs image {got}, not {image}"));
    }
    Ok(())
}

/// Containers, volumes and networks that still carry the project's compose label.
pub fn leftovers(project: &str) -> Result<Vec<String>> {
    let filter = format!("label=com.docker.compose.project={project}");
    let mut found = Vec::new();
    for (kind, args) in [
        ("container", vec!["ps", "-a", "-q", "--filter", &filter]),
        ("volume", vec!["volume", "ls", "-q", "--filter", &filter]),
        ("network", vec!["network", "ls", "-q", "--filter", &filter]),
    ] {
        for id in text("docker", &args, READ)?.split_whitespace() {
            found.push(format!("{kind} {id}"));
        }
    }
    Ok(found)
}

fn default_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Exit status: 0, 1 (refused, failed or leftovers), 2 (usage).
#[derive(Debug, Default, PartialEq)]
struct Cli {
    positional: Vec<String>,
    root: Option<String>,
    tag: Option<String>,
    project: Option<String>,
}

/// `--root`, `--tag`, `--project` as `--name value` or `--name=value`, each at
/// most once, anywhere after the subcommand; everything else is positional.
fn parse_cli(argv: Vec<OsString>) -> std::result::Result<Cli, String> {
    let mut cli = Cli::default();
    let mut args = argv.into_iter();
    while let Some(arg) = args.next() {
        let arg = arg
            .into_string()
            .map_err(|_| "non-UTF-8 argument".to_owned())?;
        let Some(body) = arg.strip_prefix("--") else {
            cli.positional.push(arg);
            continue;
        };
        let (name, inline) = match body.split_once('=') {
            Some((name, value)) => (name, Some(value.to_owned())),
            None => (body, None),
        };
        let slot = match name {
            "root" => &mut cli.root,
            "tag" => &mut cli.tag,
            "project" => &mut cli.project,
            "help" => return Err(String::new()),
            _ => return Err(format!("unrecognized argument {arg}")),
        };
        let value = match inline {
            Some(value) => value,
            None => args
                .next()
                .and_then(|v| v.into_string().ok())
                .ok_or_else(|| format!("argument --{name}: expected one argument"))?,
        };
        if value.is_empty() || slot.replace(value).is_some() {
            return Err(format!("argument --{name}: one non-empty value"));
        }
    }
    Ok(cli)
}

/// Exit status: 0, 1 (refused, failed or leftovers), 2 (usage).
pub fn run(argv: Vec<OsString>, out: &mut dyn Write) -> i32 {
    let cli = match parse_cli(argv) {
        Ok(cli) => cli,
        Err(message) if message.is_empty() => {
            let _ = out.write_all(USAGE.as_bytes());
            return 0;
        }
        Err(message) => {
            eprint!("install-image: {message}\n{USAGE}");
            return 2;
        }
    };
    let root = cli.root.as_deref().map_or_else(default_root, PathBuf::from);
    let usage = || {
        eprint!("{USAGE}");
        2
    };
    let positional: Vec<&str> = cli.positional.iter().map(String::as_str).collect();
    let only =
        |tag: bool, project: bool| (tag || cli.tag.is_none()) && (project || cli.project.is_none());
    let owned = |reference: &str, id: String| (reference.to_owned(), id);
    let result = match positional.as_slice() {
        ["build"] if only(true, false) => {
            let tag = cli.tag.as_deref().unwrap_or(DEFAULT_TAG);
            build(&root, tag).map(|id| owned(tag, id))
        }
        ["verify", reference] if only(false, false) => {
            verify(&root, reference, None).map(|id| owned(reference, id))
        }
        ["verify", reference, id] if only(false, false) => {
            verify(&root, reference, Some(id)).map(|id| owned(reference, id))
        }
        ["save", reference, dir] if only(false, false) => {
            save(&root, reference, Path::new(dir)).map(|id| owned(reference, id))
        }
        ["load", dir] if only(false, false) => load(&root, Path::new(dir)),
        ["acquire"] if only(false, false) => acquire(&root),
        ["running", project, service, image] if only(false, false) => {
            return match running(project, service, image) {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("install-image: {error}");
                    1
                }
            };
        }
        ["leftovers"] if only(false, true) => {
            let Some(project) = cli.project.as_deref() else {
                return usage();
            };
            return match leftovers(project) {
                Ok(found) if found.is_empty() => 0,
                Ok(found) => {
                    eprintln!("cleanup: {project} still has: {}", found.join(", "));
                    1
                }
                Err(error) => {
                    eprintln!("cleanup: cannot list the resources of {project}: {error}");
                    1
                }
            };
        }
        _ => return usage(),
    };
    match result.and_then(|(reference, id)| emit(out, &reference, &id)) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("install-image: {error}");
            1
        }
    }
}

pub fn main(argv: Vec<OsString>) -> i32 {
    run(argv, &mut io::stdout().lock())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn exit_and_signal_are_described_apart() {
        let exited = Command::new("sh").args(["-c", "exit 9"]).status().unwrap();
        let killed = Command::new("sh")
            .args(["-c", "kill -9 $$"])
            .status()
            .unwrap();
        assert_eq!(describe(exited), "exited 9");
        assert_eq!(describe(killed), "killed by signal 9");
    }

    #[test]
    fn manifest_needs_every_key_once_and_nothing_else() {
        let good = "ref=r\nid=i\ntar-sha256=s\nrepository=\nrun-id=1\nrun-attempt=2\n";
        assert_eq!(parse_manifest(good).unwrap().len(), 6);
        for bad in [
            "ref=r\nid=i\ntar-sha256=s\nrepository=\nrun-id=1\n",
            &format!("{good}ref=other\n"),
            &format!("{good}extra=1\n"),
            &format!("{good}no-equals\n"),
        ] {
            assert!(parse_manifest(bad).is_err(), "{bad:?}");
        }
    }
}
