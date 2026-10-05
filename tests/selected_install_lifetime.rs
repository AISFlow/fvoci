#![cfg(feature = "db-tests")]
//! Root-allocated real `fvoci-migrate --start` process controls. Registration,
//! fresh coherent binaries/native/dist, root UID and isolated inputs are owned
//! by the coordinator. Missing inputs fail; no skip/alternate backend exists.
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::FutureExt;
use fvoci_server::db::{backend::Backend, migrate, pool};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uuid::Uuid;

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} must be allocated by root"))
}

struct Inputs {
    root: PathBuf,
    migrate: PathBuf,
    env: BTreeMap<String, String>,
}
impl Inputs {
    fn load() -> Self {
        assert_eq!(
            std::fs::metadata("/proc/self").unwrap().uid(),
            0,
            "this decisive install target requires actual root UID"
        );
        let root = PathBuf::from(required("FVOCI_SELECTED_INSTALL_RUN_ROOT"));
        let meta = std::fs::symlink_metadata(&root).unwrap();
        assert!(
            root.is_absolute()
                && meta.is_dir()
                && meta.uid() == 0
                && meta.gid() == 1000
                && meta.mode() & 0o777 == 0o710,
            "root-owned test run root must grant only service-group traversal (0710)"
        );
        let migrate = PathBuf::from(env!("CARGO_BIN_EXE_fvoci-migrate"));
        let server = PathBuf::from(env!("CARGO_BIN_EXE_fvoci-server"));
        assert_eq!(
            migrate.parent(),
            server.parent(),
            "real entrypoint execs its actual sibling server"
        );
        for (file, name) in [
            (&migrate, "FVOCI_SELECTED_INSTALL_MIGRATE_SHA256"),
            (&server, "FVOCI_SELECTED_INSTALL_SERVER_SHA256"),
        ] {
            let meta = std::fs::symlink_metadata(file).unwrap();
            assert!(meta.is_file() && meta.uid() == 0 && meta.mode() & 0o022 == 0);
            assert_eq!(
                format!("{:x}", Sha256::digest(std::fs::read(file).unwrap())),
                required(name),
                "fresh candidate executable differs from root receipt"
            );
        }
        let env_path = PathBuf::from(required("FVOCI_SELECTED_INSTALL_ENV_FILE"));
        let meta = std::fs::symlink_metadata(&env_path).unwrap();
        assert!(
            env_path.is_absolute() && meta.is_file() && meta.uid() == 0 && meta.mode() & 0o077 == 0
        );
        let env: BTreeMap<String, String> =
            serde_json::from_slice(&std::fs::read(env_path).unwrap()).unwrap();
        for key in [
            "DATABASE_URL",
            "DATABASE_APP_URL",
            "FVOCI_APP_DATABASE_URL",
            "FVOCI_MIGRATION_URL",
            "POSTGRES_PASSWORD",
            "FVOCI_APP_PASSWORD",
            "FVOCI_LIBSQL_URL",
            "FVOCI_LIBSQL_AUTH_TOKEN",
            "FVOCI_TEST_SQLITE_GATE_SOCKET",
            "FVOCI_TEST_STARTUP_TRANSFER_FINISH_CONTROL",
        ] {
            assert!(
                !env.contains_key(key),
                "isolated SQLite fixture contains conflicting input {key}"
            );
        }
        assert!(env.contains_key("FVOCI_PUBLIC_ORIGIN"));
        Self { root, migrate, env }
    }
    fn run_directory(&self, label: &str) -> OwnedRun {
        let id = Uuid::now_v7();
        let receipts = self.root.join(format!("receipts-{label}-{id}"));
        std::fs::create_dir(&receipts).unwrap();
        std::fs::set_permissions(&receipts, std::fs::Permissions::from_mode(0o700)).unwrap();
        let run = self.root.join(format!("tmp-{label}-{id}"));
        std::fs::create_dir(&run).unwrap();
        // Own the unique temporary root before the first subsequent fallible
        // setup step. Durable receipts never live in the disposable subtree.
        let mut run = OwnedRun {
            path: run,
            receipts,
            inode: None,
            finalized: false,
        };
        let meta = std::fs::symlink_metadata(&run.path).unwrap();
        run.inode = Some((meta.dev(), meta.ino()));
        let directory = std::fs::File::open(&run.path).unwrap();
        std::os::unix::fs::fchown(&directory, Some(0), Some(1000)).unwrap();
        directory
            .set_permissions(std::fs::Permissions::from_mode(0o710))
            .unwrap();
        let data = run.join("data");
        std::fs::create_dir(&data).unwrap();
        std::fs::set_permissions(&data, std::fs::Permissions::from_mode(0o700)).unwrap();
        run
    }
    fn command(&self, db: &Path) -> Command {
        let mut command = Command::new(&self.migrate);
        command
            .arg("--start")
            .env_clear()
            .envs(&self.env)
            .env("FVOCI_DATABASE_BACKEND", "sqlite")
            .env("FVOCI_SQLITE_PATH", db)
            .env("FVOCI_BIND", "127.0.0.1:0")
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }
}

