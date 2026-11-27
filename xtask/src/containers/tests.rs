use super::*;

fn probe() -> Container {
    Container {
        name: "probe-pg".to_owned(),
        host_port: 15999,
        image: "postgres:18",
        internal_port: 5432,
        environment: vec!["POSTGRES_USER=postgres".to_owned()],
        shm_bytes: None,
        command: vec!["server", "-s3"],
    }
}

#[test]
fn the_postgres_container_takes_the_connections_of_the_tests() {
    let container = Container::postgres().expect("the default container");
    let asked = format!("max_connections={POSTGRES_MAX_CONNECTIONS}");
    assert_eq!(container.command, ["-c", asked.as_str()]);
}

#[test]
fn a_container_is_published_on_the_loopback_interface_only() {
    assert_eq!(
        probe().run_arguments(),
        [
            "run",
            "--detach",
            "--name",
            "probe-pg",
            "--env",
            "POSTGRES_USER=postgres",
            "--publish",
            "127.0.0.1:15999:5432",
            "postgres:18",
            "server",
            "-s3",
        ],
        "name, environment, loopback port, image, then the command"
    );
}

#[test]
fn the_postgres_container_is_made_with_shared_memory_for_parallel_queries() {
    let container = Container::postgres().expect("the default container");
    let arguments = container.run_arguments();
    let size = arguments.iter().position(|part| part == "--shm-size");
    let image = arguments.iter().position(|part| part == container.image);
    assert_eq!(
        size.map(|at| arguments[at + 1].as_str()),
        Some("1073741824"),
        "{arguments:?}"
    );
    assert!(size < image, "an option of `docker run`: {arguments:?}");
    let store = Container::object_store().expect("the default container");
    assert!(!store.run_arguments().contains(&"--shm-size".to_owned()));
}

#[test]
fn a_container_made_with_less_shared_memory_is_named() {
    let said = smaller_shm("probe-pg", POSTGRES_SHM_BYTES, "67108864\n").expect("smaller");
    assert!(
        said.contains("probe-pg: made with 64 MB") && said.contains("gives 1024 MB"),
        "{said}"
    );
    assert_eq!(
        smaller_shm("probe-pg", POSTGRES_SHM_BYTES, "1073741824\n"),
        None
    );
    assert_eq!(
        smaller_shm("probe-pg", POSTGRES_SHM_BYTES, "no number"),
        None
    );
}

fn succeeded(output: &str) -> Captured {
    Captured {
        succeeded: true,
        output: output.to_owned(),
        error: String::new(),
    }
}

fn failed(error: &str) -> Captured {
    Captured {
        succeeded: false,
        output: String::new(),
        error: error.to_owned(),
    }
}

#[test]
fn the_inspection_decides_between_running_stopped_and_missing() {
    let state = |captured: Captured| State::from_inspect(&captured).expect("a state");
    assert_eq!(state(succeeded("true")), State::Running, "true");
    assert_eq!(state(succeeded("false")), State::Stopped, "false");
    assert_eq!(
        state(failed("Error: No such object: fvoci-test-pg")),
        State::Missing,
        "no such object"
    );
}

/// The containers the tests below stop.
static STOPPED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

#[expect(
    clippy::unnecessary_wraps,
    reason = "it stands in for the stopping, which can fail"
)]
fn note_stop(name: &str) -> Result<()> {
    STOPPED.lock().expect("lock poisoned").push(name.to_owned());
    Ok(())
}

fn stopped(prefix: &str) -> Vec<String> {
    let stopped = STOPPED.lock().expect("lock poisoned");
    stopped
        .iter()
        .filter(|name| name.starts_with(prefix))
        .cloned()
        .collect()
}

