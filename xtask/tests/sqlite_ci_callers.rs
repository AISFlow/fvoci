//! Port of the caller and workflow-text controls in
//! scripts/fixtures/sqlite-ci/test-wiring.py: local root callers go through
//! the prerequisite and stop on its failure, and every root output cache and
//! compilation in the workflows follows usable SQLite preparation. The text
//! contract is deliberately bounded; the CI planner separately parses YAML.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn temp() -> TempDir {
    TempDir(xtask::host::mkdtemp("fvoci-sqlite-callers-", &std::env::temp_dir()).unwrap())
}

fn write_exec(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

// ---------------------------------------------------------------- callers

struct CallerRoot {
    _temp: TempDir,
    root: PathBuf,
    bin: PathBuf,
    trace: PathBuf,
}

impl CallerRoot {
    fn new(scripts: &[&str]) -> Self {
        let temp = temp();
        let root = temp.0.clone();
        let bin = root.join("bin");
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::create_dir_all(root.join("apps/web")).unwrap();
        fs::create_dir(&bin).unwrap();
        for name in scripts {
            fs::copy(
                repo().join("scripts").join(name),
                root.join("scripts").join(name),
            )
            .unwrap();
        }
        let trace = root.join("trace");
        fs::write(&trace, "").unwrap();
        Self {
            _temp: temp,
            root,
            bin,
            trace,
        }
    }

    fn command(&self, script: &str) -> Command {
        let mut command = Command::new("bash");
        command
            .arg(self.root.join("scripts").join(script))
            .env(
                "PATH",
                format!("{}:{}", self.bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("TRACE", &self.trace);
        command
    }

    fn trace(&self) -> String {
        fs::read_to_string(&self.trace).unwrap()
    }
}

#[test]
fn local_root_callers_require_preflight_and_forward_cargo_arguments() {
    let r = CallerRoot::new(&["generate-api.sh", "run-db-tests.sh"]);
    write_exec(
        &r.bin.join("bun"),
        "#!/usr/bin/env bash\necho 'fixture bun'\n",
    );
    write_exec(
        &r.bin.join("cargo"),
        "#!/usr/bin/env bash\necho 'caller bypassed SQLite prerequisite' >&2\nexit 97\n",
    );
    let target = r.root.join("target");
    fs::create_dir_all(target.join("debug")).unwrap();
    write_exec(
        &target.join("debug/fvoci-export-openapi"),
        "#!/usr/bin/env bash\nprintf '{}\\n'\n",
    );
    // Stub only the build prerequisite, recording the actual wrapper argv.
    write_exec(
        &r.root.join("scripts/prepare-sqlite-ci.sh"),
        "#!/usr/bin/env bash\nset -euo pipefail\nprintf '%s\\n' \"$*\" >>\"$TRACE\"\nexit \"${PREREQ_EXIT:-0}\"\n",
    );
    for (script, expected) in [
        (
            "generate-api.sh",
            "-- cargo build --locked --offline --bin fvoci-export-openapi --features api-schema",
        ),
        (
            "run-db-tests.sh",
            "-- cargo test --locked --offline --features db-tests --test db_integration -- --nocapture",
        ),
    ] {
        let run = |exit: Option<&str>| {
            let mut command = r.command(script);
            command
                .env("TEST_DATABASE_URL", "fixture-unused-url")
                .env("CARGO_TARGET_DIR", &target);
            match exit {
                Some(code) => command.env("PREREQ_EXIT", code),
                None => command.env_remove("PREREQ_EXIT"),
            };
            command.output().unwrap()
        };
        let out = run(None);
        assert_eq!(out.status.code(), Some(0), "{script}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(r.trace().lines().last(), Some(expected), "{script}");
        assert_eq!(run(Some("19")).status.code(), Some(19), "{script}");
    }
}

#[test]
fn web_build_stops_on_preflight_failure() {
    let r = CallerRoot::new(&["run-web-e2e.sh"]);
    write_exec(
        &r.bin.join("bun"),
        "#!/usr/bin/env bash\necho \"bun $*\" >>\"$TRACE\"\n[ \"$*\" = '--bun x --no-install playwright --version' ]\n",
    );
    write_exec(
        &r.root.join("scripts/prepare-sqlite-ci.sh"),
        "#!/usr/bin/env bash\nprintf '%s\\n' \"preflight $*\" >>\"$TRACE\"\nexit 19\n",
    );
    let out = r.command("run-web-e2e.sh").output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let trace = r.trace();
    assert!(trace.contains("preflight --env-file"), "{trace}");
    assert!(!trace.contains("helper "));
    assert!(!trace.contains("cargo"));
    assert!(!trace.contains("bun run"));
}

// ------------------------------------------------------- workflow contract

type Check = Result<(), String>;

fn ensure(condition: bool, message: impl FnOnce() -> String) -> Check {
    if condition {
        Ok(())
    } else {
        Err(message())
    }
}

fn find(text: &str, needle: &str) -> Result<usize, String> {
    text.find(needle)
        .ok_or_else(|| format!("substring not found: {needle:?}"))
}

fn less(a: usize, b: usize, what: &str) -> Check {
    ensure(a < b, || format!("{what}: {a} not less than {b}"))
}

fn less_equal(a: usize, b: usize, what: &str) -> Check {
    ensure(a <= b, || {
        format!("{what}: {a} not less than or equal to {b}")
    })
}

fn contains(text: &str, needle: &str, what: &str) -> Check {
    ensure(text.contains(needle), || {
        format!("{what}: {needle:?} not found")
    })
}

fn excludes(text: &str, needle: &str, what: &str) -> Check {
    ensure(!text.contains(needle), || {
        format!("{what}: {needle:?} unexpectedly found")
    })
}

fn has_line(text: &str, line: &str) -> bool {
    text.lines().any(|l| l == line)
}

fn line_starts(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    text.split_inclusive('\n').map(move |line| {
        let start = offset;
        offset += line.len();
        (start, line.strip_suffix('\n').unwrap_or(line))
    })
}

fn is_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Two-space keys `  name:` delimit job bodies (`^  [a-z][\w-]*:`).
fn is_boundary(line: &str) -> bool {
    line.strip_prefix("  ")
        .and_then(|rest| rest.split_once(':'))
        .is_some_and(|(name, _)| is_name(name))
}

/// `dict(re.findall(r'^  ([a-z][\w-]*):\n(.*?)(?=^  [a-z][\w-]*:|\Z)', ...))`.
fn jobs(text: &str) -> Vec<(String, (usize, usize))> {
    let lines: Vec<(usize, &str)> = line_starts(text).collect();
    let mut out: Vec<(String, (usize, usize))> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let (start, line) = lines[i];
        let header = line
            .strip_prefix("  ")
            .and_then(|rest| rest.strip_suffix(':'))
            .filter(|name| is_name(name) && start + line.len() < text.len());
        if let Some(name) = header {
            let body_start = start + line.len() + 1;
            let mut j = i + 1;
            while j < lines.len() && !is_boundary(lines[j].1) {
                j += 1;
            }
            let body_end = lines.get(j).map_or(text.len(), |(s, _)| *s);
            out.retain(|(n, _)| n != name);
            out.push((name.to_owned(), (body_start, body_end)));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn job<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    jobs(text)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, (s, e))| &text[s..e])
}

/// Step blocks: from a `      - ` line to the next one (`^      - .*?(?=^      - |\Z)`).
fn blocks(body: &str) -> Vec<(usize, usize)> {
    let starts: Vec<usize> = line_starts(body)
        .filter(|(_, line)| line.starts_with("      - "))
        .map(|(s, _)| s)
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, s)| (*s, starts.get(i + 1).copied().unwrap_or(body.len())))
        .collect()
}

fn line_value<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.lines().find_map(|l| l.strip_prefix(prefix))
}

