//! `xtask schema-baseline` as a real process: stub `docker`, `git`, migrator
//! and restored test executable (run through `xtask rust-binaries run`) show
//! which owned identities are created and retired, which child gets which
//! credential, and that raw output stays private.

use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const URL: &str = "postgres://postgres:fixture-only@127.0.0.1:5432/postgres";
const SUCCESS: &str = "schema baseline: 2 SQLite controls + 1 prepared-owner catalog read + 1 PostgreSQL app-role gate passed; owned DB/role retired\n";
const FAILED: &str = "schema baseline controls failed; private evidence retained\n";
const TEST_PATH: &str = "target/db-tests/debug/deps/schema_baseline_integration";
const WORKFLOW: &str = "jobs:\n  postgres:\n    env:\n      FVOCI_POSTGRES_MATRIX_CATALOG: |\n        [{\"tests\": \"--test first\"}]\n";

struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

const DOCKER: &str = r#"sql=$(cat)
printf '%s\n' "$*" >> "$FIXTURE_LOG/docker-args.log"
printf '%s\n' "$sql" >> "$FIXTURE_LOG/sql.log"
case "$FIXTURE_DEFECT:$sql" in
  collision:"CREATE ROLE"*) echo 'role exists' >&2; exit 7 ;;
  create-database:"CREATE DATABASE"*) exit 5 ;;
  drop-database:"DROP DATABASE"*) exit 6 ;;
esac"#;

const MIGRATE: &str = r#"printf '%s|%s|%s|%s\n' "$*" "$FVOCI_DATABASE_BACKEND" "$DATABASE_URL" "${FVOCI_SCHEMA_CATALOG_DATABASE_URL:-unset}" >> "$FIXTURE_LOG/migrate.log"
if [ "$FIXTURE_DEFECT" = interrupt ]; then kill -INT "$PPID"; exec sleep 30; fi
if [ "$FIXTURE_DEFECT" = orphan ]; then sleep 30 & kill -INT "$PPID"; exit 0; fi"#;

const TARGET: &str = r#"printf '%s|%s|%s|%s\n' "$*" "$TEST_DATABASE_URL" "${PREPARATION_DATABASE_URL:-unset}" "${DATABASE_URL:-unset}" >> "$FIXTURE_LOG/target.log"
bypass=false; count=4
[ "$FIXTURE_DEFECT" = elevated ] && bypass=true
[ "$FIXTURE_DEFECT" = count ] && count=3
ledger='{}'; i=1; while [ $i -lt 12 ]; do ledger="$ledger,{}"; i=$((i + 1)); done
printf '{"tables":[{"name":"fixture"}],"ledger":[%s],"app_role":{"role":"%s","attributes":{"exists":true,"login":true,"member_of":[],"superuser":false,"createdb":false,"createrole":false,"replication":false,"bypassrls":%s}}}' "$ledger" "$FVOCI_SCHEMA_CATALOG_APP_ROLE" "$bypass" > "$FVOCI_SCHEMA_CATALOG_OUT"
[ "$FIXTURE_DEFECT" = skip ] && echo 'SKIP postgres_catalog_dump: FVOCI_SCHEMA_CATALOG_DATABASE_URL unset' >&2
echo "test result: ok. $count passed; 0 failed; 0 ignored; 0 measured; 0 filtered out""#;

struct Fixture {
    _temp: Temp,
    root: PathBuf,
    command: Command,
}