struct OwnedRun {
    path: PathBuf,
    receipts: PathBuf,
    inode: Option<(u64, u64)>,
    finalized: bool,
}
impl OwnedRun {
    fn join(&self, path: impl AsRef<Path>) -> PathBuf {
        self.path.join(path)
    }
    fn report(&self, path: impl AsRef<Path>) -> PathBuf {
        self.receipts.join(path)
    }
    fn cleanup(&mut self, original: &str) -> std::io::Result<()> {
        if self.finalized {
            return Ok(());
        }
        self.finalized = true;
        let result = (|| {
            let meta = std::fs::symlink_metadata(&self.path)?;
            if !meta.is_dir() || Some((meta.dev(), meta.ino())) != self.inode {
                return Err(std::io::Error::other(
                    "owned temporary root inode changed; cleanup refused",
                ));
            }
            // Maintained std removal does not follow child symlinks. Never
            // recursively delete the supplied run root, inputs or receipts.
            std::fs::remove_dir_all(&self.path)
        })();
        let receipt = json!({
            "temporaryRoot":self.path,"originalFailure":original,
            "temporaryRootRemoved":result.is_ok(),
            "cleanupFailure":result.as_ref().err().map(ToString::to_string),
            "retainedPath":if result.is_err() { Some(&self.path) } else { None },
            "durableReceipts":self.receipts,
        });
        let written = std::fs::write(self.report("temporary-cleanup.json"), receipt.to_string());
        if let Err(error) = &written {
            eprintln!("owned fixture cleanup receipt failed: {error}; original={original}; temporary={}; cleanup={result:?}; durable={}", self.path.display(), self.receipts.display());
        }
        match (result, written) {
            (Err(cleanup), _) => Err(cleanup),
            (Ok(()), written) => written,
        }
    }
    fn finish(mut self) -> std::io::Result<()> {
        self.cleanup("none: healthy body completed")
    }
}
impl Drop for OwnedRun {
    fn drop(&mut self) {
        if !self.finalized {
            // Declared before every process/gate: lexical unwinding reaps the
            // owned child and closes gate FDs before this finalizer runs.
            let original = if std::thread::panicking() {
                "original body/setup panic; retained by test runner"
            } else {
                "early return before healthy finalizer"
            };
            if let Err(error) = self.cleanup(original) {
                eprintln!("owned fixture temporary cleanup failed: {error}; original={original}; retained={}", self.path.display());
            }
        }
    }
}