fn meeting(test: &str) -> PathBuf {
    let path = std::env::temp_dir()
        .join("fvoci-xtask-test")
        .join(format!("{test}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

/// A container a test run started is stopped by the last run that uses it, not
/// under another run; the mark goes with it.
#[test]
fn the_last_run_stops_what_a_test_run_started() {
    let place = meeting("last");
    let mut first = Uses::meeting_in(place.clone(), note_stop).expect("a meeting");
    let mut second = Uses::meeting_in(place.clone(), note_stop).expect("a meeting");
    drop(first.claim("last-pg").expect("claimed"));
    first.mark_started("last-pg").expect("marked");
    drop(second.claim("last-pg").expect("claimed"));
    let (name, lock) = first.held.pop().expect("held");
    assert_eq!(
        first.release(&name, &lock).expect("released"),
        Release::LeftToOthers
    );
    drop(lock);
    assert_eq!(
        stopped("last-"),
        Vec::<String>::new(),
        "the second run still uses it"
    );
    drop(second);
    assert_eq!(stopped("last-"), ["last-pg"]);
    assert!(!place.join("last-pg.started").exists(), "the mark is gone");
}

/// A container that ran before any test run started it runs on.
#[test]
fn what_ran_before_runs_on() {
    let mut run = Uses::meeting_in(meeting("before"), note_stop).expect("a meeting");
    drop(run.claim("before-pg").expect("claimed"));
    let (name, lock) = run.held.pop().expect("held");
    assert_eq!(
        run.release(&name, &lock).expect("released"),
        Release::LeftRunning
    );
    assert_eq!(stopped("before-"), Vec::<String>::new());
}

/// The containers are let go of the last first.
#[test]
fn the_containers_are_stopped_the_last_first() {
    let mut run = Uses::meeting_in(meeting("order"), note_stop).expect("a meeting");
    for name in ["order-pg", "order-s3"] {
        drop(run.claim(name).expect("claimed"));
        run.mark_started(name).expect("marked");
    }
    drop(run);
    assert_eq!(stopped("order-"), ["order-s3", "order-pg"]);
}

/// A daemon that cannot be reached is not a missing container: creating one would
/// fail next, with the reason lost.
#[test]
fn an_inspection_that_failed_for_another_reason_is_an_error_with_dockers_words() {
    let error = State::from_inspect(&failed(
        "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.",
    ))
    .expect_err("not a state");
    assert!(
        error.to_string().contains("Cannot connect"),
        "the reason is kept: {error}"
    );
}

#[test]
fn the_test_urls_name_the_host_port() {
    assert_eq!(
        probe().postgres_url(),
        "postgres://fvoci:fvoci-test@localhost:15999/fvoci?sslmode=disable",
        "the URL the store's test support reads, of the role that is no superuser"
    );
    assert_eq!(
        probe().postgres_superuser_url(),
        "postgres://postgres:fvoci-test@localhost:15999/fvoci?sslmode=disable",
        "and the superuser's, for the tests of a role the policies do not hold"
    );
    assert_eq!(
        probe().s3_endpoint(),
        "localhost:15999",
        "host and port, no scheme"
    );
}

/// The role is made what it must be also where an older run left it otherwise.
#[test]
fn the_test_role_is_no_superuser_and_is_held_by_row_level_security() {
    assert_eq!(
        test_role_statements(),
        "DO $$ BEGIN CREATE ROLE fvoci; \
         EXCEPTION WHEN duplicate_object THEN NULL; END $$; \
         ALTER ROLE fvoci LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB \
         NOCREATEROLE PASSWORD 'fvoci-test'; \
         GRANT CREATE ON DATABASE fvoci TO fvoci;",
        "made if missing, altered if there, and let make schemas"
    );
}

#[test]
fn a_container_of_an_older_task_lacks_the_superuser_it_is_given_now() {
    let container = Container::postgres().expect("the default container");
    let older = "POSTGRES_USER=fvoci\nPOSTGRES_PASSWORD=fvoci-test\nPOSTGRES_DB=fvoci\nPATH=/bin\n";
    assert_eq!(
        missing_environment(&container.environment, older),
        ["POSTGRES_USER=postgres"]
    );
    let current = format!("{}\nPATH=/bin", container.environment.join("\n"));
    assert_eq!(
        missing_environment(&container.environment, &current),
        Vec::<&str>::new()
    );
}

#[test]
fn the_default_containers_use_digest_pinned_images_matching_ci() {
    let postgres = Container::postgres().expect("the default container");
    let object_store = Container::object_store().expect("the default container");
    for image in [postgres.image, object_store.image] {
        let digest = image
            .split_once("@sha256:")
            .map(|(_, digest)| digest)
            .unwrap_or_default();
        assert!(
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')),
            "expected a 64-hex sha256 digest pin, got {image}"
        );
    }
    let script =
        std::fs::read_to_string(crate::workspace_root().join(".github/scripts/test-services.sh"))
            .expect("test-services.sh is readable");
    for (variable, image) in [
        ("POSTGRES_IMAGE", postgres.image),
        ("S3_IMAGE", object_store.image),
    ] {
        let expected = format!("{variable}='{image}'");
        assert!(
            script.lines().any(|line| line == expected),
            "test-services.sh does not define {expected}"
        );
    }
}

#[test]
fn the_status_code_is_read_from_the_status_line() {
    assert_eq!(
        status_code("HTTP/1.1 409 Conflict\r\nServer: x\r\n\r\n"),
        Some(409),
        "an existing bucket"
    );
    assert_eq!(status_code(""), None, "no answer");
    assert_eq!(status_code("garbage 200"), None, "no HTTP status line");
}