fn is_target_path_line(line: &str) -> bool {
    line.strip_prefix("          path: target")
        .is_some_and(|rest| rest.is_empty() || (rest.starts_with('/') && rest.len() > 1))
}

fn root_work_line(line: &str) -> bool {
    let cargo = line.match_indices("cargo ").any(|(i, _)| {
        let rest = &line[i + 6..];
        ["build", "test", "clippy", "check"].iter().any(|verb| {
            rest.strip_prefix(verb).is_some_and(|after| {
                !after
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphanumeric() || c == '_')
            })
        })
    });
    let selected = cargo
        || line.contains("scripts/ci_selection.py rust-binaries build")
        || line.contains("bash scripts/run-web-e2e.sh");
    // Independent workspaces with their own lockfile and target dir and no
    // SQLite dependency: helper crates under crates/ and the xtask crate.
    let crate_manifest = ["crates/", "xtask/"].iter().any(|dir| {
        ["", "\"", "'"]
            .iter()
            .any(|quote| line.contains(&format!("--manifest-path {quote}{dir}")))
    });
    selected && !crate_manifest
}

fn masked(block: &str) -> bool {
    block.lines().any(|l| {
        l.strip_prefix("        ")
            .is_some_and(|r| r.starts_with("if:") || r.starts_with("continue-on-error:"))
    })
}