impl Fixture {
    fn new(defect: &str) -> Self {
        let dir = xtask::host::mkdtemp("schema-baseline-", &std::env::temp_dir()).unwrap();
        let root = dir.canonicalize().unwrap();
        let temp = Temp(root.clone());
        script(&root.join("bin/docker"), DOCKER);
        script(&root.join("bin/git"), &format!("echo {SHA}"));
        script(
            &root.join("target/schema-default/debug/fvoci-migrate"),
            MIGRATE,
        );
        script(&root.join(TEST_PATH), TARGET);
        fs::create_dir_all(root.join(".github/workflows")).unwrap();
        fs::write(root.join(".github/workflows/rust.yml"), WORKFLOW).unwrap();
        fs::create_dir_all(root.join("log")).unwrap();
        let handoff = root.join("runner/rust-binaries");
        fs::create_dir_all(&handoff).unwrap();
        let digest = xtask::host::sha256_hex(&fs::read(root.join(TEST_PATH)).unwrap());
        let manifest = json!({"version": 1,
            "context": {"sha": SHA, "run_id": "4242", "run_attempt": "1", "arch": "X64", "workspace": root.to_str().unwrap()},
            "entries": {"schema_baseline_integration": {"path": TEST_PATH, "sha256": digest,
                "record": {"target": {"name": "schema_baseline_integration", "kind": ["test"]}}}}});
        fs::write(handoff.join("postgres-manifest.json"), manifest.to_string()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
        command
            .arg("schema-baseline")
            .current_dir(&root)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", root.join("bin").display()),
            )
            .env("CI", "true")
            .env("GITHUB_ACTIONS", "true")
            .env("GITHUB_SHA", SHA)
            .env("GITHUB_RUN_ID", "4242")
            .env("GITHUB_RUN_ATTEMPT", "1")
            .env("RUNNER_ARCH", "X64")
            .env("PG_CONTAINER", "b".repeat(64))
            .env("PREPARATION_DATABASE_URL", URL)
            .env("RUNNER_TEMP", root.join("runner"))
            .env("FIXTURE_LOG", root.join("log"))
            .env("FIXTURE_DEFECT", defect)
            .stdin(Stdio::null());
        Fixture {
            _temp: temp,
            root,
            command,
        }
    }

    fn run(&mut self) -> Output {
        self.command.output().unwrap()
    }

    fn log(&self, name: &str) -> Vec<String> {
        fs::read_to_string(self.root.join("log").join(name))
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    fn statements(&self) -> Vec<String> {
        self.log("sql.log")
    }

    /// The private evidence directories under RUNNER_TEMP.
    fn evidence(&self) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = fs::read_dir(self.root.join("runner"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("fvoci-schema-ci-")
            })
            .collect();
        found.sort();
        found
    }

    /// The single evidence directory, checked 0700 with only 0600 files.
    fn private_evidence(&self) -> PathBuf {
        let [root] = self.evidence().try_into().unwrap();
        let meta = fs::symlink_metadata(&root).unwrap();
        assert_eq!(meta.mode() & 0o7777, 0o700);
        for entry in fs::read_dir(&root).unwrap() {
            let meta = entry.unwrap().metadata().unwrap();
            assert!(meta.is_file());
            assert_eq!(meta.mode() & 0o7777, 0o600);
        }
        root
    }
}

