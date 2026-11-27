//! What the installation must not do, looked at while it runs.

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::Value;

use super::Trial;
use crate::steps::{Captured, capture, say};

/// The services of the installation that run without a profile.
const SERVICES: [&str; 3] = ["fvoci", "postgres", "search"];

/// The one-shot services, which have ended when the checks run.
const ONE_SHOTS: [&str; 2] = ["roles", "migrate"];

/// The role the server connects as, as the database's superuser sees it: no superuser,
/// no `BYPASSRLS`, no `CREATEDB` or `CREATEROLE`, the owner of no table, while the
/// tables with row-level security have it forced; and, as itself, unable to turn it off.
///
/// # Errors
///
/// Fails if the role passes the wall or owns a table, or the database cannot be asked.
pub(super) fn database_role(trial: &Trial) -> Result<()> {
    let query = "SELECT r.rolsuper OR r.rolbypassrls OR r.rolcreatedb OR r.rolcreaterole, \
         (SELECT count(*) FROM pg_tables WHERE schemaname = 'public' AND tableowner = r.rolname), \
         (SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
           WHERE n.nspname = 'public' AND c.relrowsecurity), \
         (SELECT count(*) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
           WHERE n.nspname = 'public' AND c.relrowsecurity AND NOT c.relforcerowsecurity) \
         FROM pg_roles r WHERE r.rolname = 'fvoci'";
    let answer = trial.compose(&[
        "exec",
        "-T",
        "postgres",
        "psql",
        "--username=postgres",
        "--dbname=fvoci",
        "--no-align",
        "--tuples-only",
        "--field-separator=|",
        "--command",
        query,
    ])?;
    ensure!(answer.succeeded, "psql failed: {}", answer.error);
    let fields: Vec<&str> = answer.output.split('|').collect();
    let [past, owned, walled, unforced] = fields[..] else {
        bail!("the role fvoci is not there: {:?}", answer.output);
    };
    ensure!(
        past == "f",
        "fvoci is a superuser or may pass the wall or make roles: {}",
        answer.output
    );
    ensure!(owned == "0", "fvoci owns {owned} tables");
    let walled: u32 = walled.parse().context("the count of walled tables")?;
    ensure!(walled > 0, "no table of the schema has row-level security");
    ensure!(
        unforced == "0",
        "{unforced} walled tables do not force their policies"
    );

    // As fvoci, with its password on the standard input: the wall stays.
    let attempt = as_the_server_role(trial, "ALTER TABLE blocks NO FORCE ROW LEVEL SECURITY")?;
    ensure!(
        !attempt.succeeded && attempt.error.contains("must be owner"),
        "fvoci was not refused the change of a table: {} {}",
        attempt.output,
        attempt.error
    );
    say(&format!(
        "    fvoci: no superuser, no BYPASSRLS, owns no table, may not alter one; \
         {walled} tables walled and forced"
    ));
    Ok(())
}