const IDENTITY: &str = "${{ steps.sqlite.outputs.cache_identity }}";
const COLLAB_LANES: [&str; 5] = [
    "collaboration-install-on",
    "collaboration-postgres-on",
    "collaboration-sqlite-on",
    "collaboration-postgres-off",
    "collaboration-sqlite-off",
];

fn contract(root: &Path) -> Check {
    let consumers: [(&str, &[&str]); 3] = [
        (
            "rust",
            &[
                "fast",
                "native-arm64",
                "postgres-build",
                "postgres",
                "collaboration",
            ],
        ),
        (
            "web",
            &[
                "web-checks",
                "web-native-checks",
                "workspace-browser-build",
                "workspace-browser-shard",
                "collaboration-build",
                "collaboration-install-on",
                "collaboration-postgres-on",
                "collaboration-sqlite-on",
                "collaboration-postgres-off",
                "collaboration-sqlite-off",
            ],
        ),
        ("documents", &["native-extraction"]),
    ];
    for (workflow, expected) in consumers {
        let text = fs::read_to_string(root.join(format!(".github/workflows/{workflow}.yml")))
            .map_err(|e| e.to_string())?;
        let all = jobs(&text);
        let mut actual: Vec<&str> = all
            .iter()
            .filter(|(_, (s, e))| has_line(&text[*s..*e], "        id: sqlite"))
            .map(|(n, _)| n.as_str())
            .collect();
        actual.sort_unstable();
        let mut wanted = expected.to_vec();
        wanted.sort_unstable();
        ensure(actual == wanted, || {
            format!("{workflow}: SQLite jobs {actual:?} != {wanted:?}")
        })?;
        for name in expected {
            let body = job(&text, name).ok_or_else(|| format!("missing job {name}"))?;
            let what = format!("{workflow}/{name}");
            let what = what.as_str();
            if workflow == "rust" && *name == "postgres" {
                excludes(body, "path: target", what)?;
                contains(body, "Restore prepared SQLite prefix", what)?;
                less(
                    find(body, "Verify cached SQLite prefix")?,
                    find(body, "Download required postgres")?,
                    what,
                )?;
            } else if workflow == "web" && *name == "workspace-browser-shard" {
                excludes(body, "path: target", what)?;
                contains(body, "needs: [ci-plan, workspace-browser-build]", what)?;
                contains(
                    body,
                    "artifact-ids: ${{ needs.workspace-browser-build.outputs.artifact_id }}",
                    what,
                )?;
                contains(body, "--ci-consume-browser", what)?;
                less(
                    find(body, "id: sqlite")?,
                    find(body, "Download this run")?,
                    what,
                )?;
            } else if workflow == "web" && COLLAB_LANES.contains(name) {
                // Each lane prepares its own SDK before admitting the producer's exact artifact.
                excludes(body, "path: target", what)?;
                excludes(body, "path: crates/collab-engine/target", what)?;
                if *name == "collaboration-install-on" {
                    contains(body, "needs: [ci-plan, collaboration-build]\n", what)?;
                } else {
                    contains(
                        body,
                        "needs: [ci-plan, collaboration-build, collaboration-install-on]\n",
                        what,
                    )?;
                }
                contains(
                    body,
                    "artifact-ids: ${{ needs.collaboration-build.outputs.artifact_id }}",
                    what,
                )?;
                contains(
                    body,
                    "FVOCI_WEB_BUILD_HANDOFF_SHA256: ${{ needs.collaboration-build.outputs.handoff_sha256 }}",
                    what,
                )?;
                contains(
                    body,
                    "bash scripts/run-web-e2e.sh --ci-use-committed-api --ci-consume-selected",
                    what,
                )?;
                less(
                    find(body, "id: sqlite")?,
                    find(body, "Download this run")?,
                    what,
                )?;
                less(
                    find(body, "Download this run")?,
                    find(body, "id: browser")?,
                    what,
                )?;
            } else {
                less(find(body, "id: sqlite")?, find(body, "path: target")?, what)?;
                let after = &body[find(body, "path: target")? + "path: target".len()..];
                let root_cache = after.split("      - ").next().unwrap_or_default();
                contains(root_cache, IDENTITY, what)?;
                excludes(root_cache, "restore-keys:", what)?;
            }
            contains(
                body,
                "--github-env \"$GITHUB_ENV\" --github-output \"$GITHUB_OUTPUT\"",
                what,
            )?;
            contains(body, "libclang-18-dev=1:18.1.8-20ubuntu8", what)?;
            // Every root output cache and compilation must follow usable
            // preparation; source-download caches do not contain build output.
            let steps = blocks(body);
            let text_of = |(s, e): (usize, usize)| &body[s..e];
            let prepare = *steps
                .iter()
                .find(|b| has_line(text_of(**b), "        id: sqlite"))
                .ok_or("missing preparation block")?;
            let mut ready = prepare;
            if workflow == "rust" {
                let verified: Vec<_> = steps
                    .iter()
                    .filter(|b| {
                        text_of(**b).contains("name: Verify cached SQLite prefix or build\n")
                    })
                    .collect();
                ensure(verified.len() == 1, || {
                    format!("{what}: required SQLite verification step missing/duplicated")
                })?;
                ready = *verified[0];
                less_equal(prepare.1, ready.0, what)?;
                let restore = *steps
                    .iter()
                    .find(|b| text_of(**b).contains("name: Restore prepared SQLite prefix\n"))
                    .ok_or("missing prefix restore")?;
                less_equal(prepare.1, restore.0, what)?;
                less_equal(restore.1, ready.0, what)?;
                contains(
                    text_of(ready),
                    "--expected-cache-identity \"${{ steps.sqlite.outputs.cache_identity }}\"",
                    what,
                )?;
                contains(text_of(ready), "--github-env \"$GITHUB_ENV\"", what)?;
            }
            for preparation in [prepare, ready] {
                ensure(!masked(text_of(preparation)), || {
                    format!("{what}: preparation cannot be skipped or masked")
                })?;
            }
            for block in &steps {
                let step = text_of(*block);
                if step.lines().any(is_target_path_line) {
                    less_equal(ready.1, block.0, what)?;
                    if step.contains("actions/cache/save@") {
                        let primary = step.split("key: ${{ steps.").nth(1).and_then(|rest| {
                            rest.split_once(".outputs.cache-primary-key }}")
                                .map(|(id, _)| id)
                                .filter(|id| {
                                    !id.is_empty()
                                        && id.chars().all(|c| c.is_ascii_lowercase() || c == '_')
                                })
                        });
                        let restored: Vec<&(usize, usize)> = match primary {
                            None => {
                                let unqualified =
                                    || format!("{what}: save must use its qualified restore key");
                                let literal =
                                    line_value(step, "          key: ").ok_or_else(unqualified)?;
                                let hit = step
                                    .split("steps.")
                                    .skip(1)
                                    .find_map(|rest| {
                                        rest.split_once(".outputs.cache-hit")
                                            .map(|(id, _)| id)
                                            .filter(|id| {
                                                !id.is_empty()
                                                    && id.chars().all(|c| {
                                                        c.is_ascii_alphanumeric() || c == '_'
                                                    })
                                            })
                                    })
                                    .ok_or_else(unqualified)?;
                                let restored: Vec<_> = steps
                                    .iter()
                                    .filter(|b| {
                                        has_line(text_of(**b), &format!("        id: {hit}"))
                                    })
                                    .collect();
                                ensure(restored.len() == 1, unqualified)?;
                                let restore_key =
                                    line_value(text_of(*restored[0]), "          key: ")
                                        .ok_or_else(unqualified)?;
                                ensure(restore_key == literal, unqualified)?;
                                restored
                            }
                            Some(id) => {
                                let restored: Vec<_> = steps
                                    .iter()
                                    .filter(|b| {
                                        has_line(text_of(**b), &format!("        id: {id}"))
                                    })
                                    .collect();
                                ensure(restored.len() == 1, || {
                                    format!("{what}: restore for {id}")
                                })?;
                                restored
                            }
                        };
                        let source = text_of(*restored[0]);
                        less_equal(restored[0].1, block.0, what)?;
                        contains(source, "actions/cache/restore@", what)?;
                        contains(source, IDENTITY, what)?;
                        let path = |t: &str| {
                            t.lines()
                                .find(|l| l.starts_with("          path: target"))
                                .map(str::to_owned)
                        };
                        ensure(path(step) == path(source), || {
                            format!("{what}: save/restore path differs")
                        })?;
                    } else {
                        contains(step, IDENTITY, what)?;
                    }
                    excludes(step, "restore-keys:", what)?;
                }
                // Documents defaults to the independent native crate; root steps override it.
                if workflow != "documents" || step.contains("working-directory: .") {
                    let root_work = !step.contains("working-directory: crates/")
                        && step.lines().any(root_work_line);
                    if root_work {
                        less_equal(ready.1, block.0, what)?;
                    }
                }
                if step.contains("path: crates/collab-engine/target") {
                    ensure(
                        !step.contains("steps.sqlite.outputs.cache_identity"),
                        || format!("{what}: independent helper cache must retain its own inputs"),
                    )?;
                }
            }
        }
        if workflow == "documents" {
            let body = job(&text, "native-extraction").ok_or("missing native-extraction")?;
            less(
                find(body, "Production helper rejects test controls")?,
                find(body, "id: sqlite")?,
                "documents",
            )?;
            let after = &body[find(body, "id: sqlite")? + "id: sqlite".len()..];
            contains(
                after.split("      - ").next().unwrap_or_default(),
                "working-directory: .",
                "documents",
            )?;
        }
    }
    for workflow in ["collab-engine", "install"] {
        let text = fs::read_to_string(root.join(format!(".github/workflows/{workflow}.yml")))
            .map_err(|e| e.to_string())?;
        excludes(&text, "prepare-sqlite-ci", workflow)?;
    }
    let docker =
        fs::read_to_string(root.join("infra/rust/Dockerfile")).map_err(|e| e.to_string())?;
    let (builder, runtime) = docker.split_once(" AS runtime").ok_or("no runtime stage")?;
    contains(builder, "libclang-18-dev=1:18.1.8-20ubuntu8", "Dockerfile")?;
    contains(
        builder,
        "FROM ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7 AS ubuntu",
        "Dockerfile",
    )?;
    contains(builder, "FROM ubuntu AS rust-sources", "Dockerfile")?;
    contains(builder, "FROM ubuntu AS web-build", "Dockerfile")?;
    contains(
        builder,
        "prepare-sqlite-ci.sh --parent /sqlite-build -- cargo build",
        "Dockerfile",
    )?;
    for tool in ["libclang", "python3", "gcc", "/sqlite-build"] {
        excludes(runtime, tool, "Dockerfile runtime")?;
    }
    Ok(())
}

