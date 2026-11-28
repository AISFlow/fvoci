//! The two containers the tests use: PostgreSQL and an S3-compatible object store.
//!
//! A container that runs is used as it is; a stopped one is started and a missing one
//! created. The test runs of every checkout on this machine share the containers: a
//! container that a test run started is stopped by the last run that uses it, when it
//! ends, whether it passed, failed or was interrupted, and one that ran before any test
//! run started it is left running ([`Uses`]). Nothing is ever removed here: the
//! containers outlive the test runs.

use std::fs::{File, TryLockError};
use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};

use crate::steps::{Captured, capture, say};

/// How long a container may take to accept requests after it was started.
const READY_TIMEOUT: Duration = Duration::from_secs(90);

/// How long one connection attempt of a readiness probe may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// The pause between two readiness probes.
const PROBE_INTERVAL: Duration = Duration::from_millis(500);

/// Test-only credentials of the PostgreSQL container. This user is the image's own
/// superuser, as a database's administrator is: it prepares the database, and the tests
/// connect as it only to show what the server makes of such a role, and to own tables
/// that the role of the tests uses.
const POSTGRES_USER: &str = "postgres";
const POSTGRES_PASSWORD: &str = "fvoci-test";
const POSTGRES_DATABASE: &str = "fvoci";

/// The role the tests connect as: no superuser and not exempt from row-level security,
/// so that the policies of the schema hold for it as they do for a server in use. A
/// superuser passes every policy, and a test run as one would show nothing of them.
const POSTGRES_TEST_ROLE: &str = "fvoci";

/// The connections the PostgreSQL container takes. The tests run side by side, a test
/// server's pool and the connection that holds its server's lock each; the image's 100
/// are too few for that on a machine with many processors.
const POSTGRES_MAX_CONNECTIONS: u32 = 300;

/// The shared memory (`/dev/shm`) of the PostgreSQL container. The processes of one
/// query that runs in parallel share their memory there; Docker gives a container
/// 64 MB, which the tests of a machine with many processors fill ("could not resize
/// shared memory segment").
const POSTGRES_SHM_BYTES: u64 = 1024 * 1024 * 1024;

/// The PostgreSQL image, pinned to a multi-architecture manifest index (`linux/amd64`,
/// `linux/arm64/v8`). Change together with `.github/scripts/test-services.sh`.
const POSTGRES_IMAGE: &str =
    "postgres:18@sha256:64c8b0d31e4ab6ea665e8426ea406a94bf52de3c05b3a4a8bf713704eefa9328";

/// The S3-compatible store image, pinned to a multi-architecture manifest index
/// (`linux/amd64`, `linux/arm64`). Change together with `.github/scripts/test-services.sh`.
const OBJECT_STORE_IMAGE: &str = "chrislusf/seaweedfs:4.55@sha256:7ca46aeae2f223b37e5d91593c17ab37738eec6df6ab69b48a0487f7643342b9";

/// The bucket the S3 storage test writes into.
const S3_BUCKET: &str = "fvoci-test";

/// A container this checkout's tests use. The name and the host port can be changed
/// through the environment, so that two checkouts on one machine do not collide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Container {
    name: String,
    host_port: u16,
    image: &'static str,
    internal_port: u16,
    environment: Vec<String>,
    /// The size of its `/dev/shm`, if not Docker's.
    shm_bytes: Option<u64>,
    command: Vec<&'static str>,
}

impl Container {
    /// The PostgreSQL container: `fvoci-test-postgres` on 127.0.0.1:15434 unless
    /// `FVOCI_TEST_PG_CONTAINER` and `FVOCI_TEST_PG_PORT` say otherwise.
    ///
    /// # Errors
    ///
    /// Fails if the port variable is set to something that is no port.
    pub(crate) fn postgres() -> Result<Self> {
        Ok(Self {
            // Not the `fvoci-test-pg` on 15433 of the tasks before the superuser was
            // postgres: worktrees of either may run their tests side by side.
            name: name_from_env("FVOCI_TEST_PG_CONTAINER", "fvoci-test-postgres"),
            host_port: port_from_env("FVOCI_TEST_PG_PORT", 15434)?,
            image: POSTGRES_IMAGE,
            internal_port: 5432,
            environment: vec![
                format!("POSTGRES_USER={POSTGRES_USER}"),
                format!("POSTGRES_PASSWORD={POSTGRES_PASSWORD}"),
                format!("POSTGRES_DB={POSTGRES_DATABASE}"),
            ],
            shm_bytes: Some(POSTGRES_SHM_BYTES),
            command: vec!["-c", "max_connections=300"],
        })
    }

