//! `xtask schema-baseline`: the schema baseline controls of rust.yml's
//! PostgreSQL A shard. On the job's isolated PostgreSQL service it creates one
//! database and one least-privilege app role, installs the schema with the
//! prepared default-feature migrator, runs the restored
//! `schema_baseline_integration` executable through `xtask rust-binaries run`
//! (2 SQLite controls, 1 prepared-owner catalog read, 1 app-role gate) and
//! checks the catalog facts it wrote.
//!
//! Inputs: no arguments; `CI`, `GITHUB_ACTIONS`, `GITHUB_SHA`,
//! `PG_CONTAINER`, `PREPARATION_DATABASE_URL` and `RUNNER_TEMP`; the working
//! directory is the checked-out workspace. Every child's raw output stays in a
//! private 0700 directory under `RUNNER_TEMP` and is never printed, because it
//! can hold URLs and seed fields. Only identities this invocation created are
//! dropped; a CREATE collision is a refusal, never an adoption.
//!
//! Exit status: 0, 1 (refused or failed), 2 (usage).

use crate::ci_fixture::{self, capture, Captured, Streams};
use crate::host::{getuid, mkdtemp, sha256_hex};
use crate::process::returncode;
use serde_json::Value;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

const USAGE: &str = "usage: xtask schema-baseline [-h]";
const HELP: &str = "\
Schema baseline SQLite controls and prepared PostgreSQL catalog (rust.yml A shard)

Reads CI, GITHUB_ACTIONS, GITHUB_SHA, PG_CONTAINER, PREPARATION_DATABASE_URL
and RUNNER_TEMP; runs from the checked-out workspace.

options:
  -h, --help  show this help message and exit
";
const MIGRATE: &str = "target/schema-default/debug/fvoci-migrate";
const TARGET: &str = "schema_baseline_integration";
const COUNT: &str = "test result: ok. 4 passed; 0 failed; 0 ignored;";
const SKIP: &str = "SKIP postgres_catalog_dump";
const LEDGER_ROWS: usize = 12;
const FALSE_ATTRIBUTES: [&str; 5] = [
    "superuser",
    "createdb",
    "createrole",
    "replication",
    "bypassrls",
];

/// These hold the superuser URL or the role password: `Debug` names the type
/// only, so no formatting of them can reach the public step log.
macro_rules! redacted_debug {
    ($($name:ident),*) => {$(
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    )*};
}
redacted_debug!(Env, Inputs, PreparationUrl, Identity);

/// The step environment; `None` is an unset variable.
#[derive(Clone, Default)]
pub struct Env {
    pub ci: Option<String>,
    pub github_actions: Option<String>,
    pub github_sha: Option<String>,
    pub pg_container: Option<String>,
    pub preparation_database_url: Option<String>,
    pub runner_temp: Option<String>,
}

/// Validated step inputs.
#[derive(PartialEq, Eq)]
pub struct Inputs {
    pub sha: String,
    pub container: String,
    pub url: PreparationUrl,
    pub runner_temp: PathBuf,
}

/// `postgres://postgres[:password]@127.0.0.1:PORT/postgres`, kept verbatim.
#[derive(PartialEq, Eq)]
pub struct PreparationUrl {
    pub text: String,
    netloc: String,
}

impl PreparationUrl {
    /// The same credentials and service with `/database` as the path.
    pub fn with_database(&self, database: &str) -> String {
        format!("postgres://{}/{database}", self.netloc)
    }
}

/// Both `CI` and `GITHUB_ACTIONS` are exactly `true`.
pub fn check_ci(env: &Env) -> Result<(), String> {
    if env.ci.as_deref() != Some("true") || env.github_actions.as_deref() != Some("true") {
        return Err("CI and GITHUB_ACTIONS must both be true".into());
    }
    Ok(())
}

