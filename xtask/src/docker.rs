//! `cargo xtask docker`: builds the two images of `docker/`, and the server's once more
//! from a source older than the build cache, which must still be what is built. Then it
//! starts them with `docker/compose.yaml` as an installation would, and tries them from
//! outside: ping names the commit, sign up, a board, a card, words in its body, and a
//! search that finds them; a reset link mailed by each provider's settings to a fake
//! provider, and no start with `email_verification` and no mail. Then it looks at what
//! the installation must not do (secrets in the images, a database role past the wall,
//! ports open to the host, processes as root), reads the profile `tunnel` without
//! starting it, backs up, restores into new volumes and updates with the scripts of
//! `docker/`, and takes everything down with its volumes, whatever happened.
//! `--push-plan` prints how the images go to GHCR.
//!
//! It needs `docker` with compose 2.24 or newer, `curl`, `git` and `tar`. The images are built
//! natively, for the processor of the machine; the other one is built on a machine of
//! that kind. The images stay, to be pushed (`--push-plan`); `docker image rm` removes
//! them.

mod backup;
mod checks;
mod freshness;
mod images;
mod mail;
mod pasted;
mod scenario;
mod tunnel;

use std::fs;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context as _, Result, bail, ensure};

use crate::steps::{self, Captured, capture, say};

pub(crate) use images::print_push_plan;

/// The prefix of the compose project of a trial; the rest names the checkout, so that
/// the trials of two worktrees do not take each other down, and a killed run's is found
/// and taken down by the next one in the same worktree.
const PROJECT_PREFIX: &str = "fvoci-smoke";

/// How long the services may take to become healthy, in seconds: the search server's
/// first start makes its certificates and its security index.
const WAIT_SECONDS: &str = "300";

/// How long `docker compose down` gives each container to stop, in seconds: the server
/// finishes its requests and lets go of its lock.
const STOP_SECONDS: &str = "40";

/// The oldest compose this was tried with features of (secrets from the environment,
/// optional dependencies, `ps --format json` a line per container, `--wait-timeout`).
const OLDEST_COMPOSE: (u32, u32) = (2, 24);

/// Variables of the environment that would win over the trial's file or choose other
/// files and services: they are not passed on to compose.
const COMPOSE_VARIABLES: [&str; 4] = [
    "COMPOSE_PROFILES",
    "COMPOSE_FILE",
    "COMPOSE_PROJECT_NAME",
    "COMPOSE_ENV_FILES",
];

/// The installation under trial: where it answers and what it was given.
pub(crate) struct Trial {
    root: PathBuf,
    project: String,
    env_file: PathBuf,
    /// The names the trial's file sets, which the caller's environment must not override.
    env_names: Vec<String>,
    /// The address of fvoci on the host.
    pub(crate) base: String,
    /// The secrets the installation was given, to be found nowhere they must not be.
    pub(crate) secrets: Vec<String>,
    /// The password of the role fvoci, one of them.
    role_password: String,
    pub(crate) images: images::Names,
    /// The mail providers' keys the trial hands fvoci.
    mail: mail::Keys,
}

impl std::fmt::Debug for Trial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Trial")
            .field("project", &self.project)
            .field("base", &self.base)
            .field("secrets", &format_args!("<{} hidden>", self.secrets.len()))
            .finish_non_exhaustive()
    }
}

impl Trial {
    /// `docker compose` on the trial's project and file, with nothing of the caller's
    /// environment that would override them.
    fn command(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new("docker");
        command
            .args(["compose", "--project-name", &self.project, "--env-file"])
            .arg(&self.env_file)
            .arg("--file")
            .arg(self.root.join("docker/compose.yaml"))
            .args(arguments);
        self.scrub(&mut command);
        command
    }

    /// One of the scripts of `docker/` (`backup.sh`, ...) on the trial's project, files
    /// and `.env`, as an operator runs it, with nothing of the caller's environment that
    /// would override them.
    fn script(&self, name: &str, arguments: &[&str]) -> Command {
        let mut command = Command::new(self.root.join("docker").join(name));
        command
            .args(["--project-name", &self.project, "--env-file"])
            .arg(&self.env_file)
            .arg("--file")
            .arg(self.root.join("docker/compose.yaml"))
            .args(arguments);
        self.scrub(&mut command);
        command
    }

