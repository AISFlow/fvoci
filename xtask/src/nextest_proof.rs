//! One-shot nextest archive proof. Archive/JSON formats remain owned by nextest/jq.
//! This command replaces manual producer, masked consumer, inventory reporting,
//! list/run, and artifact identity bookkeeping. It does not accept a prior proof.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

pub const HELP: &str = "\
Usage: cargo xtask nextest-proof --image sha256:<64 hex> --namespace <suffix>
       --fixture <worktree-relative directory> --expected-tests <positive count>
       --expected-binaries <positive count> --native-helper <exact name>
       --cpus <positive integer> --memory-mib <positive integer>

Requires an explicitly allocated Docker runtime proof batch and an existing local
Cargo fixture. No product build flags, filters, retries, or prior archives are
accepted. The fixture must contain every runtime asset its tests require.
Creates target/xtask-nextest-proof-1-<suffix> exactly once. Requires installed
docker, cargo-nextest 0.9.148 in the pinned image, jq, tar/zstd, sha256sum, and gh.
Posts the exact consumer find command and discovered paths to AISFlow/fvoci #368
and verifies the comment read-back before list/run. No CI actions or pushes.
";

const NEXTEST: &str = "/usr/local/bin/cargo-nextest";
const MASKS: [&str; 3] = ["/opt/cargo", "/opt/rustup", "/home/ci/.cargo"];
const COMPILERS: [&str; 11] = [
    "cargo",
    "rustc",
    "rustup",
    "rustdoc",
    "rustfmt",
    "clippy-driver",
    "cargo-clippy",
    "cargo-fmt",
    "rust-gdb",
    "rust-gdbgui",
    "rust-lldb",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    image: String,
    namespace: String,
    fixture: PathBuf,
    tests: usize,
    binaries: usize,
    helper: String,
    cpus: usize,
    memory_mib: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProofError {
    Invalid(&'static str),
    Missing(&'static str),
    Io(&'static str),
    Failed { label: String, exit: Option<i32> },
}

impl std::fmt::Display for ProofError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "FAIL: {message}"),
            Self::Missing(message) => write!(f, "MISSING: {message}"),
            Self::Io(message) => write!(f, "FAIL: {message}"),
            Self::Failed { label, exit } => write!(
                f,
                "FAIL: {label}, exit {exit:?}; see relative evidence logs"
            ),
        }
    }
}

type Result<T> = std::result::Result<T, ProofError>;

fn positive(value: &str) -> Result<usize> {
    value
        .parse()
        .ok()
        .filter(|n| *n > 0)
        .ok_or(ProofError::Invalid(
            "count/resource must be a positive integer",
        ))
}