/// The preparation URL must name the postgres superuser on the job's
/// loopback-published service and its maintenance database, with nothing
/// else. Unlike `urllib.parse.urlsplit` it never strips whitespace or control
/// characters, folds the scheme's case, or accepts an empty `?` or `#`.
pub fn preparation_url(text: &str) -> Result<PreparationUrl, String> {
    let refused = || {
        Err("PREPARATION_DATABASE_URL must be postgres://postgres@127.0.0.1:PORT/postgres".into())
    };
    if !text.bytes().all(|b| b.is_ascii_graphic()) || text.contains(['?', '#']) {
        return refused();
    }
    let Some((netloc, path)) = text
        .strip_prefix("postgres://")
        .and_then(|rest| rest.split_once('/'))
    else {
        return refused();
    };
    let Some((userinfo, host)) = netloc.rsplit_once('@') else {
        return refused();
    };
    let username = userinfo.split_once(':').map_or(userinfo, |(user, _)| user);
    let port = host.strip_prefix("127.0.0.1:").unwrap_or_default();
    let port_ok = !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u32>().is_ok_and(|p| (1..=65_535).contains(&p));
    if path != "postgres" || username != "postgres" || !port_ok {
        return refused();
    }
    Ok(PreparationUrl {
        text: text.to_owned(),
        netloc: netloc.to_owned(),
    })
}

fn is_lower_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Every input check that needs no filesystem or child process.
pub fn validate(env: &Env, head: &str) -> Result<Inputs, String> {
    check_ci(env)?;
    let sha = env.github_sha.clone().unwrap_or_default();
    if head.is_empty() || head != sha {
        return Err("checked-out HEAD differs from GITHUB_SHA".into());
    }
    let container = env.pg_container.clone().unwrap_or_default();
    if !is_lower_hex(&container, 64) {
        return Err("PG_CONTAINER must be a 64-hex container id".into());
    }
    let url = preparation_url(env.preparation_database_url.as_deref().unwrap_or_default())?;
    let runner_temp = PathBuf::from(env.runner_temp.clone().unwrap_or_default());
    if !runner_temp.is_absolute() {
        return Err("RUNNER_TEMP must be an absolute path".into());
    }
    Ok(Inputs {
        sha,
        container,
        url,
        runner_temp,
    })
}

/// The owned identities of one invocation.
pub struct Identity {
    pub database: String,
    pub role: String,
    password: String,
}

impl Identity {
    /// `suffix` is 16 lowercase hex characters, `password` 64.
    pub fn new(suffix: &str, password: String) -> Self {
        Self {
            database: format!("fvoci_schema_ci_{suffix}"),
            role: format!("fvoci_schema_app_{suffix}"),
            password,
        }
    }

    // CREATE has no IF NOT EXISTS: a collision is a refusal, not adoption.
    pub fn create_role(&self) -> String {
        format!(
            "CREATE ROLE \"{}\" LOGIN PASSWORD '{}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;",
            self.role, self.password
        )
    }

    pub fn create_database(&self) -> String {
        format!("CREATE DATABASE \"{}\";", self.database)
    }

    pub fn drop_database(&self) -> String {
        format!("DROP DATABASE \"{}\" WITH (FORCE);", self.database)
    }

    pub fn drop_role(&self) -> String {
        format!("DROP ROLE \"{}\";", self.role)
    }
}

/// The schema target ran exactly four controls and none skipped the catalog.
pub fn check_controls(stdout: &str, stderr: &str) -> Result<(), String> {
    if !stdout.contains(COUNT) {
        return Err("schema target must report exactly 4 passed controls".into());
    }
    if stderr.contains(SKIP) {
        return Err("schema target skipped the PostgreSQL catalog dump".into());
    }
    Ok(())
}

/// The catalog has tables and the 12-row ledger, and the app role exists,
/// can log in, holds none of the elevated attributes and belongs to no role.
pub fn check_facts(facts: &Value, role: &str) -> Result<(), String> {
    let refused = |what: &str| Err(format!("catalog facts: {what}"));
    if !facts["tables"].as_array().is_some_and(|t| !t.is_empty()) {
        return refused("tables must be a non-empty list");
    }
    if facts["ledger"].as_array().map(Vec::len) != Some(LEDGER_ROWS) {
        return refused("ledger must have exactly 12 rows");
    }
    let authority = &facts["app_role"];
    if authority["role"].as_str() != Some(role) {
        return refused("app role is not the owned role");
    }
    let attributes = &authority["attributes"];
    for key in ["exists", "login"] {
        if attributes[key] != Value::Bool(true) {
            return refused(&format!("app role attribute {key} must be true"));
        }
    }
    for key in FALSE_ATTRIBUTES {
        if attributes[key] != Value::Bool(false) {
            return refused(&format!("app role attribute {key} must be false"));
        }
    }
    if attributes["member_of"] != Value::Array(Vec::new()) {
        return refused("app role must not be a member of any role");
    }
    Ok(())
}