fn names(statement: &str) -> &str {
    statement.split('"').nth(1).unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[test]
fn success_retires_only_owned_database_and_role() {
    let mut fixture = Fixture::new("");
    let output = fixture.run();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), SUCCESS);
    let sql = fixture.statements();
    assert_eq!(sql.len(), 4, "{sql:?}");
    assert!(sql[0].starts_with("CREATE ROLE \"fvoci_schema_app_"));
    assert!(sql[0].ends_with("NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;"));
    assert!(sql[1].starts_with("CREATE DATABASE \"fvoci_schema_ci_"));
    assert!(sql[2].starts_with("DROP DATABASE \"") && sql[2].ends_with("\" WITH (FORCE);"));
    assert!(sql[3].starts_with("DROP ROLE \""));
    let (role, database) = (names(&sql[0]), names(&sql[1]));
    assert_eq!(names(&sql[3]), role);
    assert_eq!(names(&sql[2]), database);
    assert_eq!(
        role["fvoci_schema_app_".len()..],
        database["fvoci_schema_ci_".len()..]
    );
    let suffix = &role["fvoci_schema_app_".len()..];
    assert!(suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()));
    let password = sql[0].split('\'').nth(1).unwrap();
    assert!(password.len() == 64 && password.bytes().all(|b| b.is_ascii_hexdigit()));
    for args in fixture.log("docker-args.log") {
        assert_eq!(
            args,
            format!(
                "exec -i {} psql -X -U postgres -d postgres -v ON_ERROR_STOP=1",
                "b".repeat(64)
            )
        );
    }

    let owner = format!("postgres://postgres:fixture-only@127.0.0.1:5432/{database}");
    assert_eq!(
        fixture.log("migrate.log"),
        [
            format!("|postgres|{owner}|unset"),
            format!("--grant-app-role {role}|postgres|{owner}|unset"),
        ]
    );
    // The catalog observer gets the owner URL only through its own variable,
    // and the app-role gate the preparation URL as TEST_DATABASE_URL.
    assert_eq!(
        fixture.log("target.log"),
        [format!("--nocapture|{URL}|unset|unset")]
    );

    let evidence = fixture.private_evidence();
    let mut files: Vec<String> = fs::read_dir(&evidence)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    files.sort();
    let mut expected: Vec<String> = [
        "create-role",
        "create-database",
        "normal-install",
        "normal-app-grants",
        "schema-target",
        "drop-database",
        "drop-role",
    ]
    .iter()
    .flat_map(|n| [format!("{n}.stdout.log"), format!("{n}.stderr.log")])
    .chain(["catalog.private.json".to_owned(), "scope.json".to_owned()])
    .collect();
    expected.sort();
    assert_eq!(files, expected);
    let catalog = fs::read(evidence.join("catalog.private.json")).unwrap();
    let scope: Value =
        serde_json::from_slice(&fs::read(evidence.join("scope.json")).unwrap()).unwrap();
    assert_eq!(
        scope,
        json!({"source": SHA, "catalog_sha256": xtask::host::sha256_hex(&catalog), "actual_controls": 4,
               "sqlite_controls": 2, "postgres_catalog_reads": 1, "postgres_app_role_gate_controls": 1,
               "normal_product_role_tests": "separate existing PostgreSQL integration suites"})
    );
    // Raw child output never reaches the step log.
    assert!(!stderr(&output).contains("fixture-only"));
    assert!(!stdout(&output).contains(password));
}

#[test]
fn count_skip_or_elevated_role_keep_private_evidence_and_retire_owned() {
    for defect in ["count", "skip", "elevated"] {
        let mut fixture = Fixture::new(defect);
        let output = fixture.run();
        assert_eq!(output.status.code(), Some(1), "{defect}");
        assert_eq!(stdout(&output), FAILED, "{defect}");
        assert!(stderr(&output).contains("schema baseline CI preparation/execution failed"));
        let sql = fixture.statements();
        assert_eq!(sql.len(), 4, "{defect}: {sql:?}");
        assert!(sql[2].starts_with("DROP DATABASE") && sql[3].starts_with("DROP ROLE"));
        let evidence = fixture.private_evidence();
        assert!(!evidence.join("scope.json").exists(), "{defect}");
        assert!(!stderr(&output).contains("fixture-only"));
    }
}

#[test]
fn foreign_inputs_refuse_before_any_identity_or_evidence() {
    type Setup = fn(&mut Command, &Path);
    let cases: [(Setup, &str); 9] = [
        (
            |c, _| {
                c.env(
                    "PREPARATION_DATABASE_URL",
                    "postgres://postgres:fixture-only@foreign.invalid:5432/postgres",
                );
            },
            "PREPARATION_DATABASE_URL",
        ),
        (
            |c, _| {
                c.env("PREPARATION_DATABASE_URL", format!("{URL}?sslmode=disable"));
            },
            "PREPARATION_DATABASE_URL",
        ),
        (
            |c, _| {
                c.env_remove("PREPARATION_DATABASE_URL");
            },
            "PREPARATION_DATABASE_URL",
        ),
        (
            |c, _| {
                c.env("PG_CONTAINER", "postgres");
            },
            "PG_CONTAINER",
        ),
        (
            |c, _| {
                c.env("CI", "false");
            },
            "CI and GITHUB_ACTIONS",
        ),
        (
            |c, _| {
                c.env_remove("GITHUB_ACTIONS");
            },
            "CI and GITHUB_ACTIONS",
        ),
        (
            |c, _| {
                c.env("GITHUB_SHA", "c".repeat(40));
            },
            "HEAD differs",
        ),
        (
            |_, root| {
                fs::remove_file(root.join("target/schema-default/debug/fvoci-migrate")).unwrap()
            },
            "fvoci-migrate",
        ),
        (
            |_, root| script(&root.join("bin/git"), "exit 128"),
            "git rev-parse HEAD exit=128",
        ),
    ];
    for (setup, message) in cases {
        let mut fixture = Fixture::new("");
        setup(&mut fixture.command, &fixture.root.clone());
        let output = fixture.run();
        assert_eq!(output.status.code(), Some(1), "{message}");
        assert!(
            stderr(&output).contains(message),
            "{message}: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output), "");
        assert!(fixture.statements().is_empty(), "{message}");
        assert!(fixture.evidence().is_empty(), "{message}");
    }
}

#[test]
fn migrator_symlink_resolves_to_a_regular_file() {
    let mut fixture = Fixture::new("");
    let link = fixture
        .root
        .join("target/schema-default/debug/fvoci-migrate");
    let real = fixture
        .root
        .join("target/schema-default/debug/real-migrate");
    fs::rename(&link, &real).unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert_eq!(fixture.run().status.code(), Some(0));
    let mut fixture = Fixture::new("");
    let link = fixture
        .root
        .join("target/schema-default/debug/fvoci-migrate");
    fs::remove_file(&link).unwrap();
    fs::create_dir(&link).unwrap();
    let output = fixture.run();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("not a regular file"));
    assert!(fixture.statements().is_empty());
}