struct OwnedProcess {
    child: Child,
    logs: Arc<Mutex<Vec<String>>>,
    reader: Option<std::thread::JoinHandle<()>>,
    status: Option<ExitStatus>,
    report: PathBuf,
    reader_started: bool,
}
impl OwnedProcess {
    fn start(command: &mut Command, report: PathBuf) -> Self {
        let mut process = Self::spawn_owned(command, report);
        process.observe_stderr(None);
        process
    }
    fn spawn_owned(command: &mut Command, report: PathBuf) -> Self {
        let child = command.spawn().unwrap();
        let logs = Arc::new(Mutex::new(Vec::new()));
        Self {
            child,
            logs,
            reader: None,
            status: None,
            report,
            reader_started: false,
        }
    }
    fn observe_stderr(&mut self, forced_setup_failure: Option<&str>) {
        if let Some(original) = forced_setup_failure {
            panic!("{original}");
        }
        let stderr = self.child.stderr.take().unwrap();
        let read_logs = self.logs.clone();
        let reader = std::thread::Builder::new()
            .name("selected-install-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    match line {
                        Ok(line) => read_logs.lock().unwrap().push(line),
                        Err(error) => {
                            read_logs
                                .lock()
                                .unwrap()
                                .push(format!("owned stderr read failed: {error}"));
                            break;
                        }
                    }
                }
            })
            .expect("owned stderr reader setup");
        self.reader = Some(reader);
        self.reader_started = true;
    }
    fn text(&self) -> String {
        self.logs.lock().unwrap().join("\n")
    }
    fn signal(&self) {
        // Same maintained OS command used by existing process integration
        // fixtures; target is this owned unreaped PID, never a group/foreign PID.
        let result = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status()
            .unwrap();
        assert!(result.success());
    }
    async fn line(&mut self, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(line) = self
                .logs
                .lock()
                .unwrap()
                .iter()
                .find(|line| line.contains(needle))
                .cloned()
            {
                return line;
            }
            assert!(
                self.child.try_wait().unwrap().is_none(),
                "owned process exited before {needle}: {}",
                self.text()
            );
            assert!(
                Instant::now() < deadline,
                "owned process did not reach {needle}: {}",
                self.text()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    async fn finish(&mut self) -> ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                self.status = Some(status);
                self.reader.take().unwrap().join().unwrap();
                self.receipt("observed-exit");
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "owned process failed to exit: {}",
                self.text()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    fn receipt(&self, kind: &str) {
        std::fs::write(&self.report, json!({"pid":self.child.id(),"kind":kind,"status":self.status.map(|status| status.to_string()),"stderr":self.text(),"readerStarted":self.reader_started,"readerJoined":self.reader_started && self.reader.is_none()}).to_string()).unwrap();
    }
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        if self.status.is_none() {
            // Exceptional fixture cleanup is forceful and labelled; it cannot
            // satisfy a graceful finish assertion or hide the original panic.
            let _ = self.child.kill();
            self.status = self.child.wait().ok();
            if let Some(reader) = self.reader.take() {
                let _ = reader.join();
            }
            let _ = std::fs::write(&self.report, json!({"pid":self.child.id(),"kind":"exceptional-force-reap","status":self.status.map(|status| status.to_string()),"stderr":self.text(),"readerStarted":self.reader_started,"readerJoined":self.reader_started && self.reader.is_none()}).to_string());
        }
    }
}

async fn ready(process: &mut OwnedProcess) -> String {
    let line = process.line("fvoci-server listening on http://").await;
    line.split("fvoci-server listening on ")
        .nth(1)
        .unwrap()
        .trim()
        .to_owned()
}