    /// Takes out of `command`'s environment what would win over the trial's file.
    fn scrub(&self, command: &mut Command) {
        for name in self
            .env_names
            .iter()
            .map(String::as_str)
            .chain(COMPOSE_VARIABLES)
        {
            command.env_remove(name);
        }
        // Every setting of the server and the database the compose file reads: a value
        // exported for a server run by hand would win over the trial's file.
        for (name, _) in std::env::vars_os() {
            if name
                .to_str()
                .is_some_and(|name| name.starts_with("FVOCI_") || name.starts_with("POSTGRES_"))
            {
                command.env_remove(&name);
            }
        }
    }

    /// Runs `docker compose` on the trial's project, and returns what it wrote.
    pub(crate) fn compose(&self, arguments: &[&str]) -> Result<Captured> {
        let output = self
            .command(arguments)
            .output()
            .context("starting docker compose")?;
        Ok(Captured {
            succeeded: output.status.success(),
            output: String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            error: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        })
    }

    /// The label of every container, volume and network of the trial.
    fn label(&self) -> String {
        format!("label=com.docker.compose.project={}", self.project)
    }
}

/// Takes the trial down and removes its files when it is dropped, also on an early
/// return or a panic; [`Teardown::finish`] does it and says how it went.
struct Teardown<'a> {
    trial: &'a Trial,
    directory: PathBuf,
    done: bool,
}

impl Teardown<'_> {
    fn finish(mut self) -> Result<()> {
        self.done = true;
        let down = take_down(self.trial);
        let removed = remove_files(self.trial, &self.directory);
        down.and(removed)
    }
}

impl Drop for Teardown<'_> {
    fn drop(&mut self) {
        if !self.done {
            if let Err(error) = take_down(self.trial) {
                say(&format!(
                    "the trial {} may be left up: {error:#}; the next run takes it down",
                    self.trial.project
                ));
            }
            let _ = remove_files(self.trial, &self.directory);
        }
    }
}

/// Removes leftovers of a trial (or of a previous killed run) under `directory`: its
/// secrets file, session cookies, and trial backup folders. Each is tried whatever the
/// others did: all may hold secrets.
fn clean_leftovers(directory: &Path) -> Result<()> {
    let cookies = match fs::remove_file(directory.join("cookies.txt")) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).context("removing the trial's cookies")
        }
        _ => Ok(()),
    };
    let env = match fs::remove_file(directory.join("smoke.env")) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).context("removing the file of the trial's secrets")
        }
        _ => Ok(()),
    };
    let backups = backup::folder(directory);
    let removed = match fs::remove_dir_all(&backups) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error)
            .with_context(|| format!("removing the trial's backups, {}", backups.display())),
        _ => Ok(()),
    };
    cookies.and(env).and(removed)
}

/// Removes the trial's files: its secrets, the session of a server that is gone, and
/// any trial backups. Each is tried whatever the others did.
fn remove_files(trial: &Trial, directory: &Path) -> Result<()> {
    let extra_env = if trial.env_file == directory.join("smoke.env") {
        Ok(())
    } else {
        match fs::remove_file(&trial.env_file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).context("removing the file of the trial's secrets")
            }
            _ => Ok(()),
        }
    };
    clean_leftovers(directory).and(extra_env)
}

/// Builds, starts, tries and takes down the installation.
///
/// # Errors
///
/// Fails if a step fails; the installation is taken down all the same.
pub(crate) fn run(root: &Path) -> Result<()> {
    let started = Instant::now();
    check_compose()?;
    let directory = root.join("target/docker-smoke");
    // Remove secrets and backups left behind by an earlier killed run right away, before
    // building images or starting a new trial.
    clean_leftovers(&directory)?;
    let names = images::Names::of_checkout(root)?;
    images::build(root, &names)?;
    freshness::probe(root)?;
    let built = started.elapsed();

    fs::create_dir_all(&directory).with_context(|| format!("creating {}", directory.display()))?;
    let port = free_port()?;
    let secrets = Secrets::new()?;
    let mail_keys = mail::Keys::new(root)?;
    let mut env_text = secrets.env_file(&names, port);
    env_text.push_str(&mail_keys.env_lines());
    let env_file = directory.join("smoke.env");
    let trial = Trial {
        root: root.to_owned(),
        project: format!("{PROJECT_PREFIX}-{}", checkout_key(root)),
        env_names: env_text
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name.to_owned()))
            .collect(),
        env_file,
        base: format!("http://localhost:{port}"),
        secrets: secrets
            .values()
            .into_iter()
            .chain(mail_keys.secrets())
            .collect(),
        role_password: secrets.database,
        images: names,
        mail: mail_keys,
    };
    let teardown = Teardown {
        trial: &trial,
        directory: directory.clone(),
        done: false,
    };
    write_private(&trial.env_file, &env_text)?;
    // What a killed run left behind is this trial's too.
    take_down(&trial)?;
    let outcome = try_installation(&trial, &directory);
    match (outcome, teardown.finish()) {
        (Err(failed), Err(down)) => {
            return Err(failed.context(format!("and taking the trial down failed: {down:#}")));
        }
        (outcome, down) => outcome.and(down)?,
    }
    say(&format!(
        "\ndocker: passed in {:.0}s (of which building {:.0}s)",
        started.elapsed().as_secs_f64(),
        built.as_secs_f64()
    ));
    Ok(())
}