#[test]
fn collision_never_adopts_or_drops() {
    let mut fixture = Fixture::new("collision");
    let output = fixture.run();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("create-role exit=7"));
    let sql = fixture.statements();
    assert_eq!(sql.len(), 1);
    assert!(sql[0].starts_with("CREATE ROLE"));
    let evidence = fixture.private_evidence();
    assert_eq!(
        fs::read(evidence.join("create-role.stderr.log")).unwrap(),
        b"role exists\n"
    );
}

#[test]
fn database_collision_retires_only_the_created_role() {
    let mut fixture = Fixture::new("create-database");
    let output = fixture.run();
    assert_eq!(output.status.code(), Some(1));
    let sql = fixture.statements();
    assert_eq!(sql.len(), 3, "{sql:?}");
    assert!(sql[1].starts_with("CREATE DATABASE") && sql[2].starts_with("DROP ROLE"));
    assert_eq!(names(&sql[0]), names(&sql[2]));
}

#[test]
fn failed_database_drop_still_retires_the_role_and_fails() {
    let mut fixture = Fixture::new("drop-database");
    let output = fixture.run();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert!(stderr(&output).contains("cleanup: drop-database exit=6"));
    let sql = fixture.statements();
    assert_eq!(sql.len(), 4);
    assert!(sql[3].starts_with("DROP ROLE"));
}

#[test]
fn sigint_kills_the_running_child_and_retires_owned_identities() {
    // `interrupt`: the migrator itself would sleep 30 s and is killed.
    // `orphan`: the migrator exits but a grandchild holds its output pipes
    // for 30 s; the wait for them ends on the interrupt too.
    for defect in ["interrupt", "orphan"] {
        let mut fixture = Fixture::new(defect);
        let started = Instant::now();
        let output = fixture.run();
        assert!(started.elapsed() < Duration::from_secs(20), "{defect}");
        assert_eq!(output.status.code(), Some(1), "{defect}");
        assert_eq!(stdout(&output), FAILED, "{defect}");
        assert!(
            stderr(&output).contains("normal-install: interrupted"),
            "{defect}: {}",
            stderr(&output)
        );
        // No further command starts after the interrupt.
        assert_eq!(fixture.log("migrate.log").len(), 1, "{defect}");
        let sql = fixture.statements();
        assert_eq!(sql.len(), 4, "{defect}: {sql:?}");
        assert!(sql[2].starts_with("DROP DATABASE") && sql[3].starts_with("DROP ROLE"));
    }
}

#[test]
fn arguments_are_usage_errors() {
    let mut fixture = Fixture::new("");
    let output = fixture.command.arg("--unknown").output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(fixture.statements().is_empty());
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["schema-baseline", "--help"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output).starts_with("usage: xtask schema-baseline"));
}