/// `scope.json`, byte for byte as `json.dumps` wrote it.
pub fn scope(sha: &str, catalog_sha256: &str) -> String {
    format!(
        "{{\"source\": \"{sha}\", \"catalog_sha256\": \"{catalog_sha256}\", \"actual_controls\": 4, \"sqlite_controls\": 2, \"postgres_catalog_reads\": 1, \"postgres_app_role_gate_controls\": 1, \"normal_product_role_tests\": \"separate existing PostgreSQL integration suites\"}}"
    )
}

// ---- process, filesystem and database I/O ----

fn env_text(name: &str) -> Result<Option<String>, String> {
    std::env::var_os(name)
        .map(|v| v.into_string().map_err(|_| format!("{name} is not UTF-8")))
        .transpose()
}

fn step_env() -> Result<Env, String> {
    Ok(Env {
        ci: env_text("CI")?,
        github_actions: env_text("GITHUB_ACTIONS")?,
        github_sha: env_text("GITHUB_SHA")?,
        pg_container: env_text("PG_CONTAINER")?,
        preparation_database_url: env_text("PREPARATION_DATABASE_URL")?,
        runner_temp: env_text("RUNNER_TEMP")?,
    })
}

fn head() -> Result<String, String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "git rev-parse HEAD exit={}",
            returncode(output.status)
        ));
    }
    String::from_utf8(output.stdout)
        .map(|text| text.trim().to_owned())
        .map_err(|_| "git rev-parse HEAD output is not UTF-8".into())
}

/// The prepared migrator, resolved through symlinks, must be a regular file.
fn migrator() -> Result<PathBuf, String> {
    let path = Path::new(MIGRATE)
        .canonicalize()
        .map_err(|e| format!("{MIGRATE}: {e}"))?;
    let meta = fs::symlink_metadata(&path).map_err(|e| format!("{MIGRATE}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{MIGRATE} is not a regular file"));
    }
    Ok(path)
}

/// A new private evidence directory owned by the current user.
fn evidence_root(runner_temp: &Path) -> Result<PathBuf, String> {
    let root = mkdtemp("fvoci-schema-ci-", runner_temp)
        .map_err(|e| format!("{}: {e}", runner_temp.display()))?;
    let meta = fs::symlink_metadata(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    if !meta.is_dir() || meta.permissions().mode() & 0o7777 != 0o700 || meta.uid() != getuid() {
        return Err("private evidence directory must be 0700 and owned by the caller".into());
    }
    Ok(root)
}

struct Run<'a> {
    root: &'a Path,
    container: &'a str,
}

impl Run<'_> {
    /// Run a child with its output in `<name>.stdout.log`/`<name>.stderr.log`.
    fn command(
        &self,
        name: &str,
        command: &mut Command,
        sql: Option<&str>,
        cancellable: bool,
    ) -> Result<Captured, String> {
        let done = capture(
            command,
            sql.map(str::as_bytes),
            Streams::Separate,
            cancellable,
        )
        .map_err(|e| format!("{name}: {e}"))?;
        ci_fixture::write_private(&self.root.join(format!("{name}.stdout.log")), &done.stdout)?;
        ci_fixture::write_private(&self.root.join(format!("{name}.stderr.log")), &done.stderr)?;
        if !done.status.success() {
            return Err(format!("{name} exit={}", returncode(done.status)));
        }
        Ok(done)
    }

    fn sql(&self, name: &str, text: &str, cancellable: bool) -> Result<(), String> {
        let mut command = Command::new("docker");
        command.args([
            "exec",
            "-i",
            self.container,
            "psql",
            "-X",
            "-U",
            "postgres",
            "-d",
            "postgres",
            "-v",
            "ON_ERROR_STOP=1",
        ]);
        self.command(name, &mut command, Some(text), cancellable)
            .map(drop)
    }
}

fn utf8(bytes: Vec<u8>, what: &str) -> Result<String, String> {
    String::from_utf8(bytes).map_err(|_| format!("{what} is not UTF-8"))
}

/// Which owned identities exist and must be retired.
#[derive(Default)]
struct Created {
    role: bool,
    database: bool,
}