#[tokio::test]
async fn selected_install_fresh_and_existing_service_uid() {
    let inputs = Inputs::load();
    let run = inputs.run_directory("uid");
    let db = run.join("data/app.sqlite");
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let mut inode = None;
    for (attempt, needed) in [("fresh", true), ("existing", false)] {
        let mut process = OwnedProcess::start(
            &mut inputs.command(&db),
            run.report(format!("{attempt}-process.json")),
        );
        let url = ready(&mut process).await;
        let status =
            std::fs::read_to_string(format!("/proc/{}/status", process.child.id())).unwrap();
        let uid = status
            .lines()
            .find(|line| line.starts_with("Uid:"))
            .unwrap();
        let gid = status
            .lines()
            .find(|line| line.starts_with("Gid:"))
            .unwrap();
        assert!(uid.split_whitespace().skip(1).all(|value| value == "1000"));
        assert!(gid.split_whitespace().skip(1).all(|value| value == "1000"));
        let meta = std::fs::symlink_metadata(&db).unwrap();
        assert_eq!(
            (meta.uid(), meta.gid(), meta.mode() & 0o777, meta.nlink()),
            (1000, 1000, 0o600, 1)
        );
        let actual_inode = (meta.dev(), meta.ino());
        if let Some(expected) = inode {
            assert_eq!(
                actual_inode, expected,
                "existing install cannot recreate its DB"
            );
        }
        inode = Some(actual_inode);
        let parent = std::fs::metadata(db.parent().unwrap()).unwrap();
        assert_eq!(
            (parent.uid(), parent.gid(), parent.mode() & 0o777),
            (1000, 1000, 0o700)
        );
        assert!(
            migrate::SqliteAdmission::installation_handoff(&db).is_err(),
            "live service's same inode refuses migration/handoff"
        );
        let response = client
            .get(format!("{url}/api/v1/setup"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["needed"],
            needed
        );
        if needed {
            let response = client.post(format!("{url}/api/v1/setup"))
                .header("Origin", inputs.env.get("FVOCI_PUBLIC_ORIGIN").unwrap())
                .json(&json!({"email":"install@fixture.invalid","password":"supersecret1","givenName":"Install","workspaceSlug":"install","workspaceName":"Install fixture"}))
                .send().await.unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        }
        process.signal();
        assert!(
            process.finish().await.success(),
            "normal service drain failed: {}",
            process.text()
        );
        let pool = pool::connect_sqlite_app(&db, 1).await.unwrap();
        let schema = migrate::assert_sqlite_schema_current(&Backend::Sqlite(pool.clone())).await;
        let count = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM users WHERE email='install@fixture.invalid'",
        )
        .fetch_one(&pool)
        .await;
        pool.close().await;
        schema.unwrap();
        assert_eq!(count.unwrap(), 1);
        std::fs::write(run.report(format!("{attempt}-uid-readback.json")), json!({"uid":1000,"gid":1000,"inode":actual_inode,"setupNeeded":needed,"preservedSetupUserCount":1,"databaseMode":"0600","parentMode":"0700","serviceExit":0}).to_string()).unwrap();
    }
    run.finish().unwrap();
}