/// Runs a statement in the database as the role fvoci, whose password goes to psql on
/// its standard input, on no command line.
fn as_the_server_role(trial: &Trial, statement: &str) -> Result<Captured> {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = trial
        .command(&[
            "exec",
            "-T",
            "postgres",
            "sh",
            "-c",
            "PGPASSWORD=$(cat) exec psql \"$@\"",
            "sh",
            "--host=127.0.0.1",
            "--username=fvoci",
            "--dbname=fvoci",
            "--command",
            statement,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("starting docker compose exec")?;
    child
        .stdin
        .take()
        .context("the standard input of psql")?
        .write_all(trial.role_password.as_bytes())
        .context("giving psql the password")?;
    let output = child.wait_with_output().context("waiting for psql")?;
    Ok(Captured {
        succeeded: output.status.success(),
        output: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
        error: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    })
}

/// The worker the server starts for the bodies edited together runs in the image: the
/// same executable, as the image's user, under the address space the server gives it
/// (1 GiB), ending when its input ends. The scenario writes bodies over HTTP, which
/// starts no worker; the server starts one when a body is opened over the WebSocket.
///
/// # Errors
///
/// Fails if the worker does not start or does not end cleanly.
pub(super) fn collab_worker(trial: &Trial) -> Result<()> {
    let ran = trial.compose(&[
        "exec",
        "-T",
        "fvoci",
        "sh",
        "-c",
        "ulimit -v 1048576 && exec /opt/fvoci/fvoci-server collab-worker < /dev/null",
    ])?;
    ensure!(
        ran.succeeded,
        "the worker did not run in the image: {} {}",
        ran.output,
        ran.error
    );
    say("    the worker ran and ended with its input, under 1 GiB of address space");
    Ok(())
}

/// The containers of the services, by name.
fn containers(trial: &Trial) -> Result<Vec<Value>> {
    let answer = trial.compose(&["ps", "--format", "json"])?;
    ensure!(
        answer.succeeded,
        "docker compose ps failed: {}",
        answer.error
    );
    answer
        .output
        .lines()
        .map(|line| serde_json::from_str(line).with_context(|| format!("not JSON: {line}")))
        .collect()
}

/// Only fvoci is reachable from the host, and only on the loopback interface (the
/// trial's `.env` keeps the default address).
///
/// # Errors
///
/// Fails for any other published port.
pub(super) fn published_ports(trial: &Trial) -> Result<()> {
    let mut published = Vec::new();
    for container in containers(trial)? {
        let service = container
            .get("Service")
            .and_then(Value::as_str)
            .unwrap_or("?");
        let publishers = container.get("Publishers").and_then(Value::as_array);
        for publisher in publishers.into_iter().flatten() {
            let port = publisher
                .get("PublishedPort")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            if port != 0 {
                let address = publisher.get("URL").and_then(Value::as_str).unwrap_or("?");
                published.push((service.to_owned(), address.to_owned(), port));
            }
        }
    }
    let unexpected: Vec<_> = published
        .iter()
        .filter(|(service, address, _)| service != "fvoci" || address != "127.0.0.1")
        .collect();
    ensure!(
        unexpected.is_empty(),
        "published beyond fvoci on 127.0.0.1: {unexpected:?}"
    );
    ensure!(!published.is_empty(), "fvoci's port is not published");
    say(&format!("    published: {published:?}"));
    Ok(())
}

/// Every process of every service runs as a user other than root.
///
/// # Errors
///
/// Fails for a process of user ID 0.
pub(super) fn processes(trial: &Trial) -> Result<()> {
    for service in SERVICES {
        let id = trial.compose(&["ps", "--quiet", service])?;
        ensure!(
            id.succeeded && !id.output.is_empty(),
            "{service} is not running"
        );
        let top = capture("docker", &["top", &id.output, "-o", "pid,uid,comm"])?;
        ensure!(top.succeeded, "docker top {service} failed: {}", top.error);
        // pid, uid, command
        let rows: Vec<Vec<&str>> = top
            .output
            .lines()
            .skip(1)
            .map(|line| line.split_whitespace().collect())
            .collect();
        // The health checks' probes run as the image's user, which is root for the
        // postgres image; they come and go and are no part of the service.
        let probes = ["pg_isready", "curl"];
        let root: Vec<_> = rows
            .iter()
            .filter(|row| row.get(1) == Some(&"0"))
            .filter(|row| !row.get(2).is_some_and(|command| probes.contains(command)))
            .collect();
        ensure!(root.is_empty(), "{service} runs as root: {root:?}");
        let users: Vec<&str> = rows.iter().filter_map(|row| row.get(1).copied()).collect();
        say(&format!("    {service}: user IDs {users:?}"));
    }
    Ok(())
}

/// None of the trial's secrets is in an image (its layers' commands, environment,
/// labels), in the description of a container (its environment and command line), or
/// in the logs of the services.
///
/// # Errors
///
/// Fails naming where a secret was found, never the secret.
pub(super) fn secrets(trial: &Trial) -> Result<()> {
    let mut places = Vec::new();
    for image in [&trial.images.server.local, &trial.images.search.local] {
        let history = capture(
            "docker",
            &["history", "--no-trunc", "--format", "{{.CreatedBy}}", image],
        )?;
        let inspect = capture("docker", &["image", "inspect", image])?;
        ensure!(
            history.succeeded && inspect.succeeded,
            "{image} cannot be inspected"
        );
        places.push((format!("the history of {image}"), history.output));
        places.push((format!("the description of {image}"), inspect.output));
    }
    for service in SERVICES.iter().chain(&ONE_SHOTS) {
        let id = trial.compose(&["ps", "--all", "--quiet", service])?;
        ensure!(
            id.succeeded && !id.output.is_empty(),
            "no container of {service}"
        );
        let inspect = capture("docker", &["inspect", &id.output])?;
        ensure!(inspect.succeeded, "{service} cannot be inspected");
        places.push((format!("the container of {service}"), inspect.output));
    }
    let logs = trial.compose(&["logs", "--no-color"])?;
    places.push((
        "the logs".to_owned(),
        format!("{}\n{}", logs.output, logs.error),
    ));
    for (place, text) in &places {
        if trial
            .secrets
            .iter()
            .any(|secret| text.contains(secret.as_str()))
        {
            bail!("a secret is in {place}");
        }
    }
    say(&format!("    looked in {} places", places.len()));
    Ok(())
}