    /// The S3-compatible store: `fvoci-test-s3` on 127.0.0.1:18333 unless
    /// `FVOCI_TEST_S3_CONTAINER` and `FVOCI_TEST_S3_PORT` say otherwise.
    ///
    /// # Errors
    ///
    /// Fails if the port variable is set to something that is no port.
    pub(crate) fn object_store() -> Result<Self> {
        Ok(Self {
            name: name_from_env("FVOCI_TEST_S3_CONTAINER", "fvoci-test-s3"),
            host_port: port_from_env("FVOCI_TEST_S3_PORT", 18333)?,
            image: OBJECT_STORE_IMAGE,
            internal_port: 8333,
            environment: Vec::new(),
            shm_bytes: None,
            command: vec!["server", "-s3", "-s3.port=8333"],
        })
    }

    /// The PostgreSQL URL the tests connect with, as [`POSTGRES_TEST_ROLE`].
    pub(crate) fn postgres_url(&self) -> String {
        format!(
            "postgres://{POSTGRES_TEST_ROLE}:{POSTGRES_PASSWORD}@localhost:{}/{POSTGRES_DATABASE}?sslmode=disable",
            self.host_port
        )
    }

    /// The PostgreSQL URL of the container's superuser, for the tests of what the
    /// server makes of a role the policies do not hold.
    pub(crate) fn postgres_superuser_url(&self) -> String {
        format!(
            "postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}@localhost:{}/{POSTGRES_DATABASE}?sslmode=disable",
            self.host_port
        )
    }

    /// The `host:port` the S3 storage test connects to.
    pub(crate) fn s3_endpoint(&self) -> String {
        format!("localhost:{}", self.host_port)
    }

    /// The arguments of the `docker run` that creates this container, published on the
    /// loopback interface only.
    fn run_arguments(&self) -> Vec<String> {
        let mut arguments = vec![
            "run".to_owned(),
            "--detach".to_owned(),
            "--name".to_owned(),
            self.name.clone(),
        ];
        for variable in &self.environment {
            arguments.push("--env".to_owned());
            arguments.push(variable.clone());
        }
        if let Some(bytes) = self.shm_bytes {
            arguments.push("--shm-size".to_owned());
            arguments.push(bytes.to_string());
        }
        arguments.push("--publish".to_owned());
        arguments.push(format!(
            "127.0.0.1:{}:{}",
            self.host_port, self.internal_port
        ));
        arguments.push(self.image.to_owned());
        arguments.extend(self.command.iter().map(|part| (*part).to_owned()));
        arguments
    }

    /// Makes sure the container runs: starts it if it is stopped, creates it if it does
    /// not exist. The run uses it from then on (`uses`); one a test run started is
    /// marked, to be stopped by the last run that uses it.
    fn ensure_running(&self, uses: &mut Uses) -> Result<()> {
        // Held until the container runs: no other run starts or stops it meanwhile.
        let _setup = uses.claim(&self.name)?;
        let state = self.state()?;
        if state != State::Missing {
            self.check_environment()?;
            self.say_if_shm_is_smaller();
        }
        if state != State::Running {
            // Marked before it is started: a start that fails half-way leaves it to stop.
            uses.mark_started(&self.name)?;
        }
        match state {
            State::Running => say(&format!(
                "    {}: already running; used as it is",
                self.name
            )),
            State::Stopped => {
                say(&format!("    {}: stopped; starting it", self.name));
                docker(&["start", &self.name])?;
            }
            State::Missing => {
                say(&format!(
                    "    {}: not found; creating it from {}",
                    self.name, self.image
                ));
                let arguments = self.run_arguments();
                let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
                // A `docker run` that fails after creating the container (a port that
                // is taken, say) leaves it behind in the state "created".
                docker(&arguments).with_context(|| {
                    format!(
                        "creating {name}; if a container of that name was left behind, \
                         look at it with `docker ps --all` and remove it with `docker rm {name}`",
                        name = self.name
                    )
                })?;
            }
        }
        self.check_published_port()
    }

