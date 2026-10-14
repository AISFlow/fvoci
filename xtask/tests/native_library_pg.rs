//! Ownership ordering with the production fixture source and capture's
//! completion/interruption contract. Real PostgreSQL and child ACK boundaries
//! are additionally exercised by the dispatched native fixture probes.

#[path = "../src/native_library_pg.rs"]
mod pg;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
pub use xtask::schema_baseline;

fn required_env(name: &str) -> Result<String, String> {
    std::env::var(name).map_err(|_| "missing test fixture input".into())
}

#[derive(Default)]
struct State {
    role: bool,
    database: bool,
    drops: Vec<&'static str>,
}
static STATE: Mutex<State> = Mutex::new(State {
    role: false,
    database: false,
    drops: Vec::new(),
});
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

mod ci_fixture {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    pub use xtask::ci_fixture::{random_hex, Captured, Streams};

    pub fn catch_interrupt() -> Result<(), String> {
        xtask::ci_fixture::catch_interrupt()
    }

    pub fn interrupted() -> bool {
        INTERRUPTED.load(Ordering::SeqCst)
    }

    pub fn capture(
        _command: &mut Command,
        input: Option<&[u8]>,
        _streams: Streams,
        cancellable: bool,
    ) -> Result<Captured, String> {
        if cancellable && interrupted() {
            return Err("interrupted".into());
        }
        let mode = std::env::var("NATIVE_PG_UNIT_CASE").unwrap();
        let mut state = STATE.lock().unwrap();
        let mut status = 0;
        if let Some(sql) = input {
            let creation = if sql.starts_with(b"CREATE ROLE") {
                Some(("role", &mut state.role))
            } else if sql.starts_with(b"CREATE DATABASE") {
                Some(("database", &mut state.database))
            } else {
                None
            };
            if let Some((kind, exists)) = creation {
                if *exists {
                    status = 1;
                } else {
                    *exists = true;
                    if mode == format!("interrupt-{kind}") {
                        INTERRUPTED.store(true, Ordering::SeqCst);
                    }
                    if mode == format!("unknown-{kind}") {
                        return Err("unconfirmed client result".into());
                    }
                }
            } else if sql.starts_with(b"DROP DATABASE") {
                state.drops.push("database");
                state.database = false;
            } else if sql.starts_with(b"DROP ROLE") {
                state.drops.push("role");
                if mode == "drop-failure" {
                    status = 1;
                } else {
                    state.role = false;
                }
            } else {
                panic!("unexpected SQL phase");
            }
        }
        // A cancellable capture can lose the successful CREATE result at
        // this boundary. A noncancellable capture returns the child's ACK.
        if cancellable && interrupted() {
            return Err("interrupted".into());
        }
        Ok(Captured {
            status: std::process::ExitStatus::from_raw(status << 8),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }
}

#[test]
fn ownership_completion_interruption_and_failed_create_contracts() {
    let Ok(mode) = std::env::var("NATIVE_PG_UNIT_CASE") else {
        for mode in [
            "success",
            "interrupt-role",
            "interrupt-database",
            "primary-failure",
            "drop-failure",
            "collision-role",
            "collision-database",
            "unknown-role",
            "unknown-database",
        ] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ownership_completion_interruption_and_failed_create_contracts",
                ])
                .env("NATIVE_PG_UNIT_CASE", mode)
                .env("PG_CONTAINER", "isolated-unit-fixture")
                .env(
                    "PREPARATION_DATABASE_URL",
                    "postgres://postgres@127.0.0.1:5432/postgres",
                )
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
        return;
    };
    {
        let mut state = STATE.lock().unwrap();
        state.role = mode == "collision-role";
        state.database = mode == "collision-database";
    }
    let result = pg::run(std::path::Path::new("unused-unit-migrator"), || {
        assert!(matches!(
            mode.as_str(),
            "success" | "primary-failure" | "drop-failure"
        ));
        if mode == "success" {
            Ok(())
        } else {
            Err("original failure".into())
        }
    });
    assert!(std::env::var_os("FVOCI_NATIVE_PG_APP_URL").is_none());
    let state = STATE.lock().unwrap();
    match mode.as_str() {
        "success" => assert_eq!(result, Ok(())),
        "interrupt-role" | "interrupt-database" => {
            assert!(result.as_ref().unwrap_err().contains("interrupted"));
            assert!(!state.role && !state.database, "committed CREATE leaked");
        }
        "primary-failure" => assert_eq!(result.as_ref().unwrap_err(), "original failure"),
        "drop-failure" => {
            assert_eq!(
                result.as_ref().unwrap_err(),
                "original failure; native PG app drop-role failed"
            );
            assert!(state.role && !state.database);
        }
        "collision-role" | "unknown-role" => {
            assert!(result.is_err());
            assert!(state.role && !state.database);
            assert!(state.drops.is_empty());
        }
        "collision-database" | "unknown-database" => {
            assert!(result.is_err());
            assert!(!state.role && state.database);
            assert_eq!(state.drops, ["role"]);
        }
        _ => unreachable!(),
    }
    if mode.starts_with("unknown") {
        assert!(result
            .as_ref()
            .unwrap_err()
            .contains("creation outcome unconfirmed"));
    }
    if matches!(
        mode.as_str(),
        "success" | "primary-failure" | "drop-failure"
    ) {
        assert_eq!(state.drops, ["database", "role"]);
    }
}