/// Fails, saying why, if `docker compose` is older than [`OLDEST_COMPOSE`].
fn check_compose() -> Result<()> {
    let version = capture("docker", &["compose", "version", "--short"])?;
    if !version.succeeded {
        bail!("docker compose is needed: {}", version.error);
    }
    let parsed: Vec<u32> = version
        .output
        .trim_start_matches('v')
        .split(['.', '-', '+'])
        .take(2)
        .map_while(|part| part.parse().ok())
        .collect();
    match parsed[..] {
        [major, minor] if (major, minor) >= OLDEST_COMPOSE => Ok(()),
        _ => bail!(
            "docker compose {} is older than {}.{}, the oldest this trial was made for",
            version.output,
            OLDEST_COMPOSE.0,
            OLDEST_COMPOSE.1
        ),
    }
}

/// A short, stable name of the checkout at `root`: FNV-1a of its path.
fn checkout_key(root: &Path) -> String {
    let hash = root
        .to_string_lossy()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        });
    format!("{:08x}", hash >> 32)
}

/// Starts the installation and puts it through the scenario and the checks.
fn try_installation(trial: &Trial, directory: &Path) -> Result<()> {
    say("\n==> starting the installation");
    let up = trial.compose(&[
        "up",
        "--detach",
        "--no-build",
        "--wait",
        "--wait-timeout",
        WAIT_SECONDS,
    ])?;
    if !up.succeeded {
        let logs = trial.compose(&["logs", "--no-color", "--tail", "60"])?;
        bail!(
            "the installation did not become healthy: {}\n{}",
            up.error,
            logs.output
        );
    }
    let checks = steps::run_all(
        "docker",
        vec![
            steps::Step::new("sign up, board, card, body, search", || {
                scenario::run(trial, directory)
            }),
            steps::Step::new(
                "mail by each provider, and none refused at the start",
                || mail::providers(trial, directory),
            ),
            steps::Step::new("the server's database role", || {
                checks::database_role(trial)
            }),
            steps::Step::new("only fvoci's port is published", || {
                checks::published_ports(trial)
            }),
            steps::Step::new("no process runs as root", || checks::processes(trial)),
            steps::Step::new("the worker of editing together starts", || {
                checks::collab_worker(trial)
            }),
            steps::Step::new("no secret in the images, containers or logs", || {
                checks::secrets(trial)
            }),
            steps::Step::new("the profile tunnel, and nothing of it without", || {
                tunnel::profile(trial)
            }),
            steps::Step::new(
                "lines pasted wrong in the .env: each script stops, hiding them",
                || pasted::refused_hidden(trial, directory),
            ),
            // Last: it takes the volumes away and makes the installation anew.
            steps::Step::new("back up, take the volumes away, restore, update", || {
                backup::round_trip(trial, directory)
            }),
        ],
    )
    .into_result();
    if checks.is_err() {
        let logs = trial.compose(&["logs", "--no-color", "--tail", "60", "fvoci"])?;
        say(&format!("\nfvoci's last lines:\n{}", logs.output));
    }
    checks
}