#[test]
fn workflow_root_cache_preparation_order_and_independent_crates() {
    contract(&repo()).unwrap();
}

/// Copy the contract inputs into a throwaway tree for mutation checks.
fn contract_copy() -> (TempDir, PathBuf) {
    let temp = temp();
    let root = temp.0.clone();
    fs::create_dir_all(root.join(".github/workflows")).unwrap();
    fs::create_dir_all(root.join("infra/rust")).unwrap();
    for workflow in ["rust", "web", "documents", "collab-engine", "install"] {
        let name = format!(".github/workflows/{workflow}.yml");
        fs::copy(repo().join(&name), root.join(&name)).unwrap();
    }
    fs::copy(
        repo().join("infra/rust/Dockerfile"),
        root.join("infra/rust/Dockerfile"),
    )
    .unwrap();
    (temp, root)
}

fn job_span(text: &str, name: &str) -> (usize, usize) {
    jobs(text)
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, span)| span)
        .unwrap()
}

fn replace_once(text: &str, old: &str, new: &str) -> String {
    text.replacen(old, new, 1)
}

fn assert_mutations_fail(
    path: &Path,
    root: &Path,
    original: &str,
    span: (usize, usize),
    changes: Vec<(String, String, Option<&str>)>,
) {
    contract(root).unwrap();
    let body = &original[span.0..span.1];
    for (label, changed, message) in changes {
        assert_ne!(body, changed, "{label}: mutation did not change the body");
        fs::write(
            path,
            format!("{}{changed}{}", &original[..span.0], &original[span.1..]),
        )
        .unwrap();
        let error = contract(root).expect_err(&label);
        if let Some(message) = message {
            assert!(error.contains(message), "{label}: {error}");
        }
    }
    fs::write(path, original).unwrap();
}