    /// Checks that a container of this name was made with the environment this task
    /// gives it. A PostgreSQL container made by an older task has another superuser
    /// (`fvoci`, now the name of the tests' role) and would fail the tests' setup in a
    /// way that does not say why.
    fn check_environment(&self) -> Result<()> {
        let inspected = capture(
            "docker",
            &[
                "inspect",
                "--format",
                "{{range .Config.Env}}{{println .}}{{end}}",
                &self.name,
            ],
        )?;
        if !inspected.succeeded {
            bail!("`docker inspect {}` failed: {}", self.name, inspected.error);
        }
        let missing = missing_environment(&self.environment, &inspected.output);
        if missing.is_empty() {
            return Ok(());
        }
        let older = if missing
            .iter()
            .any(|entry| entry.starts_with("POSTGRES_USER="))
        {
            " (one made by an older task had the superuser fvoci, now the tests' role)"
        } else {
            ""
        };
        bail!(
            "the container {name} was not made by this task as it is now: it lacks {missing:?}{older}. \
             Remove it with `docker rm --force {name}` (its test data goes with it) and run \
             again, or name another container and port with the environment",
            name = self.name
        )
    }

    /// Says so if a container of this name has less shared memory than this task gives
    /// one it makes: it was made before, and is used as it is, since removing it is
    /// not this task's to do. A size that cannot be read is no reason to stop.
    fn say_if_shm_is_smaller(&self) {
        let Some(wanted) = self.shm_bytes else {
            return;
        };
        let format = "{{.HostConfig.ShmSize}}";
        let Ok(inspected) = capture("docker", &["inspect", "--format", format, &self.name]) else {
            return;
        };
        if let Some(notice) = smaller_shm(&self.name, wanted, &inspected.output) {
            say(&notice);
        }
    }

    /// Checks that the container publishes its port where the tests will connect. A
    /// container of the same name that was made with another mapping would pass every
    /// probe inside it while the tests talk to whatever listens on the expected port.
    fn check_published_port(&self) -> Result<()> {
        let internal = format!("{}/tcp", self.internal_port);
        let published = capture("docker", &["port", &self.name, &internal])?;
        if !published.succeeded {
            bail!(
                "`docker port {} {internal}` failed: {}",
                self.name,
                published.error
            );
        }
        let wanted = format!(":{}", self.host_port);
        if published
            .output
            .lines()
            .any(|line| line.trim().ends_with(&wanted))
        {
            return Ok(());
        }
        bail!(
            "the container {} publishes port {internal} at {:?}, not at port {}; it is not \
             the one this task makes. Name another container or port with the environment, \
             or remove it",
            self.name,
            published.output,
            self.host_port
        )
    }

    fn state(&self) -> Result<State> {
        let inspected = capture(
            "docker",
            &["inspect", "--format", "{{.State.Running}}", &self.name],
        )
        .context("docker is needed to prepare the test containers")?;
        State::from_inspect(&inspected)
            .with_context(|| format!("`docker inspect {}` failed", self.name))
    }
}

/// The containers this run uses, and where the runs of all checkouts on this machine
/// meet to share them: per container, a lock that every run using it holds shared, a
/// lock held while a run starts or stops it, and a mark saying a test run started it.
///
/// Dropped at the end of the run (after it passed or failed, after a panic, and after
/// Ctrl-C, `steps::catch_interrupts`), it lets go of each container, the last first. A
/// run that is the last to use a marked container stops it and takes the mark away; one
/// that another run still uses is left to that run; one without the mark ran before any
/// test run started it and is left running.
#[derive(Debug)]
pub(crate) struct Uses {
    /// Where the locks and the marks are.
    meeting: PathBuf,
    /// The containers in use, with the shared lock this run holds on each.
    held: Vec<(String, File)>,
    /// Stops a container: `docker stop`, or what a test puts in its place.
    stop: fn(&str) -> Result<()>,
}

/// What a run did with a container it let go of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Release {
    /// It was the last to use the container that a test run had started, and stopped it.
    Stopped,
    /// Another run still uses it, and stops it if it is the last.
    LeftToOthers,
    /// No test run started it: it ran before, and runs on.
    LeftRunning,
}

impl Uses {
    /// The uses of a run, meeting the other runs in a folder of the system's temporary
    /// folder, which the checkouts on this machine share.
    ///
    /// # Errors
    ///
    /// Fails if the folder cannot be made.
    pub(crate) fn new() -> Result<Self> {
        Self::meeting_in(std::env::temp_dir().join("fvoci-xtask-containers"), stop)
    }