#[tokio::test]
async fn selected_install_sigterm_waits_actual_commit_and_close() {
    let inputs = Inputs::load();
    for (phase, entered) in [("commit", b'C'), ("close", b'L')] {
        let run = inputs.run_directory(phase);
        let db = run.join("data/app.sqlite");
        let gates = run.join("gates");
        std::fs::create_dir(&gates).unwrap();
        std::fs::set_permissions(&gates, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = gates.join("gate.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let mut command = inputs.command(&db);
        command
            .env("FVOCI_TEST_SQLITE_GATE_SOCKET", &socket)
            .env("FVOCI_TEST_SQLITE_GATE_PHASE", phase);
        let mut process = OwnedProcess::start(&mut command, run.report("signal-process.json"));
        let (mut gate, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut marker = [0];
        tokio::time::timeout(Duration::from_secs(10), gate.read_exact(&mut marker))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            marker,
            [entered],
            "actual worker did not reach its named gate"
        );
        process.signal();
        process
            .line("preparation SIGTERM cancellation requested; awaiting original owner before exit")
            .await;
        // The original caller's unchanged runtime shutdown budget is five
        // seconds. Cross it with the same COMMIT/close still held, rather
        // than mistaking buffered SIGTERM or a brief scheduling delay for join.
        let since = Instant::now();
        while since.elapsed() < Duration::from_secs(6) {
            assert!(
                process.child.try_wait().unwrap().is_none(),
                "process exited before original gate release: {}",
                process.text()
            );
            assert!(migrate::SqliteAdmission::server(&db).is_err());
            assert!(!process.text().contains("fvoci-server listening"));
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        gate.write_all(b"R").await.unwrap();
        let status = process.finish().await;
        assert_eq!(status.code(), Some(143));
        assert!(
            process.text().contains("Closed"),
            "original owned cleanup/join disposition missing"
        );
        assert!(process.text().contains("no server exec"));
        assert!(!process.text().contains("fvoci-server listening"));
        let admission = migrate::SqliteAdmission::installation_handoff(&db).unwrap();
        drop(admission);
        // Restart uses actual current migration receipts and schema; a first
        // COMMIT may have completed despite cancellation. No reset is allowed.
        let mut restart =
            OwnedProcess::start(&mut inputs.command(&db), run.report("restart-process.json"));
        let url = ready(&mut restart).await;
        let response = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("{url}/api/v1/setup"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["needed"],
            true
        );
        restart.signal();
        assert!(restart.finish().await.success());
        std::fs::write(run.report("signal-gate-readback.json"), json!({"phase":phase,"signalExit":143,"heldBeyondOriginalFiveSecondShutdown":true,"originalDrain":"Closed","serverExecDuringSignal":false,"restartExit":0,"restartSetupNeeded":true}).to_string()).unwrap();
        drop(restart);
        drop(process);
        drop(gate);
        drop(listener);
        run.finish().unwrap();
    }
}

#[tokio::test]
async fn selected_install_refuses_alias_foreign_and_unrelated_paths() {
    use std::os::unix::fs::symlink;
    let inputs = Inputs::load();
    for kind in ["symlink", "hardlink", "foreign", "unrelated"] {
        let run = inputs.run_directory(kind);
        let db = run.join("data/app.sqlite");
        let retained = run.join("retained.txt");
        std::fs::write(&retained, b"owned synthetic bytes must survive refusal").unwrap();
        std::fs::set_permissions(&retained, std::fs::Permissions::from_mode(0o600)).unwrap();
        match kind {
            "symlink" => symlink(&retained, &db).unwrap(),
            "hardlink" => std::fs::hard_link(&retained, &db).unwrap(),
            "foreign" => {
                std::fs::write(&db, b"foreign fixture inode").unwrap();
                let file = std::fs::File::open(&db).unwrap();
                file.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .unwrap();
                // Only this new synthetic fixture inode receives a foreign UID;
                // it never changes an account, shared file or product permission.
                std::os::unix::fs::fchown(&file, Some(2000), Some(2000)).unwrap();
            }
            "unrelated" => std::fs::write(run.join("data/keep.txt"), b"keep").unwrap(),
            _ => unreachable!(),
        }
        let original = std::fs::symlink_metadata(&retained).unwrap();
        let foreign_before = (kind == "foreign").then(|| std::fs::metadata(&db).unwrap());
        let mut process =
            OwnedProcess::start(&mut inputs.command(&db), run.report("refused-process.json"));
        assert_eq!(process.finish().await.code(), Some(1));
        assert!(!process.text().contains("fvoci-server listening"));
        let refusal = match kind {
            "symlink" => "SQLite installation DB entry no-follow open",
            "hardlink" | "foreign" => {
                "SQLite installation refuses foreign/symlink/hardlinked/nonprivate DB entries"
            }
            "unrelated" => "SQLite installation parent contains unrelated entries",
            _ => unreachable!(),
        };
        assert!(process.text().contains(refusal), "refusal must come from the actual file admission, before a later schema/driver error: {}", process.text());
        let after = std::fs::symlink_metadata(&retained).unwrap();
        assert_eq!(
            (
                after.dev(),
                after.ino(),
                after.uid(),
                after.gid(),
                after.mode()
            ),
            (
                original.dev(),
                original.ino(),
                original.uid(),
                original.gid(),
                original.mode()
            )
        );
        assert_eq!(
            std::fs::read(&retained).unwrap(),
            b"owned synthetic bytes must survive refusal"
        );
        if let Some(before) = foreign_before {
            let after = std::fs::metadata(&db).unwrap();
            assert_eq!(
                (
                    after.dev(),
                    after.ino(),
                    after.uid(),
                    after.gid(),
                    after.mode()
                ),
                (
                    before.dev(),
                    before.ino(),
                    before.uid(),
                    before.gid(),
                    before.mode()
                )
            );
            assert_eq!(std::fs::read(&db).unwrap(), b"foreign fixture inode");
        }
        if kind == "unrelated" {
            assert!(!db.exists());
            assert_eq!(std::fs::read(run.join("data/keep.txt")).unwrap(), b"keep");
        }
        drop(process);
        run.finish().unwrap();
    }
    // Replace the literal parent only while the original real SQLite COMMIT
    // is paused. Its owned migration may settle; handoff must still refuse
    // before changing the replacement target or executing a server.
    let run = inputs.run_directory("replacement");
    let data = run.join("data");
    let db = data.join("app.sqlite");
    let gates = run.join("gates");
    std::fs::create_dir(&gates).unwrap();
    std::fs::set_permissions(&gates, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = gates.join("gate.sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let replacement = run.join("replacement");
    std::fs::create_dir(&replacement).unwrap();
    std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o700)).unwrap();
    let replacement_file = replacement.join("app.sqlite");
    std::fs::write(
        &replacement_file,
        b"replacement must not receive root ownership changes or writes",
    )
    .unwrap();
    std::fs::set_permissions(&replacement_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let before = std::fs::metadata(&replacement_file).unwrap();
    let mut command = inputs.command(&db);
    command
        .env("FVOCI_TEST_SQLITE_GATE_SOCKET", &socket)
        .env("FVOCI_TEST_SQLITE_GATE_PHASE", "commit");
    let mut process = OwnedProcess::start(&mut command, run.report("replacement-process.json"));
    let (mut gate, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let mut marker = [0];
    tokio::time::timeout(Duration::from_secs(10), gate.read_exact(&mut marker))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(marker, *b"C");
    std::fs::rename(&data, run.join("original-data")).unwrap();
    symlink(&replacement, &data).unwrap();
    gate.write_all(b"R").await.unwrap();
    assert_eq!(process.finish().await.code(), Some(1));
    assert!(!process.text().contains("fvoci-server listening"));
    assert!(process
        .text()
        .contains("SQLite installation ancestor/parent inode changed"));
    let after = std::fs::metadata(&replacement_file).unwrap();
    assert_eq!(
        (
            after.dev(),
            after.ino(),
            after.uid(),
            after.gid(),
            after.mode()
        ),
        (
            before.dev(),
            before.ino(),
            before.uid(),
            before.gid(),
            before.mode()
        )
    );
    assert_eq!(
        std::fs::read(&replacement_file).unwrap(),
        b"replacement must not receive root ownership changes or writes"
    );
    assert!(run.join("original-data/app.sqlite").is_file());
    drop(process);
    drop(gate);
    drop(listener);
    run.finish().unwrap();
}

#[tokio::test]
async fn selected_install_forced_failure_cleans_owned_child_gate_and_temporary_tree() {
    use std::os::unix::fs::symlink;
    #[derive(Debug, PartialEq, Eq)]
    struct TypedFixtureFailure {
        stage: &'static str,
        value: u64,
    }
    const NON_STRING_LABEL: &str =
        "non-string original panic after actual owned child and COMMIT gate; same payload resumed";
    let inputs = Inputs::load();
    for phase in ["setup", "body", "typed", "healthy"] {
        let mut run = inputs.run_directory(phase);
        let temporary = run.path.clone();
        let receipts = run.receipts.clone();
        let sentinel = run.report("outside-sentinel.txt");
        std::fs::write(
            &sentinel,
            b"durable outside target must survive temporary cleanup",
        )
        .unwrap();
        std::fs::set_permissions(&sentinel, std::fs::Permissions::from_mode(0o600)).unwrap();
        let sentinel_before = std::fs::metadata(&sentinel).unwrap();
        symlink(&sentinel, run.join("outside-link")).unwrap();
        let db = run.join("data/app.sqlite");
        let gates = run.join("gates");
        std::fs::create_dir(&gates).unwrap();
        std::fs::set_permissions(&gates, std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = gates.join("gate.sock");
        let mut owned_pid = None;
        let mut owned_listener = None;
        let original = format!("forced {phase} failure after actual owned child and COMMIT gate");
        // The run guard lives outside this caught future. All its child/gate
        // locals retire before the explicit cleanup receives the original
        // panic payload; unexpected failures cannot be mistaken for this control.
        let disposition = std::panic::AssertUnwindSafe(async {
            let body = std::panic::AssertUnwindSafe(async {
                let listener = tokio::net::UnixListener::bind(&socket).unwrap();
                let mut command = inputs.command(&db);
                command
                    .env("FVOCI_TEST_SQLITE_GATE_SOCKET", &socket)
                    .env("FVOCI_TEST_SQLITE_GATE_PHASE", "commit");
                let mut process = if phase == "setup" {
                    OwnedProcess::spawn_owned(&mut command, run.report("forced-process.json"))
                } else {
                    OwnedProcess::start(&mut command, run.report("forced-process.json"))
                };
                owned_pid = Some(process.child.id());
                let (mut gate, _) =
                    tokio::time::timeout(Duration::from_secs(10), listener.accept())
                        .await
                        .unwrap()
                        .unwrap();
                let mut marker = [0];
                tokio::time::timeout(Duration::from_secs(10), gate.read_exact(&mut marker))
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(marker, *b"C");
                if phase == "setup" {
                    // The actual child is already owned, even if stderr-reader
                    // setup fails before its reader thread exists.
                    process.observe_stderr(Some(&original));
                } else if phase == "body" {
                    panic!("{original}");
                } else if phase == "typed" {
                    std::panic::panic_any(TypedFixtureFailure {
                        stage: "actual child COMMIT gate",
                        value: 0x0340_9636,
                    });
                }
                gate.write_all(b"R").await.unwrap();
                let url = ready(&mut process).await;
                owned_listener = Some(
                    url.strip_prefix("http://")
                        .unwrap()
                        .parse::<std::net::SocketAddr>()
                        .unwrap(),
                );
                let response = reqwest::Client::builder()
                    .no_proxy()
                    .build()
                    .unwrap()
                    .get(format!("{url}/api/v1/setup"))
                    .send()
                    .await
                    .unwrap();
                assert_eq!(response.status(), reqwest::StatusCode::OK);
                assert_eq!(
                    response.json::<serde_json::Value>().await.unwrap()["needed"],
                    true
                );
                process.signal();
                assert!(process.finish().await.success());
                std::fs::write(
                    run.report("healthy-readback.json"),
                    json!({"actualSetupNeeded":true,"ownedServiceExit":0}).to_string(),
                )
                .unwrap();
            })
            .catch_unwind()
            .await;
            let primary = match body {
                Ok(()) => {
                    assert_eq!(phase, "healthy", "forced failure did not happen");
                    "none: healthy body completed".to_owned()
                }
                Err(payload) => {
                    let caught = payload.downcast_ref::<String>().cloned().or_else(|| {
                        payload
                            .downcast_ref::<&str>()
                            .map(|value| (*value).to_owned())
                    });
                    let Some(caught) = caught else {
                        // Keep the original Box<dyn Any> intact. Diagnostics and
                        // cleanup cannot replace its concrete type or value.
                        let written = std::fs::write(
                            run.report("original-failure.json"),
                            json!({"original":NON_STRING_LABEL,"samePayloadResumed":true})
                                .to_string(),
                        );
                        if let Err(error) = written {
                            eprintln!(
                                "{NON_STRING_LABEL}; durable failure receipt failed: {error}"
                            );
                        }
                        if let Err(error) = run.cleanup(NON_STRING_LABEL) {
                            eprintln!(
                                "{NON_STRING_LABEL}; cleanup failed: {error}; retained {}",
                                temporary.display()
                            );
                        }
                        std::panic::resume_unwind(payload);
                    };
                    if phase == "healthy" || caught != original {
                        if let Err(error) = run.cleanup(&caught) {
                            eprintln!(
                                "{caught}; cleanup failed: {error}; retained {}",
                                temporary.display()
                            );
                        }
                        std::panic::resume_unwind(payload);
                    }
                    std::fs::write(
                        run.report("original-failure.json"),
                        json!({"original":caught,"expectedControl":true}).to_string(),
                    )
                    .unwrap();
                    caught
                }
            };
            if let Err(cleanup) = run.cleanup(&primary) {
                panic!(
                    "{primary}; owned temporary cleanup failed: {cleanup}; retained {}",
                    temporary.display()
                );
            }
            primary
        })
        .catch_unwind()
        .await;
        let primary = match disposition {
            Ok(primary) => {
                assert_ne!(phase, "typed", "concrete typed panic did not happen");
                primary
            }
            Err(payload) => {
                if phase != "typed" {
                    std::panic::resume_unwind(payload);
                }
                let recovered = match payload.downcast::<TypedFixtureFailure>() {
                    Ok(recovered) => recovered,
                    Err(original) => std::panic::resume_unwind(original),
                };
                assert_eq!(
                    *recovered,
                    TypedFixtureFailure {
                        stage: "actual child COMMIT gate",
                        value: 0x0340_9636,
                    }
                );
                std::fs::write(
                    run.report("recovered-typed-failure.json"),
                    json!({"stage":recovered.stage,"value":recovered.value,"sameConcreteTypeAndValue":true}).to_string(),
                ).unwrap();
                NON_STRING_LABEL.to_owned()
            }
        };
        let pid = owned_pid.expect("actual child was spawned");
        assert!(
            !PathBuf::from(format!("/proc/{pid}")).exists(),
            "owned child remains after reap"
        );
        assert!(
            !temporary.exists(),
            "owned temporary socket/DB/foreign-link subtree remains"
        );
        assert!(!socket.exists());
        assert!(tokio::net::UnixStream::connect(&socket).await.is_err());
        assert!(
            !std::fs::read_to_string("/proc/net/unix")
                .unwrap()
                .contains(socket.to_str().unwrap()),
            "original owned Unix gate listener/stream remains in kernel after retirement"
        );
        if let Some(listener) = owned_listener {
            assert!(
                tokio::net::TcpStream::connect(listener).await.is_err(),
                "owned service listener remains after reap"
            );
        }
        let process: serde_json::Value =
            serde_json::from_slice(&std::fs::read(receipts.join("forced-process.json")).unwrap())
                .unwrap();
        assert!(process["status"].is_string());
        assert_eq!(
            process["kind"],
            if phase == "healthy" {
                "observed-exit"
            } else {
                "exceptional-force-reap"
            }
        );
        assert_eq!(process["readerStarted"], phase != "setup");
        assert_eq!(process["readerJoined"], phase != "setup");
        let cleanup: serde_json::Value = serde_json::from_slice(
            &std::fs::read(receipts.join("temporary-cleanup.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cleanup["originalFailure"], primary);
        assert_eq!(cleanup["temporaryRootRemoved"], true);
        assert!(cleanup["cleanupFailure"].is_null());
        assert!(cleanup["retainedPath"].is_null());
        let after = std::fs::metadata(&sentinel).unwrap();
        assert_eq!(
            (
                after.dev(),
                after.ino(),
                after.uid(),
                after.gid(),
                after.mode()
            ),
            (
                sentinel_before.dev(),
                sentinel_before.ino(),
                sentinel_before.uid(),
                sentinel_before.gid(),
                sentinel_before.mode()
            )
        );
        assert_eq!(
            std::fs::read(&sentinel).unwrap(),
            b"durable outside target must survive temporary cleanup"
        );
        if phase == "healthy" {
            assert!(receipts.join("healthy-readback.json").is_file());
        } else {
            assert!(receipts.join("original-failure.json").is_file());
        }
        if phase == "typed" {
            let original: serde_json::Value = serde_json::from_slice(
                &std::fs::read(receipts.join("original-failure.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(original["original"], NON_STRING_LABEL);
            assert_eq!(original["samePayloadResumed"], true);
            assert!(receipts.join("recovered-typed-failure.json").is_file());
        }
    }
}