#[test]
fn new_producer_mutations_fail_same_wiring_contract() {
    let (_temp, root) = contract_copy();
    for (workflow, name) in [
        ("rust", "postgres-build"),
        ("web", "workspace-browser-build"),
    ] {
        let path = root.join(format!(".github/workflows/{workflow}.yml"));
        let original = fs::read_to_string(&path).unwrap();
        let span = job_span(&original, name);
        let body = &original[span.0..span.1];
        let steps = blocks(body);
        let text_of = |(s, e): (usize, usize)| &body[s..e];
        let prep = *steps
            .iter()
            .find(|b| has_line(text_of(**b), "        id: sqlite"))
            .unwrap();
        let cache = *steps
            .iter()
            .find(|b| text_of(**b).lines().any(is_target_path_line))
            .unwrap();
        let helper = *steps
            .iter()
            .find(|b| text_of(**b).contains("path: crates/collab-engine/target"))
            .unwrap();
        let cache_text = text_of(cache);
        let move_cache_before = |anchor: (usize, usize)| {
            format!(
                "{}{cache_text}{}{}",
                &body[..anchor.0],
                &body[anchor.0..cache.0],
                &body[cache.1..]
            )
        };
        let mut changes: Vec<(String, String, Option<&str>)> = vec![
            ("missing producer preparation".into(), replace_once(body, "id: sqlite\n", "id: missing-sqlite\n"), None),
            ("output cache before preparation".into(), move_cache_before(prep), None),
            (
                "unqualified producer cache".into(),
                replace_once(body, cache_text, &cache_text.replacen("steps.sqlite.outputs.cache_identity", "foreign_identity", 1)),
                None,
            ),
            (
                "fallback producer cache".into(),
                replace_once(body, cache_text, &format!("{cache_text}          restore-keys: unsafe\n")),
                None,
            ),
            (
                "compile before preparation".into(),
                format!("{}      - run: cargo build --locked\n{}", &body[..prep.0], &body[prep.0..]),
                None,
            ),
            (
                "skipped producer preparation".into(),
                replace_once(body, "id: sqlite\n", "id: sqlite\n        if: false\n"),
                None,
            ),
            (
                "masked producer preparation".into(),
                replace_once(body, "id: sqlite\n", "id: sqlite\n        continue-on-error: true\n"),
                None,
            ),
            (
                "second unqualified root cache".into(),
                format!("{body}      - uses: actions/cache@fixture\n        with:\n          path: target/extra\n          key: unqualified\n"),
                None,
            ),
            (
                "helper borrows SQLite inputs".into(),
                replace_once(
                    body,
                    text_of(helper),
                    &format!("{}          sqlite: ${{{{ steps.sqlite.outputs.cache_identity }}}}\n", text_of(helper)),
                ),
                None,
            ),
        ];
        if workflow == "rust" {
            let verify = *steps
                .iter()
                .find(|b| text_of(**b).contains("name: Verify cached SQLite prefix or build\n"))
                .unwrap();
            changes.push((
                "missing producer verification".into(),
                format!("{}{}", &body[..verify.0], &body[verify.1..]),
                None,
            ));
            changes.push((
                "output cache before verification".into(),
                move_cache_before(verify),
                None,
            ));
        } else {
            let save = *steps
                .iter()
                .find(|b| {
                    text_of(**b).contains("actions/cache/save@")
                        && has_line(text_of(**b), "          path: target")
                })
                .unwrap();
            changes.push((
                "save key drifts from its restore".into(),
                replace_once(
                    body,
                    text_of(save),
                    &text_of(save).replacen(
                        "v1-web-browser-default-fixture-",
                        "v1-web-browser-drifted-",
                        1,
                    ),
                ),
                None,
            ));
        }
        assert_mutations_fail(&path, &root, &original, span, changes);
    }
}