    fn meeting_in(meeting: PathBuf, stop: fn(&str) -> Result<()>) -> Result<Self> {
        std::fs::create_dir_all(&meeting)
            .with_context(|| format!("making {}", meeting.display()))?;
        Ok(Self {
            meeting,
            held: Vec::new(),
            stop,
        })
    }

    fn file(&self, name: &str, kind: &str) -> PathBuf {
        self.meeting.join(format!("{name}.{kind}"))
    }

    /// Starts using the container `name`, and returns the lock of its starting and
    /// stopping, held until it is dropped.
    fn claim(&mut self, name: &str) -> Result<File> {
        let setup = self.lock_setup(name)?;
        let used = open_lock(&self.file(name, "use"))?;
        // Never long: a run takes this exclusively only while it holds `setup`.
        used.lock_shared()
            .with_context(|| format!("taking the use of {name}"))?;
        self.held.push((name.to_owned(), used));
        Ok(setup)
    }

    /// Waits for the lock of the starting and stopping of the container `name`, held
    /// until it is dropped: for what else must not be done to it by two runs at once.
    fn lock_setup(&self, name: &str) -> Result<File> {
        let setup = open_lock(&self.file(name, "setup"))?;
        setup
            .lock()
            .with_context(|| format!("waiting for other runs to set up {name}"))?;
        Ok(setup)
    }

    /// Marks the container `name` as started by a test run.
    fn mark_started(&self, name: &str) -> Result<()> {
        let mark = self.file(name, "started");
        File::create(&mark).with_context(|| format!("marking {}", mark.display()))?;
        Ok(())
    }

    /// Lets go of the container `name`, whose shared lock `used` is, and stops it if
    /// this run is the last to use it and a test run started it.
    fn release(&self, name: &str, used: &File) -> Result<Release> {
        let setup = open_lock(&self.file(name, "setup"))?;
        setup.lock()?;
        // Turned into the exclusive lock if no other run holds it shared. Nobody takes
        // it meanwhile: a run takes it only while it holds `setup`.
        match used.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Ok(Release::LeftToOthers),
            Err(TryLockError::Error(error)) => return Err(error.into()),
        }
        let mark = self.file(name, "started");
        if !mark.exists() {
            return Ok(Release::LeftRunning);
        }
        (self.stop)(name)?;
        std::fs::remove_file(&mark).with_context(|| format!("removing {}", mark.display()))?;
        Ok(Release::Stopped)
    }
}

impl Drop for Uses {
    /// Lets go of the containers. Waiting for `docker stop` is what this task does at
    /// its end anyway; a container that cannot be stopped is reported.
    fn drop(&mut self) {
        let held = std::mem::take(&mut self.held);
        for (name, used) in held.into_iter().rev() {
            match self.release(&name, &used) {
                Ok(Release::Stopped) => say(&format!(
                    "    {name}: stopped, as a test run started it and this run used it last"
                )),
                Ok(Release::LeftToOthers) => say(&format!(
                    "    {name}: another run still uses it; the last one stops it"
                )),
                Ok(Release::LeftRunning) => {
                    say(&format!("    {name}: it ran before; left running"));
                }
                Err(error) => say(&format!("    {name}: could not be stopped: {error:#}")),
            }
        }
    }
}

fn open_lock(path: &Path) -> Result<File> {
    File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))
}

/// Stops the container `name`; one that is not there is as good as stopped. In a group
/// of its own, so that a Ctrl-C at the end of the run does not cut the stop off.
fn stop(name: &str) -> Result<()> {
    let mut command = Command::new("docker");
    command.args(["stop", name]);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    let output = command
        .output()
        .with_context(|| format!("starting `docker stop {name}`"))?;
    let error = String::from_utf8_lossy(&output.stderr);
    if output.status.success() || error.to_lowercase().contains("no such") {
        Ok(())
    } else {
        bail!("`docker stop {name}` failed: {}", error.trim())
    }
}

/// What `docker inspect` says about a container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Running,
    Stopped,
    Missing,
}

impl State {
    /// Reads the result of `docker inspect --format {{.State.Running}}`.
    ///
    /// # Errors
    ///
    /// Fails with docker's own words if the inspection failed for another reason than
    /// that there is no such container: a daemon that is down or may not be reached is
    /// not a missing container.
    fn from_inspect(inspected: &Captured) -> Result<Self> {
        if inspected.succeeded {
            return Ok(if inspected.output == "true" {
                Self::Running
            } else {
                Self::Stopped
            });
        }
        if inspected.error.to_lowercase().contains("no such") {
            return Ok(Self::Missing);
        }
        bail!("{}", inspected.error)
    }
}