fn controls(
    run: &Run,
    inputs: &Inputs,
    identity: &Identity,
    migrate: &Path,
    created: &mut Created,
) -> Result<(), String> {
    run.sql("create-role", &identity.create_role(), true)?;
    created.role = true;
    run.sql("create-database", &identity.create_database(), true)?;
    created.database = true;
    // The owner URL is confined to preparation and the documented read-only
    // catalog extractor, never given to product servers or other targets.
    let owner_url = inputs.url.with_database(&identity.database);
    let prepare = |command: &mut Command| {
        command
            .env("FVOCI_DATABASE_BACKEND", "postgres")
            .env("DATABASE_URL", &owner_url);
    };
    let mut install = Command::new(migrate);
    prepare(&mut install);
    run.command("normal-install", &mut install, None, true)?;
    let mut grants = Command::new(migrate);
    grants.args([
        OsString::from("--grant-app-role"),
        OsString::from(&identity.role),
    ]);
    prepare(&mut grants);
    run.command("normal-app-grants", &mut grants, None, true)?;

    let catalog = run.root.join("catalog.private.json");
    ci_fixture::write_private(&catalog, b"")?;
    let xtask = std::env::current_exe().map_err(|e| format!("xtask executable: {e}"))?;
    let directory = format!("{}/rust-binaries", inputs.runner_temp.display());
    let mut target = Command::new(xtask);
    target
        .args([
            "rust-binaries",
            "run",
            "--directory",
            &directory,
            "--test",
            TARGET,
            "--nocapture",
        ])
        .env("FVOCI_SCHEMA_CATALOG_DATABASE_URL", &owner_url)
        .env("FVOCI_SCHEMA_CATALOG_APP_ROLE", &identity.role)
        .env("FVOCI_SCHEMA_CATALOG_OUT", &catalog)
        // The app-role gate owns a separate database/role on this isolated service.
        .env("TEST_DATABASE_URL", &inputs.url.text)
        .env_remove("PREPARATION_DATABASE_URL");
    let done = run.command("schema-target", &mut target, None, true)?;
    check_controls(
        &utf8(done.stdout, "schema target stdout")?,
        &utf8(done.stderr, "schema target stderr")?,
    )?;
    let bytes = fs::read(&catalog).map_err(|e| format!("{}: {e}", catalog.display()))?;
    let facts: Value =
        serde_json::from_slice(&bytes).map_err(|e| format!("catalog facts are not JSON: {e}"))?;
    check_facts(&facts, &identity.role)?;
    ci_fixture::write_private(
        &run.root.join("scope.json"),
        scope(&inputs.sha, &sha256_hex(&bytes)).as_bytes(),
    )?;
    if ci_fixture::interrupted() {
        return Err("interrupted".into());
    }
    Ok(())
}

/// Retire exactly the identities this invocation created; both drops are
/// attempted even when the first fails.
fn retire(run: &Run, identity: &Identity, created: &Created) -> Result<(), String> {
    let database = if created.database {
        run.sql("drop-database", &identity.drop_database(), false)
    } else {
        Ok(())
    };
    let role = if created.role {
        run.sql("drop-role", &identity.drop_role(), false)
    } else {
        Ok(())
    };
    database.and(role)
}

fn execute() -> Result<(), String> {
    let env = step_env()?;
    check_ci(&env)?;
    let inputs = validate(&env, &head()?)?;
    let migrate = migrator()?;
    let root = evidence_root(&inputs.runner_temp)?;
    let identity = Identity::new(&ci_fixture::random_hex(8)?, ci_fixture::random_hex(32)?);
    ci_fixture::catch_interrupt()?;
    let run = Run {
        root: &root,
        container: &inputs.container,
    };
    let mut created = Created::default();
    let outcome = controls(&run, &inputs, &identity, &migrate, &mut created).inspect_err(|_| {
        println!("schema baseline controls failed; private evidence retained");
    });
    let retired = retire(&run, &identity, &created);
    let outcome = match (outcome, retired) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(format!("cleanup: {error}")),
        (Err(error), Err(cleanup)) => Err(format!("{error}; cleanup: {cleanup}")),
    };
    // A cancellation during cleanup still fails the step.
    let outcome = outcome.and_then(|()| {
        if ci_fixture::interrupted() {
            Err("interrupted".to_owned())
        } else {
            Ok(())
        }
    });
    match outcome {
        Ok(()) => {
            println!("schema baseline: 2 SQLite controls + 1 prepared-owner catalog read + 1 PostgreSQL app-role gate passed; owned DB/role retired");
            Ok(())
        }
        Err(error) => Err(format!(
            "schema baseline CI preparation/execution failed: {error} (evidence: {})",
            root.display()
        )),
    }
}