fn name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 48
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn relative(path: &Path) -> bool {
    !path.as_os_str().is_empty() && path.components().all(|c| matches!(c, Component::Normal(_)))
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Options> {
    let mut args = args.into_iter();
    let mut values = std::collections::BTreeMap::new();
    while let Some(flag) = args.next() {
        let flag = flag
            .into_string()
            .map_err(|_| ProofError::Invalid("non-Unicode flag"))?;
        if ![
            "--image",
            "--namespace",
            "--fixture",
            "--expected-tests",
            "--expected-binaries",
            "--native-helper",
            "--cpus",
            "--memory-mib",
        ]
        .contains(&flag.as_str())
        {
            return Err(ProofError::Invalid("unknown flag"));
        }
        let value = args
            .next()
            .ok_or(ProofError::Invalid("flag needs a value"))?
            .into_string()
            .map_err(|_| ProofError::Invalid("non-Unicode value"))?;
        if values.insert(flag, value).is_some() {
            return Err(ProofError::Invalid("duplicate flag"));
        }
    }
    let get = |key: &str| {
        values
            .get(key)
            .ok_or(ProofError::Invalid("missing required flag"))
    };
    let image = get("--image")?.clone();
    if !image.strip_prefix("sha256:").is_some_and(|v| {
        v.len() == 64
            && v.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    }) {
        return Err(ProofError::Invalid(
            "image must be an immutable sha256 digest",
        ));
    }
    let suffix = get("--namespace")?;
    let helper = get("--native-helper")?.clone();
    if !name(suffix) || !name(&helper) {
        return Err(ProofError::Invalid("invalid exact namespace/helper name"));
    }
    let fixture = PathBuf::from(get("--fixture")?);
    if !relative(&fixture) {
        return Err(ProofError::Invalid(
            "fixture must be a worktree-relative path without traversal",
        ));
    }
    Ok(Options {
        image,
        namespace: format!("xtask-nextest-proof-1-{suffix}"),
        fixture,
        tests: positive(get("--expected-tests")?)?,
        binaries: positive(get("--expected-binaries")?)?,
        helper,
        cpus: positive(get("--cpus")?)?,
        memory_mib: positive(get("--memory-mib")?)?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Preflight,
    Archive,
    PrepareConsumer,
    Inventory,
    ReportInventory,
    List,
    VerifyList,
    Run,
    VerifyRun,
}

const STEPS: [Step; 9] = [
    Step::Preflight,
    Step::Archive,
    Step::PrepareConsumer,
    Step::Inventory,
    Step::ReportInventory,
    Step::List,
    Step::VerifyList,
    Step::Run,
    Step::VerifyRun,
];

fn sequence(mut execute: impl FnMut(Step) -> Result<()>) -> Result<()> {
    for step in STEPS {
        execute(step)?;
    }
    Ok(())
}

fn empty_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ProofError::Missing("dedicated extraction directory"))?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || fs::read_dir(path)
            .map_err(|_| ProofError::Io("read extraction directory"))?
            .next()
            .is_some()
    {
        return Err(ProofError::Invalid(
            "extraction directory must be ordinary and empty",
        ));
    }
    Ok(())
}

fn require_workspace_manifest(workspace: &Path) -> Result<()> {
    if !workspace.join("Cargo.toml").is_file() {
        return Err(ProofError::Missing(
            "consumer workspace manifest before list",
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable(_: &Path) -> bool {
    false
}

// Exact names, ordinary files, executable permissions. No glob, symlink following,
// directory-as-executable, or fallback search.
fn discover(root: &Path, names: &[&str]) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(root).map_err(|_| ProofError::Io("read discovery root"))? {
        let entry = entry.map_err(|_| ProofError::Io("read discovery entry"))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|_| ProofError::Io("read discovery type"))?;
        if kind.is_dir() {
            found.extend(discover(&path, names)?);
        } else if names.iter().any(|n| entry.file_name() == *n) && executable(&path) {
            found.push(path);
        }
    }
    found.sort();
    Ok(found)
}

fn consumer_ready(root: bool, compiler_paths: &[PathBuf], producer_present: bool) -> Result<()> {
    if root {
        return Err(ProofError::Invalid("consumer must be nonroot"));
    }
    if !compiler_paths.is_empty() {
        return Err(ProofError::Invalid("compiler executable is available"));
    }
    if producer_present {
        return Err(ProofError::Invalid("producer path must be absent"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Counts {
    declared: usize,
    actual: usize,
    binaries: usize,
    ignored: usize,
    filtered: usize,
}

fn exact_counts(counts: &Counts, expected: &Options) -> Result<()> {
    if counts.actual == 0 {
        return Err(ProofError::Invalid("empty test list"));
    }
    if counts.declared != expected.tests
        || counts.actual != expected.tests
        || counts.binaries != expected.binaries
        || counts.ignored != 0
        || counts.filtered != 0
    {
        return Err(ProofError::Invalid(
            "test count/binary count mismatch, ignored or filtered test",
        ));
    }
    Ok(())
}

// jq owns JSON parsing. This is only a typed adapter over jq's five TSV numbers.
fn counts(bytes: &[u8]) -> Result<Counts> {
    let numbers = std::str::from_utf8(bytes)
        .map_err(|_| ProofError::Invalid("metadata encoding"))?
        .trim()
        .split('\t')
        .map(str::parse::<usize>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| ProofError::Invalid("metadata counts"))?;
    if numbers.len() != 5 {
        return Err(ProofError::Invalid("metadata count fields"));
    }
    Ok(Counts {
        declared: numbers[0],
        actual: numbers[1],
        binaries: numbers[2],
        ignored: numbers[3],
        filtered: numbers[4],
    })
}

const LIST_COUNTS: &str = r#"
  if (."test-count" | type) != "number" or (."rust-suites" | type) != "object" then error("missing list metadata") else . end |
  [."rust-suites"[]] as $s | [$s[].testcases[]] as $t |
  if any($s[]; .status != "listed") or any($t[]; (.ignored | type) != "boolean" or .kind != "test") then error("unlisted suite/non-test case") else . end |
  [."test-count", ($t|length), ($s|length), ([$t[]|select(.ignored)]|length), ([$t[]|select(."filter-match".status != "matches")]|length)] | @tsv
"#;

const RUN_COUNTS: &str = r#"
  [.[] | select(.type == "suite" and .event == "started")] as $start |
  [.[] | select(.type == "suite" and .event == "ok")] as $end |
  [.[] | select(.type == "test" and .event == "ok")] as $ok |
  if ($start|length) != ($end|length) or any(.[]; .event == "failed" or .event == "ignored") or any($end[]; .failed != 0 or .measured != 0) then error("incomplete/failed run") else . end |
  [([$start[].test_count]|add // 0), ($ok|length), ($end|length), ([$end[].ignored]|add // 0), ([$end[].filtered_out]|add // 0)] | @tsv
"#;

struct Runtime {
    options: Options,
    root: PathBuf,
    directory: PathBuf,
    artifacts: Vec<(PathBuf, String)>,
    compiler_paths: Vec<PathBuf>,
    container_started: bool,
    command_index: usize,
    archive_hash: String,
    nextest_hash: Vec<u8>,
}

impl Runtime {
    fn command(
        &mut self,
        label: &str,
        program: &str,
        args: &[OsString],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>> {
        self.command_index += 1;
        let log = self
            .directory
            .join("evidence")
            .join(format!("{:03}-{label}", self.command_index));
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|_| ProofError::Missing("required command executable"))?;
        if let Some(input) = input {
            child
                .stdin
                .take()
                .ok_or(ProofError::Io("command stdin"))?
                .write_all(input)
                .map_err(|_| ProofError::Io("write command stdin"))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|_| ProofError::Io("wait command"))?;
        fs::write(log.with_extension("stdout"), &output.stdout)
            .map_err(|_| ProofError::Io("stdout evidence"))?;
        fs::write(log.with_extension("stderr"), &output.stderr)
            .map_err(|_| ProofError::Io("stderr evidence"))?;
        fs::write(
            log.with_extension("exit"),
            format!("{:?}\n", output.status.code()),
        )
        .map_err(|_| ProofError::Io("exit evidence"))?;
        if !output.status.success() {
            return Err(ProofError::Failed {
                label: label.into(),
                exit: output.status.code(),
            });
        }
        Ok(output.stdout)
    }

    fn docker(&mut self, label: &str, args: Vec<OsString>) -> Result<Vec<u8>> {
        self.command(label, "docker", &args, None)
    }

    fn exec(&mut self, label: &str, args: &[&str]) -> Result<Vec<u8>> {
        let mut command = strings(&["exec", &self.options.namespace]);
        command.extend(strings(args));
        self.docker(label, command)
    }

    fn jq(&mut self, label: &str, query: &str, input: &[u8], slurp: bool) -> Result<Vec<u8>> {
        let args = strings(&[if slurp { "-ers" } else { "-er" }, query]);
        self.command(label, "jq", &args, Some(input))
    }

    fn hash(&mut self, label: &str, path: &Path) -> Result<String> {
        let output = self.command(label, "sha256sum", &[path.as_os_str().to_owned()], None)?;
        let hash = std::str::from_utf8(&output)
            .map_err(|_| ProofError::Invalid("hash encoding"))?
            .split_whitespace()
            .next()
            .ok_or(ProofError::Invalid("missing hash"))?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ProofError::Invalid("SHA256 output"));
        }
        Ok(hash.into())
    }

    fn mount(&self, source: &Path, destination: &str, readonly: bool) -> OsString {
        format!(
            "type=bind,src={},dst={destination}{}",
            source.display(),
            if readonly { ",readonly" } else { "" }
        )
        .into()
    }

    fn base_docker(&self) -> Vec<OsString> {
        strings(&[
            "run",
            "--pull=never",
            "--network",
            "none",
            "--cpus",
            &self.options.cpus.to_string(),
            "--memory",
            &format!("{}m", self.options.memory_mib),
            "--label",
            &format!("fvoci.task={}", self.options.namespace),
        ])
    }

    fn nextest(&mut self, action: &str) -> Result<Vec<u8>> {
        let mut args = strings(&["exec"]);
        if action == "run" {
            args.extend(strings(&["--env", "NEXTEST_EXPERIMENTAL_LIBTEST_JSON=1"]));
        }
        args.extend(strings(&[
            &self.options.namespace,
            NEXTEST,
            "nextest",
            action,
            "--archive-file",
            "/consumer/dist/fixture.tar.zst",
            "--extract-to",
            &format!("/consumer/extract-{action}"),
            "--workspace-remap",
            "/consumer/workspace",
            "--color",
            "never",
            "--user-config-file",
            "none",
        ]));
        if action == "list" {
            args.extend(strings(&["--message-format", "json"]));
        } else {
            args.extend(strings(&[
                "--message-format",
                "libtest-json",
                "--message-format-version",
                "0.1",
                "--no-fail-fast",
                "--retries",
                "0",
                "--no-tests",
                "fail",
            ]));
        }
        self.docker(action, args)
    }

    fn preflight(&mut self) -> Result<()> {
        for sub in [
            "consumer/extract-list",
            "consumer/extract-run",
            "empty-rust",
        ] {
            empty_directory(&self.directory.join(sub))?;
        }
        let root = self.command("host-root", "id", &strings(&["-u"]), None)?;
        if root == b"0\n" {
            return Err(ProofError::Invalid("host must be nonroot"));
        }
        self.command(
            "source-SHA",
            "git",
            &strings(&["rev-parse", "HEAD", "HEAD^"]),
            None,
        )?;
        let mut args = self.base_docker();
        args.extend(strings(&[
            "--rm",
            "--entrypoint",
            "/usr/bin/id",
            &self.options.image,
            "-u",
        ]));
        if self.docker("producer-root", args)? == b"0\n" {
            return Err(ProofError::Invalid("producer must be nonroot"));
        }
        let mut args = self.base_docker();
        args.extend(strings(&[
            "--rm",
            "--entrypoint",
            NEXTEST,
            &self.options.image,
            "--version",
        ]));
        let version = self.docker("nextest-version", args)?;
        if !std::str::from_utf8(&version).is_ok_and(|v| v.starts_with("cargo-nextest 0.9.148 ")) {
            return Err(ProofError::Invalid("requires cargo-nextest 0.9.148"));
        }
        let mut args = self.base_docker();
        args.extend(strings(&[
            "--rm",
            "--entrypoint",
            "/usr/bin/sha256sum",
            &self.options.image,
            NEXTEST,
        ]));
        self.nextest_hash = self.docker("producer-nextest-hash", args)?;
        Ok(())
    }

    fn archive(&mut self) -> Result<()> {
        let mut args = self.base_docker();
        args.extend(strings(&[
            "--rm",
            "--name",
            &format!("{}-producer", self.options.namespace),
            "--mount",
        ]));
        args.push(self.mount(
            &self.root.join(&self.options.fixture),
            "/producer/source",
            true,
        ));
        args.push("--mount".into());
        args.push(self.mount(
            &self.directory.join("producer-target"),
            "/producer/target",
            false,
        ));
        args.push("--mount".into());
        args.push(self.mount(&self.directory.join("dist"), "/dist", false));
        args.extend(strings(&[
            "--env",
            "CARGO_TARGET_DIR=/producer/target",
            "--workdir",
            "/producer/source",
            "--entrypoint",
            NEXTEST,
            &self.options.image,
            "nextest",
            "archive",
            "--offline",
            "--locked",
            "--archive-file",
            "/dist/fixture.tar.zst",
            "--color",
            "never",
            "--user-config-file",
            "none",
        ]));
        self.docker("archive-once", args)?;
        self.archive_hash = self.hash(
            "producer-archive-hash",
            &self.directory.join("dist/fixture.tar.zst"),
        )?;
        Ok(())
    }

    fn prepare_consumer(&mut self) -> Result<()> {
        for sub in [
            "consumer/extract-list",
            "consumer/extract-run",
            "empty-rust",
        ] {
            empty_directory(&self.directory.join(sub))?;
        }
        let from = self.directory.join("dist/fixture.tar.zst");
        let to = self.directory.join("consumer/dist/fixture.tar.zst");
        fs::copy(&from, &to).map_err(|_| ProofError::Io("copy own archive"))?;
        if self.hash("consumer-archive-hash", &to)? != self.archive_hash {
            return Err(ProofError::Invalid("archive identity mismatch"));
        }
        let mut args = self.base_docker();
        args.extend(strings(&[
            "-d",
            "-i",
            "--name",
            &self.options.namespace,
            "--read-only",
            "--tmpfs",
            "/tmp:rw,nosuid,nodev,size=64m",
            "--mount",
        ]));
        args.push(self.mount(&self.directory.join("consumer"), "/consumer", false));
        for destination in MASKS {
            args.push("--mount".into());
            args.push(self.mount(&self.directory.join("empty-rust"), destination, true));
        }
        args.extend(strings(&[
            "--env",
            "PATH=/usr/local/bin",
            "--env",
            "XDG_STATE_HOME=/consumer/state",
            "--env",
            "XDG_DATA_HOME=/consumer/data",
            "--env",
            "XDG_CACHE_HOME=/consumer/cache",
            "--workdir",
            "/consumer/workspace",
            "--entrypoint",
            "/bin/cat",
            &self.options.image,
        ]));
        self.docker("consumer-create", args)?;
        self.container_started = true;
        let mounts = self.docker(
            "physical-mounts",
            strings(&[
                "inspect",
                "--format",
                "{{range .Mounts}}{{.Destination}} {{.RW}}{{println}}{{end}}",
                &self.options.namespace,
            ]),
        )?;
        let mounts =
            std::str::from_utf8(&mounts).map_err(|_| ProofError::Invalid("mount encoding"))?;
        for destination in MASKS {
            if !mounts
                .lines()
                .any(|line| line == format!("{destination} false"))
            {
                return Err(ProofError::Invalid(
                    "physical readonly installation mask missing",
                ));
            }
            if !self
                .exec(
                    "mask-empty",
                    &["/usr/bin/find", destination, "-mindepth", "1", "-print"],
                )?
                .is_empty()
            {
                return Err(ProofError::Invalid("physical installation mask not empty"));
            }
        }
        let root = self.exec("consumer-root", &["/usr/bin/id", "-u"])? == b"0\n";
        if self.exec("consumer-nextest-hash", &["/usr/bin/sha256sum", NEXTEST])?
            != self.nextest_hash
        {
            return Err(ProofError::Invalid(
                "producer/consumer nextest tool hash mismatch",
            ));
        }
        for path in ["/producer", "/usr/local/cargo", "/usr/local/rustup"] {
            self.exec("producer-absent", &["/usr/bin/test", "!", "-e", path])?;
        }
        consumer_ready(root, &[], false)?;
        Ok(())
    }

    fn inventory(&mut self) -> Result<()> {
        let args = find_arguments();
        let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
        let output = self.exec("compiler-discovery", &refs)?;
        let paths =
            std::str::from_utf8(&output).map_err(|_| ProofError::Invalid("discovery encoding"))?;
        self.compiler_paths = paths.lines().map(PathBuf::from).collect();
        // Report the inventory even when it will reject compiler availability.
        Ok(())
    }

    fn report_inventory(&mut self) -> Result<()> {
        let paths = if self.compiler_paths.is_empty() {
            "0 paths".into()
        } else {
            self.compiler_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let body = format!("[from:codex] nextest-proof pre-list inventory\n\nworktree-local namespace: {}\nfind command: `{}`\nfound paths:\n```text\n{paths}\n```\nCompiler availability rejects list/run. This is preparation evidence, not independent review or acceptance.\n", self.options.namespace, find_arguments().join(" "));
        let response = self.command(
            "inventory-comment",
            "gh",
            &strings(&[
                "api",
                "repos/AISFlow/fvoci/issues/368/comments",
                "--method",
                "POST",
                "--field",
                &format!("body={body}"),
            ]),
            None,
        )?;
        let id = self.jq("comment-id", ".id | tostring", &response, false)?;
        let id = std::str::from_utf8(&id)
            .map_err(|_| ProofError::Invalid("comment ID encoding"))?
            .trim();
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
            return Err(ProofError::Invalid("comment ID"));
        }
        let readback = self.command(
            "inventory-readback",
            "gh",
            &strings(&[
                "api",
                &format!("repos/AISFlow/fvoci/issues/comments/{id}"),
                "--jq",
                ".body",
            ]),
            None,
        )?;
        if readback != format!("{body}\n").as_bytes() {
            return Err(ProofError::Invalid("inventory report read-back mismatch"));
        }
        consumer_ready(false, &self.compiler_paths, false)
    }

    fn verify_artifacts(&mut self, extraction: &str) -> Result<()> {
        let metadata = self.command(
            "archive-metadata",
            "tar",
            &[
                "-I".into(),
                "zstd".into(),
                "-xOf".into(),
                self.directory.join("dist/fixture.tar.zst").into_os_string(),
                "target/nextest/binaries-metadata.json".into(),
            ],
            None,
        )?;
        let query = r#"."rust-build-meta"."target-directory" as $root | ((."rust-binaries"[] | ["test", (."binary-path" | ltrimstr($root + "/"))]), (."rust-build-meta"."non-test-binaries"[][] | ["helper", .name, .path])) | @tsv"#;
        let rows = self.jq("artifact-paths", query, &metadata, false)?;
        let mut artifacts = Vec::new();
        let mut paths = BTreeSet::new();
        let mut helper_names = Vec::new();
        let mut test_count = 0;
        for line in std::str::from_utf8(&rows)
            .map_err(|_| ProofError::Invalid("artifact encoding"))?
            .lines()
        {
            let fields = line.split('\t').collect::<Vec<_>>();
            let path = match fields.as_slice() {
                ["test", path] => {
                    test_count += 1;
                    PathBuf::from(path)
                }
                ["helper", helper, path] => {
                    helper_names.push(*helper);
                    PathBuf::from(path)
                }
                _ => return Err(ProofError::Invalid("artifact metadata fields")),
            };
            if !relative(&path) {
                return Err(ProofError::Invalid("artifact must be target-relative"));
            }
            if !paths.insert(path.clone()) {
                return Err(ProofError::Invalid("duplicate artifact path"));
            }
            let producer = self.directory.join("producer-target").join(&path);
            let consumer = self
                .directory
                .join(format!("consumer/extract-{extraction}/target"))
                .join(&path);
            for file in [&producer, &consumer] {
                if !executable(file) {
                    return Err(ProofError::Invalid(
                        "artifact must be an ordinary executable",
                    ));
                }
                let bytes = fs::read(file).map_err(|_| ProofError::Io("read ELF artifact"))?;
                if !bytes.starts_with(b"\x7fELF") {
                    return Err(ProofError::Invalid("artifact is not ELF"));
                }
            }
            let hash = self.hash("producer-ELF", &producer)?;
            if self.hash("consumer-ELF", &consumer)? != hash {
                return Err(ProofError::Invalid("ELF/native helper hash mismatch"));
            }
            artifacts.push((path, hash));
        }
        if test_count != self.options.binaries || helper_names != [self.options.helper.as_str()] {
            return Err(ProofError::Invalid(
                "exact test ELF/native helper inventory mismatch",
            ));
        }
        let helper = self.options.helper.as_str();
        if discover(&self.directory.join("producer-target"), &[helper])?.len() != 1
            || discover(
                &self
                    .directory
                    .join(format!("consumer/extract-{extraction}/target")),
                &[helper],
            )?
            .len()
                != 1
        {
            return Err(ProofError::Invalid(
                "exact ordinary executable native helper discovery",
            ));
        }
        artifacts.sort();
        if extraction == "list" {
            self.artifacts = artifacts;
        } else if self.artifacts != artifacts {
            return Err(ProofError::Invalid("list/run artifact identity mismatch"));
        }
        if self.hash(
            "archive-still-identical",
            &self.directory.join("consumer/dist/fixture.tar.zst"),
        )? != self.archive_hash
        {
            return Err(ProofError::Invalid("archive changed"));
        }
        Ok(())
    }

    fn execute(&mut self, step: Step) -> Result<()> {
        match step {
            Step::Preflight => self.preflight(),
            Step::Archive => self.archive(),
            Step::PrepareConsumer => self.prepare_consumer(),
            Step::Inventory => self.inventory(),
            Step::ReportInventory => self.report_inventory(),
            Step::List => {
                require_workspace_manifest(&self.directory.join("consumer/workspace"))?;
                self.exec(
                    "workspace-manifest-before-list",
                    &["/usr/bin/test", "-f", "/consumer/workspace/Cargo.toml"],
                )?;
                let out = self.nextest("list")?;
                fs::write(self.directory.join("evidence/list.json"), out)
                    .map_err(|_| ProofError::Io("list evidence"))
            }
            Step::VerifyList => {
                let json = fs::read(self.directory.join("evidence/list.json"))
                    .map_err(|_| ProofError::Missing("list output"))?;
                let parsed = self.jq("list-counts", LIST_COUNTS, &json, false)?;
                exact_counts(&counts(&parsed)?, &self.options)?;
                self.verify_artifacts("list")
            }
            Step::Run => {
                let out = self.nextest("run")?;
                fs::write(self.directory.join("evidence/run.jsonl"), out)
                    .map_err(|_| ProofError::Io("run evidence"))
            }
            Step::VerifyRun => {
                let json = fs::read(self.directory.join("evidence/run.jsonl"))
                    .map_err(|_| ProofError::Missing("run output"))?;
                let parsed = self.jq("run-counts", RUN_COUNTS, &json, true)?;
                exact_counts(&counts(&parsed)?, &self.options)?;
                self.verify_artifacts("run")
            }
        }
    }
}

fn strings(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

fn find_arguments() -> Vec<String> {
    let mut args = [
        "/usr/bin/find",
        "/opt",
        "/usr/local",
        "/home/ci",
        "-type",
        "f",
        "-perm",
        "/111",
        "(",
    ]
    .map(String::from)
    .to_vec();
    for (index, name) in COMPILERS.iter().enumerate() {
        if index != 0 {
            args.push("-o".into());
        }
        args.extend(["-name".into(), (*name).into()]);
    }
    args.push(")".into());
    args
}

pub fn run(args: impl IntoIterator<Item = OsString>) -> Result<()> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args == [OsString::from("--help")] || args == [OsString::from("-h")] {
        print!("{HELP}");
        return Ok(());
    }
    let options = parse(args)?;
    let root = std::env::current_dir().map_err(|_| ProofError::Io("worktree cwd"))?;
    let fixture = root
        .join(&options.fixture)
        .canonicalize()
        .map_err(|_| ProofError::Missing("fixture"))?;
    if !fixture.starts_with(&root)
        || !fixture.join("Cargo.toml").is_file()
        || !fixture.join("Cargo.lock").is_file()
    {
        return Err(ProofError::Invalid(
            "fixture must be local with an existing Cargo.toml/Cargo.lock",
        ));
    }
    let directory = root.join("target").join(&options.namespace);
    // Atomic namespace creation: never reuse a proof, archive, or extraction root.
    fs::create_dir(&directory)
        .map_err(|_| ProofError::Invalid("namespace must be fresh; target must already exist"))?;
    for sub in [
        "evidence",
        "producer-target",
        "dist",
        "consumer",
        "empty-rust",
        "consumer/dist",
        "consumer/workspace",
        "consumer/state",
        "consumer/data",
        "consumer/cache",
        "consumer/extract-list",
        "consumer/extract-run",
    ] {
        fs::create_dir(directory.join(sub))
            .map_err(|_| ProofError::Io("create dedicated proof directory"))?;
    }
    // Preserve runtime assets at a distinct consumer path. Symlinks are refused.
    copy_fixture(&fixture, &directory.join("consumer/workspace"))?;
    let mut runtime = Runtime {
        options,
        root,
        directory,
        artifacts: Vec::new(),
        compiler_paths: Vec::new(),
        container_started: false,
        command_index: 0,
        archive_hash: String::new(),
        nextest_hash: Vec::new(),
    };
    let result = sequence(|step| runtime.execute(step));
    // Cleanup is allowed on failure; it never executes a later proof phase.
    let cleanup = if runtime.container_started {
        runtime
            .docker(
                "own-consumer-cleanup",
                strings(&["rm", "-f", &runtime.options.namespace]),
            )
            .map(|_| ())
    } else {
        Ok(())
    };
    result?;
    cleanup?;
    println!("PASS: {} tests, {} test ELF, native helper1, ignored0; evidence target/{}/evidence. Self-check is not independent acceptance.", runtime.options.tests, runtime.options.binaries, runtime.options.namespace);
    Ok(())
}

fn copy_fixture(from: &Path, to: &Path) -> Result<()> {
    for entry in fs::read_dir(from).map_err(|_| ProofError::Io("read fixture"))? {
        let entry = entry.map_err(|_| ProofError::Io("fixture entry"))?;
        if entry.file_name() == "target" || entry.file_name() == ".git" {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|_| ProofError::Io("fixture file type"))?;
        let destination = to.join(entry.file_name());
        if kind.is_dir() {
            fs::create_dir(&destination).map_err(|_| ProofError::Io("fixture directory"))?;
            copy_fixture(&entry.path(), &destination)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), destination)
                .map_err(|_| ProofError::Io("copy fixture asset"))?;
        } else {
            return Err(ProofError::Invalid(
                "fixture symlink/special file is not allowed",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn options() -> Options {
        parse(strings(&[
            "--image",
            &format!("sha256:{}", "a".repeat(64)),
            "--namespace",
            "unit",
            "--fixture",
            "target/local-fixture",
            "--expected-tests",
            "3",
            "--expected-binaries",
            "2",
            "--native-helper",
            "nativechild",
            "--cpus",
            "1",
            "--memory-mib",
            "256",
        ]))
        .unwrap()
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static INDEX: AtomicUsize = AtomicUsize::new(0);
            let parent = Path::new("target/xtask-nextest-proof-1");
            fs::create_dir_all(parent).unwrap();
            let path = parent.join(format!(
                "unit-{}-{}",
                std::process::id(),
                INDEX.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        #[cfg(unix)]
        fn file(&self, name: &str, mode: u32) {
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(mode)
                .open(self.0.join(name))
                .unwrap();
            file.write_all(b"local fixture\n").unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn parses_only_explicit_resources_and_fresh_namespace() {
        let options = options();
        assert_eq!(options.namespace, "xtask-nextest-proof-1-unit");
        assert_eq!(
            (
                options.tests,
                options.binaries,
                options.cpus,
                options.memory_mib
            ),
            (3, 2, 1, 256)
        );
    }

    #[test]
    fn rejects_unknown_duplicate_missing_flags_and_unicode_errors() {
        assert_eq!(
            parse(strings(&["--retry", "1"])),
            Err(ProofError::Invalid("unknown flag"))
        );
        assert_eq!(
            parse(strings(&["--cpus", "1", "--cpus", "2"])),
            Err(ProofError::Invalid("duplicate flag"))
        );
        assert_eq!(
            parse(strings(&["--cpus"])),
            Err(ProofError::Invalid("flag needs a value"))
        );
        assert_eq!(
            parse(Vec::new()),
            Err(ProofError::Invalid("missing required flag"))
        );
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            assert_eq!(
                parse([OsString::from_vec(vec![255])]),
                Err(ProofError::Invalid("non-Unicode flag"))
            );
        }
    }

    #[test]
    fn rejects_traversal_tags_wildcards_and_zero_counts() {
        for path in ["", "../fixture", "foo/../fixture", "/fixture"] {
            assert!(!relative(Path::new(path)));
        }
        for value in ["", "*", "nativechild?", "../root", "name with space"] {
            assert!(!name(value));
        }
        for value in ["0", "-1", "1.5", "", "184467440737095516160"] {
            assert!(positive(value).is_err());
        }
        let mut args = strings(&[
            "--image",
            "latest",
            "--namespace",
            "unit",
            "--fixture",
            "local",
            "--expected-tests",
            "3",
            "--expected-binaries",
            "2",
            "--native-helper",
            "nativechild",
            "--cpus",
            "1",
            "--memory-mib",
            "256",
        ]);
        assert_eq!(
            parse(args.clone()),
            Err(ProofError::Invalid(
                "image must be an immutable sha256 digest"
            ))
        );
        args[1] = format!("sha256:{}", "a".repeat(64)).into();
        args[3] = "../unit".into();
        assert_eq!(
            parse(args),
            Err(ProofError::Invalid("invalid exact namespace/helper name"))
        );
    }

    #[test]
    fn every_failed_phase_stops_all_later_commands_without_retry() {
        for failed in STEPS {
            let mut called = Vec::new();
            let result = sequence(|step| {
                called.push(step);
                if step == failed {
                    Err(ProofError::Invalid("negative control"))
                } else {
                    Ok(())
                }
            });
            assert_eq!(result, Err(ProofError::Invalid("negative control")));
            let index = STEPS.iter().position(|step| *step == failed).unwrap();
            assert_eq!(called, STEPS[..=index]);
            assert_eq!(
                called.iter().filter(|step| **step == Step::Archive).count(),
                usize::from(index >= 1)
            );
        }
    }

    #[test]
    fn positive_sequence_archives_once_reports_before_list_and_runs_once() {
        let mut called = Vec::new();
        sequence(|step| {
            called.push(step);
            Ok(())
        })
        .unwrap();
        assert_eq!(called, STEPS);
        assert_eq!(
            called.iter().filter(|step| **step == Step::Archive).count(),
            1
        );
        assert!(
            called
                .iter()
                .position(|step| *step == Step::ReportInventory)
                < called.iter().position(|step| *step == Step::List)
        );
    }

    #[cfg(unix)]
    #[test]
    fn discovery_requires_exact_name_ordinary_file_and_execute_bit() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        fixture.file("nativechild", 0o700);
        fixture.file("nativechild-wrong", 0o700);
        fixture.file("cargo", 0o600);
        fs::create_dir(fixture.0.join("rustc")).unwrap();
        symlink("nativechild", fixture.0.join("rustup")).unwrap();
        assert_eq!(
            discover(&fixture.0, &["nativechild"]).unwrap(),
            [fixture.0.join("nativechild")]
        );
        assert!(discover(&fixture.0, &["cargo", "rustc", "rustup"])
            .unwrap()
            .is_empty());
        assert!(discover(&fixture.0, &["child", "native*"])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn extraction_roots_are_distinct_fresh_empty_ordinary_directories() {
        let fixture = Fixture::new();
        for name in ["extract-list", "extract-run"] {
            let path = fixture.0.join(name);
            assert_eq!(
                empty_directory(&path),
                Err(ProofError::Missing("dedicated extraction directory"))
            );
            fs::create_dir(&path).unwrap();
            empty_directory(&path).unwrap();
            assert!(fs::create_dir(&path).is_err());
        }
        assert_ne!(
            fixture.0.join("extract-list"),
            fixture.0.join("extract-run")
        );
        fs::write(fixture.0.join("extract-list/stale"), "old proof").unwrap();
        assert_eq!(
            empty_directory(&fixture.0.join("extract-list")),
            Err(ProofError::Invalid(
                "extraction directory must be ordinary and empty"
            ))
        );
    }

    #[test]
    fn workspace_manifest_must_exist_as_a_file_before_list() {
        let fixture = Fixture::new();
        assert_eq!(
            require_workspace_manifest(&fixture.0),
            Err(ProofError::Missing(
                "consumer workspace manifest before list"
            ))
        );
        fs::create_dir(fixture.0.join("Cargo.toml")).unwrap();
        assert!(require_workspace_manifest(&fixture.0).is_err());
        fs::remove_dir(fixture.0.join("Cargo.toml")).unwrap();
        fs::write(fixture.0.join("Cargo.toml"), "[workspace]\n").unwrap();
        require_workspace_manifest(&fixture.0).unwrap();
    }

    #[test]
    fn rejects_root_compiler_available_and_producer_present() {
        consumer_ready(false, &[], false).unwrap();
        assert_eq!(
            consumer_ready(true, &[], false),
            Err(ProofError::Invalid("consumer must be nonroot"))
        );
        assert_eq!(
            consumer_ready(false, &[PathBuf::from("/opt/rustup/bin/rustc")], false),
            Err(ProofError::Invalid("compiler executable is available"))
        );
        assert_eq!(
            consumer_ready(false, &[], true),
            Err(ProofError::Invalid("producer path must be absent"))
        );
    }

    #[test]
    fn typed_metadata_accepts_exact_count_and_rejects_empty_mismatch_ignored_filtered() {
        let good = Counts {
            declared: 3,
            actual: 3,
            binaries: 2,
            ignored: 0,
            filtered: 0,
        };
        exact_counts(&good, &options()).unwrap();
        for changed in [
            Counts {
                actual: 0,
                ..good.clone()
            },
            Counts {
                declared: 4,
                ..good.clone()
            },
            Counts {
                actual: 2,
                ..good.clone()
            },
            Counts {
                binaries: 1,
                ..good.clone()
            },
            Counts {
                ignored: 1,
                ..good.clone()
            },
            Counts {
                filtered: 1,
                ..good.clone()
            },
        ] {
            assert!(exact_counts(&changed, &options()).is_err());
        }
        assert_eq!(counts(b"3\t3\t2\t0\t0\n").unwrap(), good);
        for invalid in [
            b"".as_slice(),
            b"3\t3\t2",
            b"3\t3\t2\tfalse\t0",
            b"3\t3\t2\t0\t0\t9",
        ] {
            assert!(counts(invalid).is_err());
        }
    }

    fn jq(query: &str, input: &[u8], slurp: bool) -> std::process::Output {
        let mut child = Command::new("jq")
            .args([if slurp { "-ers" } else { "-er" }, query])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        child.wait_with_output().unwrap()
    }

    #[test]
    fn standard_jq_parses_nextest_list_fixture_and_rejects_missing_or_skipped_metadata() {
        let fixture = br#"{"test-count":3,"rust-suites":{"lib":{"status":"listed","testcases":{"a":{"kind":"test","ignored":false,"filter-match":{"status":"matches"}},"b":{"kind":"test","ignored":false,"filter-match":{"status":"matches"}}}},"integration":{"status":"listed","testcases":{"c":{"kind":"test","ignored":false,"filter-match":{"status":"matches"}}}}}}"#;
        let output = jq(LIST_COUNTS, fixture, false);
        assert!(output.status.success());
        exact_counts(&counts(&output.stdout).unwrap(), &options()).unwrap();
        assert!(!jq(LIST_COUNTS, b"{}", false).status.success());
        let skipped = String::from_utf8(fixture.to_vec())
            .unwrap()
            .replace("listed", "skipped");
        assert!(!jq(LIST_COUNTS, skipped.as_bytes(), false).status.success());
    }

    #[test]
    fn standard_jq_parses_run_events_and_rejects_failed_or_incomplete_runs() {
        let fixture = b"{\"type\":\"suite\",\"event\":\"started\",\"test_count\":2}\n{\"type\":\"suite\",\"event\":\"started\",\"test_count\":1}\n{\"type\":\"test\",\"event\":\"ok\"}\n{\"type\":\"test\",\"event\":\"ok\"}\n{\"type\":\"test\",\"event\":\"ok\"}\n{\"type\":\"suite\",\"event\":\"ok\",\"passed\":2,\"failed\":0,\"ignored\":0,\"measured\":0,\"filtered_out\":0}\n{\"type\":\"suite\",\"event\":\"ok\",\"passed\":1,\"failed\":0,\"ignored\":0,\"measured\":0,\"filtered_out\":0}\n";
        let output = jq(RUN_COUNTS, fixture, true);
        assert!(output.status.success());
        exact_counts(&counts(&output.stdout).unwrap(), &options()).unwrap();
        assert!(!jq(
            RUN_COUNTS,
            b"{\"type\":\"suite\",\"event\":\"started\",\"test_count\":3}\n",
            true
        )
        .status
        .success());
        assert!(!jq(
            RUN_COUNTS,
            b"{\"type\":\"test\",\"event\":\"failed\"}\n",
            true
        )
        .status
        .success());
    }

    #[test]
    fn real_local_git_fixture_records_actual_sha_without_touching_product_git() {
        let fixture = Fixture::new();
        assert!(Command::new("git")
            .args(["init", "--quiet"])
            .arg(&fixture.0)
            .status()
            .unwrap()
            .success());
        let git = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&fixture.0)
                .args(args)
                .output()
                .unwrap()
        };
        assert!(git(&[
            "-c",
            "user.name=Local Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "local fixture"
        ])
        .status
        .success());
        let head = git(&["rev-parse", "HEAD"]);
        assert!(head.status.success());
        let sha = std::str::from_utf8(&head.stdout).unwrap().trim();
        assert_eq!(sha.len(), 40);
        assert!(sha.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(!git(&["rev-parse", "--verify", "missing-ref"])
            .status
            .success());
    }
}