/// The entries of `wanted` that a container's environment (`given`, one `NAME=value` a
/// line, as `docker inspect` lists it) does not have.
fn missing_environment<'a>(wanted: &'a [String], given: &str) -> Vec<&'a str> {
    let given: Vec<&str> = given.lines().map(str::trim).collect();
    wanted
        .iter()
        .map(String::as_str)
        .filter(|entry| !given.contains(entry))
        .collect()
}

/// What to say of a container whose `/dev/shm` (`inspected`, in bytes, as `docker
/// inspect` prints it) is smaller than the `wanted` bytes; nothing if it is not.
fn smaller_shm(name: &str, wanted: u64, inspected: &str) -> Option<String> {
    let has: u64 = inspected.trim().parse().ok()?;
    (has < wanted).then(|| {
        format!(
            "    {name}: made with {} MB of /dev/shm, where this task now gives {} MB; tests \
             that fail with \"could not resize shared memory segment\" want it removed \
             (`docker rm --force {name}`, its test data goes with it) and made again",
            has / (1024 * 1024),
            wanted / (1024 * 1024)
        )
    })
}

fn docker(arguments: &[&str]) -> Result<()> {
    let result = capture("docker", arguments)?;
    if result.succeeded {
        Ok(())
    } else {
        bail!("`docker {}` failed: {}", arguments.join(" "), result.error)
    }
}

fn name_from_env(variable: &str, default: &str) -> String {
    std::env::var(variable)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| default.to_owned())
}

fn port_from_env(variable: &str, default: u16) -> Result<u16> {
    match std::env::var(variable) {
        Ok(value) if !value.trim().is_empty() => {
            let port: u16 = value
                .trim()
                .parse()
                .with_context(|| format!("{variable}={value:?} is no port number"))?;
            if port == 0 {
                bail!("{variable}=0 names no port to connect to");
            }
            Ok(port)
        }
        _ => Ok(default),
    }
}

/// Repeats `probe` until it succeeds or [`READY_TIMEOUT`] has passed.
fn wait_until(what: &str, mut probe: impl FnMut() -> Result<()>) -> Result<()> {
    let started = Instant::now();
    loop {
        if crate::steps::interrupted() {
            bail!("{what}: interrupted while waiting for it");
        }
        match probe() {
            Ok(()) => return Ok(()),
            Err(error) if started.elapsed() >= READY_TIMEOUT => {
                return Err(error.context(format!(
                    "{what} was not ready after {} seconds",
                    READY_TIMEOUT.as_secs()
                )));
            }
            Err(_) => std::thread::sleep(PROBE_INTERVAL),
        }
    }
}

/// Makes sure PostgreSQL runs and accepts connections. Returns the URLs for the tests:
/// of the role they connect as, and of the superuser.
///
/// # Errors
///
/// Fails if docker is missing, the container cannot be started, or the database does
/// not accept connections in time.
pub(crate) fn prepare_postgres(uses: &mut Uses) -> Result<PostgresUrls> {
    let container = Container::postgres()?;
    container.ensure_running(uses)?;
    wait_until(&container.name, || postgres_ready(&container))?;
    {
        // One run at a time: two that alter the role at once fail ("tuple concurrently
        // updated").
        let _setup = uses.lock_setup(&container.name)?;
        docker(&[
            "exec",
            &container.name,
            "psql",
            "--username",
            POSTGRES_USER,
            "--dbname",
            POSTGRES_DATABASE,
            "--set",
            "ON_ERROR_STOP=1",
            "--quiet",
            "--command",
            &test_role_statements(),
        ])
        .with_context(|| format!("making the role {POSTGRES_TEST_ROLE} in {}", container.name))?;
        allow_connections(&container)?;
    }
    say(&format!(
        "    {}: accepting connections on port {}, the tests connect as {POSTGRES_TEST_ROLE}",
        container.name, container.host_port
    ));
    Ok(PostgresUrls {
        tests: container.postgres_url(),
        superuser: container.postgres_superuser_url(),
    })
}