/// Process entry: real environment, stdout and stderr.
pub fn main(argv: Vec<OsString>) -> i32 {
    match argv.first().map(|a| a.to_str()) {
        None => {}
        Some(Some("-h" | "--help")) if argv.len() == 1 => {
            print!("{USAGE}\n\n{HELP}");
            return 0;
        }
        Some(_) => {
            eprintln!(
                "{USAGE}\nxtask schema-baseline: error: unrecognized arguments: {:?}",
                argv
            );
            return 2;
        }
    }
    match execute() {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("{message}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const URL: &str = "postgres://postgres:ci-ephemeral-only@127.0.0.1:5432/postgres";

    fn env() -> Env {
        Env {
            ci: Some("true".into()),
            github_actions: Some("true".into()),
            github_sha: Some("a".repeat(40)),
            pg_container: Some("b".repeat(64)),
            preparation_database_url: Some(URL.into()),
            runner_temp: Some("/runner/temp".into()),
        }
    }

    #[test]
    fn accepts_the_workflow_inputs() {
        let inputs = validate(&env(), &"a".repeat(40)).unwrap();
        assert_eq!(inputs.container, "b".repeat(64));
        assert_eq!(inputs.runner_temp, Path::new("/runner/temp"));
        assert_eq!(
            inputs.url.with_database("fvoci_schema_ci_0123456789abcdef"),
            "postgres://postgres:ci-ephemeral-only@127.0.0.1:5432/fvoci_schema_ci_0123456789abcdef"
        );
    }

    #[test]
    fn refuses_foreign_ci_source_container_and_temp() {
        type Mutation = fn(&mut Env);
        let cases: [(Mutation, &str); 10] = [
            (|e| e.ci = None, "CI and GITHUB_ACTIONS"),
            (|e| e.ci = Some("TRUE".into()), "CI and GITHUB_ACTIONS"),
            (
                |e| e.github_actions = Some("false".into()),
                "CI and GITHUB_ACTIONS",
            ),
            (|e| e.github_sha = None, "HEAD differs"),
            (|e| e.github_sha = Some("c".repeat(40)), "HEAD differs"),
            (|e| e.pg_container = None, "PG_CONTAINER"),
            (|e| e.pg_container = Some("B".repeat(64)), "PG_CONTAINER"),
            (|e| e.pg_container = Some("b".repeat(63)), "PG_CONTAINER"),
            (|e| e.runner_temp = None, "RUNNER_TEMP"),
            (|e| e.runner_temp = Some("relative".into()), "RUNNER_TEMP"),
        ];
        for (mutate, message) in cases {
            let mut facts = env();
            mutate(&mut facts);
            let error = validate(&facts, &"a".repeat(40)).unwrap_err();
            assert!(error.contains(message), "{error}");
        }
        assert!(validate(&env(), "").unwrap_err().contains("HEAD differs"));
    }

    #[test]
    fn url_must_be_the_loopback_maintenance_database_of_postgres() {
        for good in [
            URL,
            "postgres://postgres@127.0.0.1:1/postgres",
            "postgres://postgres:@127.0.0.1:65535/postgres",
            "postgres://postgres:p@ss:w0rd@127.0.0.1:5432/postgres",
        ] {
            let url = preparation_url(good).unwrap();
            assert_eq!(url.text, good);
        }
        for bad in [
            "",
            "postgresql://postgres:x@127.0.0.1:5432/postgres",
            "POSTGRES://postgres:x@127.0.0.1:5432/postgres",
            "postgres://postgres:x@localhost:5432/postgres",
            "postgres://postgres:x@foreign.invalid:5432/postgres",
            "postgres://postgres:x@127.0.0.2:5432/postgres",
            "postgres://postgres:x@[::1]:5432/postgres",
            "postgres://postgres:x@127.0.0.1/postgres",
            "postgres://postgres:x@127.0.0.1:/postgres",
            "postgres://postgres:x@127.0.0.1:0/postgres",
            "postgres://postgres:x@127.0.0.1:65536/postgres",
            "postgres://postgres:x@127.0.0.1:+5432/postgres",
            "postgres://postgres:x@127.0.0.1:5432/other",
            "postgres://postgres:x@127.0.0.1:5432/postgres/",
            "postgres://postgres:x@127.0.0.1:5432",
            "postgres://admin:x@127.0.0.1:5432/postgres",
            "postgres://127.0.0.1:5432/postgres",
            "postgres://postgres:x@127.0.0.1:5432/postgres?",
            "postgres://postgres:x@127.0.0.1:5432/postgres?sslmode=disable",
            "postgres://postgres:x@127.0.0.1:5432/postgres#",
            "postgres://postgres:x@127.0.0.1:5432/post\tgres",
            " postgres://postgres:x@127.0.0.1:5432/postgres",
            "postgres://postgres:x@127.0.0.1:5432/postgres\n",
            "postgres://postgres:\u{e9}@127.0.0.1:5432/postgres",
        ] {
            assert!(preparation_url(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn statements_name_only_the_owned_identities() {
        let identity = Identity::new("0123456789abcdef", "f".repeat(64));
        assert_eq!(identity.database, "fvoci_schema_ci_0123456789abcdef");
        assert_eq!(identity.role, "fvoci_schema_app_0123456789abcdef");
        assert_eq!(
            identity.create_role(),
            format!("CREATE ROLE \"fvoci_schema_app_0123456789abcdef\" LOGIN PASSWORD '{}' NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;", "f".repeat(64))
        );
        assert_eq!(
            identity.create_database(),
            "CREATE DATABASE \"fvoci_schema_ci_0123456789abcdef\";"
        );
        assert_eq!(
            identity.drop_database(),
            "DROP DATABASE \"fvoci_schema_ci_0123456789abcdef\" WITH (FORCE);"
        );
        assert_eq!(
            identity.drop_role(),
            "DROP ROLE \"fvoci_schema_app_0123456789abcdef\";"
        );
    }

    #[test]
    fn controls_need_exactly_four_and_no_catalog_skip() {
        assert!(check_controls(&format!("x\n{COUNT} finished\n"), "").is_ok());
        for count in ["3", "5", "14"] {
            let line = COUNT.replace("4 passed", &format!("{count} passed"));
            assert!(check_controls(&line, "").is_err(), "{count}");
        }
        assert!(check_controls(&COUNT.replace("0 ignored", "1 ignored"), "").is_err());
        assert!(check_controls(&COUNT.replace("ok.", "okX"), "").is_err());
        assert!(check_controls(COUNT, "SKIP postgres_catalog_dump: unset\n").is_err());
    }

    fn facts(role: &str) -> Value {
        json!({"tables": [{"name": "fixture"}], "ledger": vec![json!({}); 12],
               "app_role": {"role": role, "attributes": {"exists": true, "login": true, "member_of": [],
                   "superuser": false, "createdb": false, "createrole": false, "replication": false, "bypassrls": false}}})
    }

    #[test]
    fn facts_require_tables_full_ledger_and_least_privilege_role() {
        assert!(check_facts(&facts("r"), "r").is_ok());
        type Mutation = fn(&mut Value);
        let cases: [Mutation; 14] = [
            |f| f["tables"] = json!([]),
            |f| f["tables"] = json!({"a": 1}),
            |f| f["ledger"] = json!(vec![json!({}); 11]),
            |f| f["ledger"] = json!("twelve chars"),
            |f| f["app_role"]["role"] = json!("other"),
            |f| f["app_role"]["attributes"] = Value::Null,
            |f| f["app_role"]["attributes"]["exists"] = json!(1),
            |f| f["app_role"]["attributes"]["login"] = json!(false),
            |f| f["app_role"]["attributes"]["bypassrls"] = json!(true),
            |f| f["app_role"]["attributes"]["superuser"] = Value::Null,
            |f| {
                f["app_role"]["attributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("createdb");
            },
            |f| f["app_role"]["attributes"]["replication"] = json!(0),
            |f| f["app_role"]["attributes"]["member_of"] = json!(["pg_read_all_data"]),
            |f| f["app_role"]["attributes"]["member_of"] = Value::Null,
        ];
        for (index, mutate) in cases.iter().enumerate() {
            let mut value = facts("r");
            mutate(&mut value);
            assert!(check_facts(&value, "r").is_err(), "case {index}");
        }
    }

    #[test]
    fn scope_matches_python_json_dumps() {
        assert_eq!(
            scope(&"a".repeat(40), &"c".repeat(64)),
            format!("{{\"source\": \"{}\", \"catalog_sha256\": \"{}\", \"actual_controls\": 4, \"sqlite_controls\": 2, \"postgres_catalog_reads\": 1, \"postgres_app_role_gate_controls\": 1, \"normal_product_role_tests\": \"separate existing PostgreSQL integration suites\"}}", "a".repeat(40), "c".repeat(64))
        );
    }
}