#[test]
fn web_producer_consumer_mutations_fail_same_wiring_contract() {
    let (_temp, root) = contract_copy();
    let path = root.join(".github/workflows/web.yml");
    let original = fs::read_to_string(&path).unwrap();
    contract(&root).unwrap();
    for (name, old, new) in [
        ("collaboration-build", "id: sqlite", "id: missing-sqlite"),
        ("collaboration-install-on", "id: sqlite", "id: missing-sqlite"),
        (
            "collaboration-install-on",
            "artifact-ids: ${{ needs.collaboration-build.outputs.artifact_id }}",
            "artifact-ids: foreign",
        ),
        (
            "collaboration-install-on",
            "FVOCI_WEB_BUILD_HANDOFF_SHA256: ${{ needs.collaboration-build.outputs.handoff_sha256 }}",
            "FVOCI_WEB_BUILD_HANDOFF_SHA256: foreign",
        ),
        ("collaboration-install-on", "--ci-consume-selected", "--with-selected-backends"),
        (
            "collaboration-install-on",
            "      - name: Download this run",
            "      - uses: actions/cache@fixture\n        with:\n          path: target\n      - name: Download this run",
        ),
    ] {
        let span = job_span(&original, name);
        let body = &original[span.0..span.1];
        assert!(body.contains(old), "{name}: {old}");
        let changed = replace_once(body, old, new);
        assert_mutations_fail(&path, &root, &original, span, vec![(format!("{name}: {old}"), changed, None)]);
    }
}