/// Raises the connections a container made before [`POSTGRES_MAX_CONNECTIONS`] takes,
/// which needs a restart of the server in it. It happens once to a container: other
/// test runs that use it at that moment lose their connections.
fn allow_connections(container: &Container) -> Result<()> {
    let psql = |command: &str| {
        capture(
            "docker",
            &[
                "exec",
                &container.name,
                "psql",
                "--username",
                POSTGRES_USER,
                "--dbname",
                POSTGRES_DATABASE,
                "--no-align",
                "--tuples-only",
                "--command",
                command,
            ],
        )
    };
    let shown = psql("SHOW max_connections")?;
    let now: u32 =
        shown.output.trim().parse().with_context(|| {
            format!("reading max_connections of {}: {:?}", container.name, shown)
        })?;
    if now >= POSTGRES_MAX_CONNECTIONS {
        return Ok(());
    }
    say(&format!(
        "    {}: takes {now} connections; restarting it to take {POSTGRES_MAX_CONNECTIONS}",
        container.name
    ));
    let altered = psql(&format!(
        "ALTER SYSTEM SET max_connections = {POSTGRES_MAX_CONNECTIONS}"
    ))?;
    if !altered.succeeded {
        bail!(
            "raising max_connections of {}: {}",
            container.name,
            altered.error
        );
    }
    docker(&["restart", &container.name])?;
    wait_until(&container.name, || postgres_ready(container))
}

/// Whether PostgreSQL in the container accepts connections over TCP: while the image
/// initializes a new database it answers on the socket only, and restarts once more
/// before it is ready.
fn postgres_ready(container: &Container) -> Result<()> {
    docker(&[
        "exec",
        &container.name,
        "pg_isready",
        "--host",
        "127.0.0.1",
        "--username",
        POSTGRES_USER,
        "--dbname",
        POSTGRES_DATABASE,
    ])
}

/// The PostgreSQL URLs the tests are given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PostgresUrls {
    /// Of the role the tests connect as, which the policies hold.
    pub(crate) tests: String,
    /// Of a superuser, for the tests of what the server makes of such a role.
    pub(crate) superuser: String,
}

/// The statements that make [`POSTGRES_TEST_ROLE`] what the tests need, whether it is
/// there already or not: a role that logs in, is no superuser, is held by row-level
/// security, and may make schemas in the test database. Every test database is a schema
/// the role makes, and so owns.
fn test_role_statements() -> String {
    format!(
        "DO $$ BEGIN CREATE ROLE {POSTGRES_TEST_ROLE}; \
         EXCEPTION WHEN duplicate_object THEN NULL; END $$; \
         ALTER ROLE {POSTGRES_TEST_ROLE} LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB \
         NOCREATEROLE PASSWORD '{POSTGRES_PASSWORD}'; \
         GRANT CREATE ON DATABASE {POSTGRES_DATABASE} TO {POSTGRES_TEST_ROLE};"
    )
}

/// Makes sure the object store runs and has the test bucket. Returns the endpoint and
/// the bucket for the tests.
///
/// # Errors
///
/// Fails if docker is missing, the container cannot be started, or the bucket cannot be
/// created in time.
pub(crate) fn prepare_object_store(uses: &mut Uses) -> Result<(String, &'static str)> {
    let container = Container::object_store()?;
    container.ensure_running(uses)?;
    wait_until(&container.name, || {
        create_bucket(container.host_port, S3_BUCKET)
    })?;
    say(&format!(
        "    {}: bucket {S3_BUCKET} is there, port {}",
        container.name, container.host_port
    ));
    Ok((container.s3_endpoint(), S3_BUCKET))
}

/// Creates a bucket with a plain `PUT /<bucket>`; an existing bucket is fine.
fn create_bucket(port: u16, bucket: &str) -> Result<()> {
    // With a limit: on some machines a connection to a closed loopback port hangs
    // instead of being refused, and one hanging probe would outlast the whole wait.
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
        .with_context(|| format!("connecting to port {port}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    write!(
        stream,
        "PUT /{bucket} HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )?;
    let mut reply = String::new();
    // The status line is all that matters; a reset after it is no failure.
    let _ = stream.read_to_string(&mut reply);
    let status = status_code(&reply)
        .with_context(|| format!("the bucket request got no HTTP answer: {reply:?}"))?;
    // 409: the bucket exists already.
    if status == 200 || status == 409 {
        Ok(())
    } else {
        bail!("creating the bucket {bucket} answered {status}")
    }
}

/// The status code of an HTTP/1.1 response.
fn status_code(response: &str) -> Option<u16> {
    let mut parts = response.lines().next()?.split_whitespace();
    if !parts.next()?.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

#[cfg(test)]
mod tests;
