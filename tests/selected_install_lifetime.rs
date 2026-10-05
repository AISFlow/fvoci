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
    fn run_directory(&self, label: &str) -> PathBuf {
        let run = self.root.join(format!("{label}-{}", Uuid::now_v7()));
        std::fs::create_dir(&run).unwrap();
        let directory = std::fs::File::open(&run).unwrap();
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

struct OwnedProcess {
    child: Child,
    logs: Arc<Mutex<Vec<String>>>,
    reader: Option<std::thread::JoinHandle<()>>,
    status: Option<ExitStatus>,
    report: PathBuf,
}
impl OwnedProcess {
    fn start(command: &mut Command, report: PathBuf) -> Self {
        let mut child = command.spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let read_logs = logs.clone();
        let reader = std::thread::spawn(move || {
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
        });
        Self {
            child,
            logs,
            reader: Some(reader),
            status: None,
            report,
        }
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
        std::fs::write(&self.report, json!({"pid":self.child.id(),"kind":kind,"status":self.status.map(|status| status.to_string()),"stderr":self.text(),"readerJoined":self.reader.is_none()}).to_string()).unwrap();
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
            let _ = std::fs::write(&self.report, json!({"pid":self.child.id(),"kind":"exceptional-force-reap","status":self.status.map(|status| status.to_string()),"stderr":self.text(),"readerJoined":true}).to_string());
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
            run.join(format!("{attempt}-process.json")),
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
        migrate::assert_sqlite_schema_current(&Backend::Sqlite(pool.clone()))
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM users WHERE email='install@fixture.invalid'"
            )
            .fetch_one(&pool)
            .await
            .unwrap(),
            1
        );
        pool.close().await;
        std::fs::write(run.join(format!("{attempt}-uid-readback.json")), json!({"uid":1000,"gid":1000,"inode":actual_inode,"setupNeeded":needed,"preservedSetupUserCount":1,"databaseMode":"0600","parentMode":"0700","serviceExit":0}).to_string()).unwrap();
    }
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
        let mut process = OwnedProcess::start(&mut command, run.join("signal-process.json"));
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
            OwnedProcess::start(&mut inputs.command(&db), run.join("restart-process.json"));
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
        std::fs::write(run.join("signal-gate-readback.json"), json!({"phase":phase,"signalExit":143,"heldBeyondOriginalFiveSecondShutdown":true,"originalDrain":"Closed","serverExecDuringSignal":false,"restartExit":0,"restartSetupNeeded":true}).to_string()).unwrap();
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
            OwnedProcess::start(&mut inputs.command(&db), run.join("refused-process.json"));
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
    let mut process = OwnedProcess::start(&mut command, run.join("replacement-process.json"));
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
}