/// The requested rust.yml wiring (xtask checks where test-wiring.py ran,
/// before SQLite preparation) satisfies the contract: xtask is an
/// independent workspace, like crates/. A root build in the same place
/// still fails.
#[test]
fn xtask_steps_before_preparation_are_independent_work() {
    let (_temp, root) = contract_copy();
    let path = root.join(".github/workflows/rust.yml");
    let original = fs::read_to_string(&path).unwrap();
    let old = "        run: python3 scripts/fixtures/sqlite-ci/test-wiring.py\n";
    let xtask = "        run: cargo fmt --check --manifest-path xtask/Cargo.toml\n      - run: cargo test --locked --manifest-path xtask/Cargo.toml\n";
    if original.contains(old) {
        fs::write(&path, original.replacen(old, xtask, 1)).unwrap();
        contract(&root).unwrap();
    }
    let span = job_span(&original, "fast");
    let body = &original[span.0..span.1];
    let first_step = blocks(body)[0];
    let changed = format!(
        "{}      - run: cargo test --locked --manifest-path ./Cargo.toml\n{}",
        &body[..first_step.0],
        &body[first_step.0..]
    );
    assert_mutations_fail(
        &path,
        &root,
        &original,
        span,
        vec![("root test before preparation".into(), changed, None)],
    );
}

#[test]
fn native_arm64_preparation_mutations_fail_same_wiring_contract() {
    let (_temp, root) = contract_copy();
    let path = root.join(".github/workflows/rust.yml");
    let original = fs::read_to_string(&path).unwrap();
    let span = job_span(&original, "native-arm64");
    let body = &original[span.0..span.1];
    let prep = body
        .find("      - name: Prepare pinned SQLite root build inputs")
        .unwrap();
    let cache = body
        .find("      - name: Restore server build outputs")
        .unwrap();
    let fetch = body.find("      - run: cargo fetch --locked").unwrap();
    // Challenge the exact original assertions, on copies only. Every other
    // workflow and every existing consumer remains part of that contract.
    let changes = vec![
        (
            "missing preparation".to_owned(),
            format!("{}{}", &body[..prep], &body[cache..]),
            Some("native-arm64"),
        ),
        (
            "cache before preparation".to_owned(),
            format!(
                "{}{}{}{}",
                &body[..prep],
                &body[cache..fetch],
                &body[prep..cache],
                &body[fetch..]
            ),
            Some("not less than"),
        ),
        (
            "unqualified cache".to_owned(),
            body.replace(
                "steps.sqlite.outputs.cache_identity",
                "fixture_static_identity",
            ),
            Some("steps.sqlite.outputs.cache_identity"),
        ),
    ];
    assert_mutations_fail(&path, &root, &original, span, changes);
}
