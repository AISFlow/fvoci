//! The PG_APP control needs a current-schema restricted application pool.
//! Reuse schema-baseline's validated loopback provisioner and SQL identities;
//! only this invocation's random database and role are retired.

use super::required_env;
use crate::ci_fixture::{self, capture, Streams};
use crate::schema_baseline::{preparation_url, Identity};
use std::path::Path;
use std::process::Command;

fn run_command(
    command: &mut Command,
    input: Option<&str>,
    phase: &str,
    cancellable: bool,
) -> Result<(), String> {
    let done = capture(
        command,
        input.map(str::as_bytes),
        Streams::Separate,
        cancellable,
    )
    .map_err(|_| format!("native PG app {phase} child failed"))?;
    if done.status.success() {
        Ok(())
    } else {
        Err(format!("native PG app {phase} failed"))
    }
}

pub(super) fn run(
    migrator: &Path,
    tests: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let url = preparation_url(&required_env("PREPARATION_DATABASE_URL")?)?;
    let container = required_env("PG_CONTAINER")?;
    if !container
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("native PG app container identity invalid".into());
    }
    let sql = |text: &str, phase: &str, cancellable: bool| {
        run_command(
            Command::new("docker").args([
                "exec",
                "-i",
                &container,
                "psql",
                "-X",
                "-U",
                "postgres",
                "-d",
                "postgres",
                "-v",
                "ON_ERROR_STOP=1",
            ]),
            Some(text),
            phase,
            cancellable,
        )
    };
    ci_fixture::catch_interrupt()?;
    let mut created_role = false;
    let mut created_database = false;
    let password = ci_fixture::random_hex(32)?;
    let identity = Identity::new(&ci_fixture::random_hex(8)?, password.clone());
    let create = |text: &str, phase: &str, owned: &mut bool| -> Result<(), String> {
        if ci_fixture::interrupted() {
            return Err("native PG app interrupted".into());
        }
        // CREATE can commit before the docker/psql ACK reaches this process.
        // Finish and reap that child, then record ownership before acting on
        // SIGINT. A failed/unknown ACK never authorizes dropping a collision.
        sql(text, phase, false).map_err(|error| {
            format!("{error}; creation outcome unconfirmed, resource not adopted")
        })?;
        *owned = true;
        if ci_fixture::interrupted() {
            return Err("native PG app interrupted".into());
        }
        Ok(())
    };
    let original = (|| {
        create(&identity.create_role(), "create-role", &mut created_role)?;
        create(
            &identity.create_database(),
            "create-database",
            &mut created_database,
        )?;
        let owner = url.with_database(&identity.database);
        run_command(
            Command::new(migrator)
                .env("FVOCI_DATABASE_BACKEND", "postgres")
                .env("DATABASE_URL", &owner),
            None,
            "migrate",
            true,
        )?;
        run_command(
            Command::new(migrator)
                .args(["--grant-app-role", &identity.role])
                .env("FVOCI_DATABASE_BACKEND", "postgres")
                .env("DATABASE_URL", &owner),
            None,
            "grant",
            true,
        )?;
        let (_, address) = url
            .text
            .rsplit_once('@')
            .ok_or("native PG app provisioner address invalid")?;
        let address = address
            .strip_suffix("/postgres")
            .ok_or("native PG app provisioner database invalid")?;
        let app = format!(
            "postgres://{}:{password}@{address}/{}",
            identity.role, identity.database
        );
        let previous = std::env::var_os("FVOCI_NATIVE_PG_APP_URL");
        std::env::set_var("FVOCI_NATIVE_PG_APP_URL", app);
        let result = tests();
        match previous {
            Some(value) => std::env::set_var("FVOCI_NATIVE_PG_APP_URL", value),
            None => std::env::remove_var("FVOCI_NATIVE_PG_APP_URL"),
        };
        result
    })();
    let db_cleanup = if created_database {
        sql(&identity.drop_database(), "drop-database", false)
    } else {
        Ok(())
    };
    let role_cleanup = if created_role {
        sql(&identity.drop_role(), "drop-role", false)
    } else {
        Ok(())
    };
    let mut errors = Vec::new();
    for result in [original, db_cleanup, role_cleanup] {
        if let Err(error) = result {
            errors.push(error);
        }
    }
    if ci_fixture::interrupted() {
        errors.push("native PG app interrupted".into());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}