/// Takes the installation down with its volumes and makes sure nothing of it is left.
/// The `down` runs in a process group of its own, so that a second Ctrl-C does not
/// stop it half-way.
fn take_down(trial: &Trial) -> Result<()> {
    use std::os::unix::process::CommandExt as _;

    // It shares fvoci's network, which goes with fvoci; what is left of it is said
    // once the rest is down.
    let fake_left = mail::remove_fake(trial);
    let status = trial
        .command(&[
            "--profile",
            "s3",
            "--profile",
            "mail",
            "--profile",
            "tunnel",
            "down",
            "--volumes",
            "--remove-orphans",
            "--timeout",
            STOP_SECONDS,
        ])
        .process_group(0)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("starting docker compose down")?;
    if !status.success() {
        bail!("docker compose down ended with {status}");
    }
    // The tags update.sh gave the images it replaced, the trial's own: each image keeps
    // its other names. Absent unless an update ran.
    if let Ok(tags) = backup::kept_tags(trial) {
        for tag in tags {
            let _ = capture("docker", &["image", "rm", &tag]);
        }
    }
    let label = trial.label();
    let listings: [&[&str]; 3] = [
        &["container", "ls", "--all"],
        &["volume", "ls"],
        &["network", "ls"],
    ];
    for listing in listings {
        let mut arguments = listing.to_vec();
        arguments.extend(["--quiet", "--filter", &label]);
        let left = capture("docker", &arguments)?;
        if !left.succeeded {
            bail!("docker {} failed: {}", listing.join(" "), left.error);
        }
        if !left.output.is_empty() {
            bail!("the trial left a {}: {}", listing[0], left.output);
        }
    }
    fake_left
}

/// What a script wrote, both streams, and whether it succeeded.
fn script_output(mut command: Command, name: &str) -> Result<(bool, String)> {
    let output = command
        .output()
        .with_context(|| format!("starting {name}"))?;
    let said = String::from_utf8_lossy(&output.stdout) + String::from_utf8_lossy(&output.stderr);
    Ok((output.status.success(), said.into_owned()))
}

/// Fails if `text` holds any of `values`, or a substantial fragment or escaped form of
/// one, saying what wrote it.
pub(super) fn no_values_in(values: &[&str], what: &str, text: &str) -> Result<()> {
    if values.iter().any(|value| value_appears_in(value, text)) {
        bail!("{what} wrote one of the trial's secrets");
    }
    Ok(())
}

/// Fails if `text` holds one of the trial's secrets (whole, fragmented, or escaped),
/// saying what wrote it.
fn no_secret_in(trial: &Trial, what: &str, text: &str) -> Result<()> {
    let secrets: Vec<&str> = trial.secrets.iter().map(String::as_str).collect();
    no_values_in(&secrets, what, text)
}

/// Whether `secret` appears in `text` whole, by line, by any 12-character fragment
/// (stepped by 6 so any >=18-character slice is caught), or in JSON/compose escaped form.
fn value_appears_in(secret: &str, text: &str) -> bool {
    const FRAGMENT: usize = 12;
    const STEP: usize = 6;
    if secret.is_empty() {
        return false;
    }
    if text.contains(secret) {
        return true;
    }
    if secret.contains('$') && text.contains(&secret.replace('$', "$$")) {
        return true;
    }
    if secret.contains(['\\', '"', '\n', '\r'])
        && let Ok(json) = serde_json::to_string(secret)
        && let Some(inner) = json
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
        && !inner.is_empty()
        && text.contains(inner)
    {
        return true;
    }
    for line in secret.lines() {
        let chars: Vec<char> = line.chars().collect();
        if chars.len() < FRAGMENT {
            continue;
        }
        if text.contains(line) {
            return true;
        }
        let mut start = 0;
        while start + FRAGMENT <= chars.len() {
            let slice: String = chars[start..start + FRAGMENT].iter().collect();
            if text.contains(&slice) {
                return true;
            }
            start += STEP;
        }
        let tail: String = chars[chars.len() - FRAGMENT..].iter().collect();
        if text.contains(&tail) {
            return true;
        }
    }
    false
}

/// fvoci's container and when it started.
fn fvoci_started(trial: &Trial) -> Result<String> {
    let container = trial.compose(&["ps", "--quiet", "fvoci"])?;
    ensure!(
        container.succeeded && !container.output.is_empty(),
        "fvoci has no container: {}",
        container.error
    );
    let started = capture(
        "docker",
        &[
            "inspect",
            "--format",
            "{{.Id}} {{.State.StartedAt}}",
            &container.output,
        ],
    )?;
    ensure!(
        started.succeeded,
        "docker inspect failed: {}",
        started.error
    );
    Ok(started.output)
}

/// A path for a file that holds the trial's secrets, in the folder of its backups,
/// which the take-down removes however the trial ended.
fn secrets_file(directory: &Path, name: &str) -> Result<PathBuf> {
    let folder = backup::folder(directory);
    fs::create_dir_all(&folder).with_context(|| format!("creating {}", folder.display()))?;
    Ok(folder.join(name))
}

/// A port of the loopback interface that nothing listens on now.
fn free_port() -> Result<u16> {
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).context("finding a free local port")?;
    Ok(listener.local_addr()?.port())
}

/// Writes `text` to a file only its owner can read.
fn write_private(path: &Path, text: &str) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("writing {}", path.display()))?;
    // `mode` only takes effect when creating a new file; tighten an existing file too.
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting permissions on {}", path.display()))?;
    file.write_all(text.as_bytes())
        .with_context(|| format!("writing {}", path.display()))
}

/// The secrets of one trial, made anew from the system's random source.
struct Secrets {
    postgres: String,
    database: String,
    secret_key: String,
    search: String,
    metrics: String,
}

impl Secrets {
    fn new() -> Result<Self> {
        Ok(Self {
            postgres: hex(&random::<24>()?),
            database: hex(&random::<24>()?),
            secret_key: base64(&random::<32>()?),
            search: hex(&random::<24>()?),
            metrics: hex(&random::<32>()?),
        })
    }

    fn values(&self) -> Vec<String> {
        vec![
            self.postgres.clone(),
            self.database.clone(),
            self.secret_key.clone(),
            self.search.clone(),
            self.metrics.clone(),
        ]
    }

    /// The `.env` of the trial, as an operator writes it from `.env.example`.
    fn env_file(&self, images: &images::Names, port: u16) -> String {
        format!(
            "FVOCI_IMAGE={}\nFVOCI_SEARCH_IMAGE={}\nFVOCI_HOST_PORT={port}\n\
             FVOCI_SERVER_URL=http://localhost:{port}\nFVOCI_ORG_CREATION=open\n\
             POSTGRES_PASSWORD={}\nFVOCI_DB_PASSWORD={}\nFVOCI_SECRET_KEY={}\n\
             FVOCI_SEARCH_PASSWORD={}\nFVOCI_METRICS_TOKEN={}\n\
             FVOCI_S3_ACCESS_KEY_ID=\nFVOCI_S3_SECRET_ACCESS_KEY=\nFVOCI_SMTP_PASSWORD=\n",
            images.server.local,
            images.search.local,
            self.postgres,
            self.database,
            self.secret_key,
            self.search,
            self.metrics
        )
    }
}

/// `N` bytes from the system's random source.
fn random<const N: usize>() -> Result<[u8; N]> {
    use std::io::Read as _;

    let mut bytes = [0; N];
    fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut bytes))
        .context("reading /dev/urandom")?;
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    bytes.iter().fold(String::new(), |mut text, byte| {
        // Writing to a String does not fail.
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// Standard base64 with padding, as `FVOCI_SECRET_KEY` is written.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let group = chunk.iter().enumerate().fold(0_u32, |group, (at, byte)| {
            group | u32::from(*byte) << (16 - 8 * at)
        });
        for at in 0..4 {
            if at <= chunk.len() {
                let index = (group >> (18 - 6 * at)) & 0x3f;
                text.push(char::from(ALPHABET[usize::try_from(index).unwrap_or(0)]));
            } else {
                text.push('=');
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_the_standard_alphabet_with_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(&[0xfb, 0xff]), "+/8=");
        assert_eq!(base64(&[0; 32]).len(), 44);
    }

    #[test]
    fn a_checkout_is_named_the_same_each_time_and_apart_from_another() {
        let here = checkout_key(Path::new("/work/one"));
        assert_eq!(here, checkout_key(Path::new("/work/one")));
        assert_ne!(here, checkout_key(Path::new("/work/two")));
        assert_eq!(here.len(), 8);
    }

    #[test]
    fn a_trial_shows_no_secret_when_debugged() {
        let trial = Trial {
            root: PathBuf::from("/work"),
            project: "fvoci-smoke-0".to_owned(),
            env_file: PathBuf::from("/work/smoke.env"),
            env_names: Vec::new(),
            base: "http://localhost:1".to_owned(),
            secrets: vec!["a-secret-value".to_owned()],
            role_password: "a-secret-value".to_owned(),
            images: images::Names::for_tests(),
            mail: mail::Keys::for_tests(),
        };
        let shown = format!("{trial:?}");
        assert!(!shown.contains("a-secret-value"), "{shown}");
    }

    #[test]
    fn hex_writes_two_digits_a_byte() {
        assert_eq!(hex(&[0, 0xab, 0x10]), "00ab10");
    }

    struct TempDir(PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn write_private_tightens_existing_looser_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir =
            TempDir(std::env::temp_dir().join(format!("fvoci-xtask-priv-{}", std::process::id())));
        fs::create_dir_all(&dir.0).expect("temp dir");
        let path = dir.0.join("secret.env");
        fs::write(&path, "old").expect("seed file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).expect("loosen mode");

        write_private(&path, "new secret").expect("write_private");
        let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "existing file mode must be tightened to 0600, got {mode:#o}"
        );
    }

    #[test]
    fn no_secret_in_catches_fragments_and_escaped_forms() {
        let secret = "AbCdEf0123456789GhIjKlMnOpQrStUvWxYz0123456=";
        let trial = Trial {
            root: PathBuf::from("/work"),
            project: "fvoci-smoke-0".to_owned(),
            env_file: PathBuf::from("/work/smoke.env"),
            env_names: Vec::new(),
            base: "http://localhost:1".to_owned(),
            secrets: vec![secret.to_owned()],
            role_password: "unused".to_owned(),
            images: images::Names::for_tests(),
            mail: mail::Keys::for_tests(),
        };
        assert!(no_secret_in(&trial, "clean", "ordinary log output").is_ok());
        assert!(
            no_secret_in(&trial, "full", &format!("leaked {secret}")).is_err(),
            "full secret must be caught"
        );
        // Truncated/quoted fragment of the secret (e.g., compose quoting before a $ or line break).
        assert!(
            no_secret_in(&trial, "prefix", &format!("prefix '{}'", &secret[..16])).is_err(),
            "prefix fragment must be caught"
        );
        assert!(
            no_secret_in(
                &trial,
                "suffix",
                &format!("suffix \"{}\"", &secret[secret.len() - 16..])
            )
            .is_err(),
            "suffix fragment must be caught"
        );
        let multiline = "part1\npart2$tail";
        assert!(
            no_values_in(&[multiline], "json", r#"value="part1\npart2$tail""#).is_err(),
            "JSON-escaped secret must be caught"
        );
        assert!(
            no_values_in(&["abc$def12345"], "dollar", "abc$$def12345").is_err(),
            "compose-escaped $$ secret must be caught"
        );
        // Trailing quote in a secret keeps its escaped quote rather than collapsing to `\`.
        assert!(
            no_values_in(&["token\""], "escaped-quote", r#"value="token\"""#).is_err(),
            "JSON-escaped trailing quote must be caught"
        );
        assert!(
            no_values_in(&["\""], "lone-backslash", r"path\to\file").is_ok(),
            "escaped quote must not collapse to a lone backslash"
        );
    }

    #[test]
    fn clean_leftovers_removes_stale_smoke_env_cookies_and_backups() {
        let dir =
            TempDir(std::env::temp_dir().join(format!("fvoci-xtask-clean-{}", std::process::id())));
        let stale_backup = backup::folder(&dir.0).join("old-run");
        fs::create_dir_all(&stale_backup).expect("backup dir");
        fs::write(dir.0.join("smoke.env"), "FVOCI_SECRET_KEY=stale").expect("env");
        fs::write(dir.0.join("cookies.txt"), "session=stale").expect("cookies");
        fs::write(stale_backup.join("env"), "FVOCI_SECRET_KEY=stale").expect("backup env");

        clean_leftovers(&dir.0).expect("clean_leftovers");
        assert!(!dir.0.join("smoke.env").exists());
        assert!(!dir.0.join("cookies.txt").exists());
        assert!(!backup::folder(&dir.0).exists());
        // Calling again when absent succeeds cleanly.
        clean_leftovers(&dir.0).expect("clean_leftovers when absent");
    }
}
