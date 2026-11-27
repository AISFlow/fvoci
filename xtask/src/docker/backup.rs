//! The scripts that keep an installation, run as an operator runs them: `backup.sh`
//! (with its files in the volume, and again as if they were in S3), the volumes taken
//! away, `restore.sh` into the new ones, and `update.sh`, which refuses an `.env` that
//! lacks a line before it stops anything, then updates and says how long fvoci was
//! down.

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail, ensure};
use serde_json::json;

use super::scenario::{self, Client};
use super::{Trial, fvoci_started, no_secret_in, no_values_in, script_output, secrets_file};
use crate::steps::{capture, say};

/// What the tables hold, table by table, as PostgreSQL's superuser counts; but the
/// queue of changes for the search server, which the restore's new index fills and the
/// server empties in the seconds after.
const COUNT_ROWS: &str = "SELECT table_name || '=' || (xpath('/row/c/text()', \
     query_to_xml(format('SELECT count(*) AS c FROM public.%I', table_name), false, true, '')))[1]::text \
     FROM information_schema.tables WHERE table_schema = 'public' AND table_type = 'BASE TABLE' \
     AND table_name <> 'search_outbox' ORDER BY table_name";

/// Every file of the files' volume with its SHA-256, as fvoci's image lists them; the
/// readiness check's file and files being written are not the installation's.
const LIST_FILES: &str = "cd /data/files && find . -type f -not -path './.tmp/*' \
     -not -name .fvoci-ready -exec sha256sum {} + | sort -k 2";

/// A secret the trial's `.env` sets, whose line `update.sh` must miss.
const LACKING: &str = "FVOCI_SES_SECRET_ACCESS_KEY";

/// Where the trial's backups go, in `main` and `s3`: removed with the trial's other
/// files.
pub(super) fn folder(directory: &Path) -> PathBuf {
    directory.join("trial-backups")
}

/// Backs up, takes the volumes away, restores, and updates.
///
/// # Errors
///
/// Fails at the first script or comparison that does not go as it should.
pub(super) fn round_trip(trial: &Trial, directory: &Path) -> Result<()> {
    let backups = folder(directory).join("main");
    let kept = Kept::write(trial, directory)?;
    let rows = counted_rows(trial)?;
    let files = listed_files(trial)?;
    ensure!(
        files.contains(&format!("/{}/", kept.board)),
        "the upload is not in the volume: {files}"
    );

    let made = run_script(trial, "backup.sh", &["--dir", &backups.to_string_lossy()])?;
    let backup = only_backup(&backups)?;
    check_private(&backups, &backup, trial)?;
    say(&format!("    {}", last_said(&made, "backup")));
    backs_up_no_files_from_s3(trial, directory)?;
    names_a_quoted_project(trial, directory)?;
    reads_the_key_as_compose_does(trial, directory)?;
    refuses_missing_key_and_mismatched_made(trial, directory)?;
    refuses_a_full_installation(trial, directory, &backup, &rows)?;

    take_volumes_away(trial)?;
    refuses_a_volume_with_files(trial, &backup)?;
    let restored = run_script(trial, "restore.sh", &[&backup.to_string_lossy()])?;
    let indexed = restored
        .split_once("made anew from ")
        .and_then(|(_, rest)| rest.split_once(' '))
        .and_then(|(count, _)| count.parse::<u32>().ok());
    ensure!(
        indexed.is_some_and(|count| count > 0),
        "restore.sh did not make the search index anew from the boards: {restored}"
    );
    ensure!(
        restored.contains("every file the records name is in the volume"),
        "restore.sh did not find every file: {restored}"
    );
    let rows_now = counted_rows(trial)?;
    ensure!(
        rows_now == rows,
        "the rows differ after the restore:\nbefore {rows}\nafter  {rows_now}"
    );
    let files_now = listed_files(trial)?;
    ensure!(
        files_now == files,
        "the files differ after the restore:\nbefore {files}\nafter  {files_now}"
    );
    kept.check(trial, directory)?;
    say(&format!(
        "    {}: rows, files, login, the file's bytes and the search are as before",
        last_said(&restored, "restore")
    ));

    refuses_a_lacking_env(trial, directory)?;
    refuses_a_profile_off(trial, directory)?;
    updates(trial, &backups)
}

/// What is done before one run of `restore.sh` that must be refused.
enum Before {
    /// Nothing: fvoci's container is there.
    Nothing,
    /// The `.env` given has another `FVOCI_SECRET_KEY`.
    AnotherKey,
    /// fvoci's container is removed: the database is the full one.
    RemoveFvoci,
}

/// `restore.sh` refuses an `.env` with another secret key, then an installation with a
/// container of fvoci, then, the container removed, a database that holds tables; and
/// changes nothing.
fn refuses_a_full_installation(
    trial: &Trial,
    directory: &Path,
    backup: &Path,
    rows: &str,
) -> Result<()> {
    let backup = backup.to_string_lossy();
    let other_env = secrets_file(directory, "another-key.env")?;
    let refusals = [
        ("FVOCI_SECRET_KEY is not the backup's", Before::AnotherKey),
        ("fvoci has a container", Before::Nothing),
        ("the database holds", Before::RemoveFvoci),
    ];
    for (refusal, before) in refusals {
        let other_env_text = other_env.to_string_lossy();
        let mut arguments = vec![backup.as_ref()];
        let mut extra_key = None;
        match before {
            Before::Nothing => {}
            Before::AnotherKey => {
                let text =
                    std::fs::read_to_string(&trial.env_file).context("reading the trial's .env")?;
                let mut lines: Vec<String> = text
                    .lines()
                    .filter(|line| !line.starts_with("FVOCI_SECRET_KEY="))
                    .map(str::to_owned)
                    .collect();
                let generated = super::base64(&super::random::<32>()?);
                lines.push(format!("FVOCI_SECRET_KEY={generated}"));
                extra_key = Some(generated);
                super::write_private(&other_env, &lines.join("\n"))?;
                arguments.extend(["--env-file", other_env_text.as_ref()]);
            }
            Before::RemoveFvoci => {
                let removed = trial.compose(&["rm", "--stop", "--force", "fvoci"])?;
                ensure!(
                    removed.succeeded,
                    "removing fvoci failed: {}",
                    removed.error
                );
            }
        }
        let outcome = script_output(trial.script("restore.sh", &arguments), "restore.sh");
        let _ = std::fs::remove_file(&other_env);
        let (succeeded, said) = outcome?;
        no_secret_in(trial, "restore.sh", &said)?;
        if let Some(key) = extra_key.as_deref() {
            no_values_in(&[key], "restore.sh", &said)?;
        }
        ensure!(
            !succeeded && said.contains(refusal) && said.contains("Nothing was changed"),
            "restore.sh did not refuse, saying \"{refusal}\": {said}"
        );
        let rows_now = counted_rows(trial)?;
        ensure!(rows_now == rows, "restore.sh refused but the rows changed");
    }
    say("    restore.sh refuses another secret key, fvoci's container and a full database");
    Ok(())
}

/// A backup whose `.env` writes `FVOCI_SECRET_KEY` some way, given to `restore.sh`.
struct KeyCase {
    what: &'static str,
    /// The lines of the backup's `.env` in place of the trial's key's line.
    backup: String,
    /// The lines of the `.env` given in place of the trial's key's line; the trial's
    /// `.env` when `None`.
    given: Option<String>,
    /// `FVOCI_SECRET_KEY` of the environment, which wins over the `.env` given.
    shell: Option<String>,
    /// What `restore.sh` must refuse saying.
    refusal: &'static str,
}

/// `restore.sh` reads the backup's `FVOCI_SECRET_KEY` as compose reads an `.env`: a key
/// written through a variable is the same key; a `$` that compose reads as a variable
/// makes another key, though the backup's text is the value of the `.env` given; and
/// the environment's key, which compose gives fvoci over the `.env`'s, is compared with
/// the backup's own; a backup's `.env` that compose refuses is refused without a word of
/// it. Each backup is a folder of empty files and an `env`: the key is compared before
/// they are read, and the same key is refused next for fvoci's container. No case may
/// pass by not finding the key.
fn reads_the_key_as_compose_does(trial: &Trial, directory: &Path) -> Result<()> {
    let text = std::fs::read_to_string(&trial.env_file).context("reading the trial's .env")?;
    let key = text
        .lines()
        .find_map(|line| line.strip_prefix("FVOCI_SECRET_KEY="))
        .context("the trial's .env has no FVOCI_SECRET_KEY")?;
    let other = super::base64(&super::random::<32>()?);
    let pasted = format!("S{}", super::hex(&super::random::<8>()?));
    let with = |lines: &str| -> String {
        text.lines()
            .filter(|line| !line.starts_with("FVOCI_SECRET_KEY="))
            .chain(lines.lines())
            .collect::<Vec<_>>()
            .join("\n")
    };
    let (same, another) = (
        "fvoci has a container",
        "FVOCI_SECRET_KEY is not the backup's",
    );
    let cases = [
        KeyCase {
            what: "the backup's key through a variable",
            backup: format!("FVOCI_KEY_PART={key}\nFVOCI_SECRET_KEY=${{FVOCI_KEY_PART}}"),
            given: None,
            shell: None,
            refusal: same,
        },
        KeyCase {
            what: "a $ in the backup's key, written in quotes in the .env given",
            backup: format!("FVOCI_SECRET_KEY={key}$FVOCI_KEY_TAIL"),
            given: Some(format!("FVOCI_SECRET_KEY='{key}$FVOCI_KEY_TAIL'")),
            shell: None,
            refusal: another,
        },
        KeyCase {
            what: "another key in the environment",
            backup: format!("FVOCI_SECRET_KEY={key}"),
            given: None,
            shell: Some(other.clone()),
            refusal: another,
        },
        KeyCase {
            what: "a multiline key with an inner NAME= line against its first line",
            backup: format!("FVOCI_SECRET_KEY=\"{key}\\nINNER=1\\n{other}\""),
            given: None,
            shell: None,
            refusal: another,
        },
        KeyCase {
            what: "a key's line pasted wrong in the backup's .env",
            backup: format!("FVOCI_SECRET_KEY={key}\nMIIEvQ+{pasted}="),
            given: None,
            shell: None,
            refusal: "compose cannot read",
        },
    ];
    let given_env = secrets_file(directory, "key-given.env")?;
    for (at, case) in cases.iter().enumerate() {
        let backup = folder(directory).join(format!("key-{at}"));
        std::fs::create_dir_all(&backup).context("making a backup's folder")?;
        for empty in ["db.dump", "files.tar"] {
            std::fs::write(backup.join(empty), "").context("writing a backup's file")?;
        }
        super::write_private(&backup.join("env"), &with(&case.backup))?;
        let (backup_text, given_text) = (backup.to_string_lossy(), given_env.to_string_lossy());
        let mut arguments = vec![backup_text.as_ref()];
        if let Some(lines) = &case.given {
            super::write_private(&given_env, &with(lines))?;
            arguments.extend(["--env-file", given_text.as_ref()]);
        }
        let mut command = trial.script("restore.sh", &arguments);
        if let Some(shell) = &case.shell {
            command.env("FVOCI_SECRET_KEY", shell);
        }
        let outcome = script_output(command, "restore.sh");
        let _ = std::fs::remove_file(&given_env);
        let _ = std::fs::remove_dir_all(&backup);
        let (succeeded, said) = outcome?;
        no_secret_in(trial, "restore.sh", &said)?;
        no_values_in(&[other.as_str(), pasted.as_str()], "restore.sh", &said)?;
        ensure!(
            !succeeded
                && said.contains(case.refusal)
                && said.contains("Nothing was changed")
                && !said.contains("does not say FVOCI_SECRET_KEY"),
            "restore.sh with {} did not refuse saying \"{}\": {said}",
            case.what,
            case.refusal
        );
    }
    say(
        "    restore.sh reads a backup's key as compose does (a variable, a $, the environment's, multiline NAME=, a line pasted wrong)",
    );
    Ok(())
}

/// `restore.sh` refuses a backup lacking `FVOCI_SECRET_KEY` (or `env`) unless
/// `--key-not-checked` is given, refuses a `<backup>/made` naming another project unless
/// `--other-project` is given, and refuses a `<backup>/made` naming another file storage.
fn refuses_missing_key_and_mismatched_made(trial: &Trial, directory: &Path) -> Result<()> {
    let env_text = std::fs::read_to_string(&trial.env_file).context("reading the trial's .env")?;
    let without_key: String = env_text
        .lines()
        .filter(|line| !line.starts_with("FVOCI_SECRET_KEY="))
        .collect::<Vec<_>>()
        .join("\n");
    let backup = folder(directory).join("made-and-key");
    let make = |env: Option<&str>, made: Option<&str>| -> Result<()> {
        let _ = std::fs::remove_dir_all(&backup);
        std::fs::create_dir_all(&backup).context("making a backup's folder")?;
        for empty in ["db.dump", "files.tar"] {
            std::fs::write(backup.join(empty), "").context("writing a backup's file")?;
        }
        if let Some(env) = env {
            super::write_private(&backup.join("env"), env)?;
        }
        if let Some(made) = made {
            std::fs::write(backup.join("made"), made).context("writing made")?;
        }
        Ok(())
    };
    let backup_text = backup.to_string_lossy().into_owned();
    let check = |args: &[&str], expected: &str, what: &str| -> Result<()> {
        let (succeeded, said) = script_output(trial.script("restore.sh", args), "restore.sh")?;
        no_secret_in(trial, "restore.sh", &said)?;
        ensure!(
            !succeeded && said.contains(expected) && said.contains("Nothing was changed"),
            "restore.sh ({what}) did not refuse saying \"{expected}\": {said}"
        );
        Ok(())
    };

    // Missing env / empty key: refused without --key-not-checked, passes key check with it.
    make(None, None)?;
    check(&[&backup_text], "--key-not-checked", "missing env")?;
    check(
        &["--key-not-checked", &backup_text],
        "fvoci has a container",
        "missing env with --key-not-checked",
    )?;
    make(Some(&format!("{without_key}\nFVOCI_SECRET_KEY=\n")), None)?;
    check(&[&backup_text], "--key-not-checked", "empty backup key")?;

    // Mismatched project in made: refused without --other-project, passes with it.
    make(
        Some(&env_text),
        Some("project=other-project\nfile_storage=local\n"),
    )?;
    check(&[&backup_text], "--other-project", "mismatched project")?;
    check(
        &["--other-project", &backup_text],
        "fvoci has a container",
        "mismatched project with --other-project",
    )?;

    // Mismatched file_storage in made: always refused.
    make(
        Some(&env_text),
        Some(&format!("project={}\nfile_storage=s3\n", trial.project)),
    )?;
    check(&[&backup_text], "file_storage=s3", "mismatched storage")?;

    let _ = std::fs::remove_dir_all(&backup);
    say(
        "    restore.sh refuses missing keys without --key-not-checked and mismatched project/storage in made",
    );
    Ok(())
}

/// On new volumes, `restore.sh` refuses a files' volume that holds a file; the volumes
/// are taken away again.
fn refuses_a_volume_with_files(trial: &Trial, backup: &Path) -> Result<()> {
    let stray = trial.compose(&[
        "run",
        "--rm",
        "--no-deps",
        "-T",
        "--entrypoint",
        "sh",
        "fvoci",
        "-c",
        "mkdir -p /data/files/stray && echo stray > /data/files/stray/file",
    ])?;
    ensure!(
        stray.succeeded,
        "writing a stray file failed: {}",
        stray.error
    );
    let (succeeded, said) = script_output(
        trial.script("restore.sh", &[&backup.to_string_lossy()]),
        "restore.sh",
    )?;
    no_secret_in(trial, "restore.sh", &said)?;
    ensure!(
        !succeeded
            && said.contains("the files' volume holds files")
            && said.contains("Nothing was changed"),
        "restore.sh did not refuse a volume with a file: {said}"
    );
    take_volumes_away(trial)?;
    say("    restore.sh refuses a files' volume that holds a file");
    Ok(())
}

/// `docker compose down --volumes`, the fake mail provider first.
fn take_volumes_away(trial: &Trial) -> Result<()> {
    super::mail::remove_fake(trial)?;
    let down = trial.compose(&["down", "--volumes", "--timeout", super::STOP_SECONDS])?;
    ensure!(down.succeeded, "docker compose down failed: {}", down.error);
    Ok(())
}

/// `update.sh` that expects another commit fails after fvoci is ready, saying it
/// serves; then one that expects the image's says how long fvoci was down.
fn updates(trial: &Trial, backups: &Path) -> Result<()> {
    let backups = backups.to_string_lossy();
    let (succeeded, said) = script_output(
        trial.script(
            "update.sh",
            &["--no-pull", "--commit", "0000000", "--dir", &backups],
        ),
        "update.sh",
    )?;
    no_secret_in(trial, "update.sh", &said)?;
    ensure!(
        !succeeded && said.contains("is ready and serving, but ping says the commit"),
        "update.sh expecting another commit did not fail saying so: {said}"
    );
    let updated = run_script(trial, "update.sh", &["--no-pull", "--dir", &backups])?;
    let commit = format!("fvoci runs the commit {}", trial.images.revision());
    ensure!(
        updated.contains("fvoci was down") && updated.contains(&commit),
        "update.sh did not say how long fvoci was down and which commit it runs: {updated}"
    );
    say("    update.sh expecting another commit fails, saying fvoci serves");
    for line in updated.lines().filter(|line| line.starts_with("update: ")) {
        say(&format!("    {line}"));
    }
    let tags = kept_tags(trial)?;
    ensure!(
        tags.len() >= 2,
        "two update.sh runs must keep distinct before-update tags (not overwrite the first): {tags:?}"
    );
    say(&format!(
        "    two update.sh runs kept {} distinct before-update tags",
        tags.len()
    ));
    Ok(())
}

/// All `before-update-<project>-*` (and legacy `before-update-<project>`) image tags
/// created by `update.sh` for this trial.
pub(super) fn kept_tags(trial: &Trial) -> Result<Vec<String>> {
    let pattern = format!("fvoci-server:before-update-{}*", trial.project);
    let listed = capture(
        "docker",
        &[
            "image",
            "ls",
            "--format",
            "{{.Repository}}:{{.Tag}}",
            "--filter",
            &format!("reference={pattern}"),
        ],
    )?;
    ensure!(listed.succeeded, "docker image ls failed: {}", listed.error);
    Ok(listed
        .output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// What a person keeps in the installation before the backup: an account, a card whose
/// body the search finds, and a file.
struct Kept {
    password: String,
    workspace: String,
    /// The board, whose folder of the storage holds the file.
    board: String,
    card: String,
    /// The path the file is read back from.
    file_path: String,
    contents: String,
}

impl Kept {
    fn write(trial: &Trial, directory: &Path) -> Result<Self> {
        let client = Client::new(&trial.base, directory)?;
        let password = super::hex(&super::random::<16>()?);
        client.expect(
            "POST",
            "/api/v1/register",
            Some(&json!({
                "username": "keeper",
                "email": "keeper@example.com",
                "password": password,
                "organizationName": "Kept",
            })),
        )?;
        client.expect(
            "POST",
            "/api/v1/login",
            Some(&json!({"username": "keeper", "password": password})),
        )?;
        let workspaces = client.expect("GET", "/api/v1/users/me/workspaces", None)?;
        let workspace = scenario::text(
            workspaces
                .get(0)
                .context("the new account has no workspace")?,
            "id",
        )?;
        let w = format!("/api/v1/w/{workspace}");
        let board = client.expect(
            "POST",
            &format!("{w}/boards"),
            Some(&json!({"visibility": "open", "title": "Kept"})),
        )?;
        let board = scenario::text(&board, "id")?;
        let card = scenario::write_card(&client, &w, &board)?;
        // Text, so that it is read back as it was written.
        let contents = super::hex(&super::random::<1024>()?);
        let upload = directory.join("kept.txt");
        super::write_private(&upload, &contents)?;
        let answer = client.upload(&format!("{w}/containers/{board}/files"), &upload);
        let _ = std::fs::remove_file(&upload);
        let answer = answer?;
        ensure!(
            answer.status == 201,
            "the upload answered {}: {}",
            answer.status,
            answer.body
        );
        let uploaded: serde_json::Value =
            serde_json::from_str(&answer.body).context("the upload's answer")?;
        let file = scenario::text(&uploaded, "fileId")?;
        Ok(Self {
            password,
            file_path: format!("{w}/containers/{board}/files/{file}"),
            workspace: w,
            board,
            card,
            contents,
        })
    }

    /// Signs in anew and finds the card and the file as they were.
    fn check(&self, trial: &Trial, directory: &Path) -> Result<()> {
        let client = Client::new(&trial.base, directory)?;
        client.expect(
            "POST",
            "/api/v1/login",
            Some(&json!({"username": "keeper", "password": self.password})),
        )?;
        let file = client.send("GET", &self.file_path, None)?;
        ensure!(
            file.status == 200 && file.body == self.contents,
            "the file is not read back as it was written ({})",
            file.status
        );
        scenario::find_card(&client, &self.workspace, &self.card)
    }
}

/// Runs a script of `docker/` on the trial with `arguments`; it must succeed, and say
/// no secret of the trial. Returns what it wrote.
fn run_script(trial: &Trial, name: &str, arguments: &[&str]) -> Result<String> {
    let (succeeded, said) = script_output(trial.script(name, arguments), name)?;
    no_secret_in(trial, name, &said)?;
    ensure!(succeeded, "{name} failed:\n{said}");
    Ok(said)
}

/// The last line a script wrote itself (`backup: ...`), not compose's.
fn last_said<'text>(text: &'text str, script: &str) -> &'text str {
    text.lines()
        .rev()
        .find(|line| {
            line.strip_prefix(script)
                .is_some_and(|rest| rest.starts_with(": "))
        })
        .unwrap_or_default()
}

/// The one backup in `backups`.
fn only_backup(backups: &Path) -> Result<PathBuf> {
    let made: Vec<PathBuf> = std::fs::read_dir(backups)
        .with_context(|| format!("reading {}", backups.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<_, _>>()?;
    match &made[..] {
        [backup] => Ok(backup.clone()),
        _ => bail!("backup.sh made {made:?}, not one folder"),
    }
}

/// The backups' folder and each backup only its owner may read, and no file is named
/// after a secret.
fn check_private(backups: &Path, backup: &Path, trial: &Trial) -> Result<()> {
    let mode = |path: &Path| -> Result<u32> {
        Ok(std::fs::metadata(path)
            .with_context(|| format!("reading {}", path.display()))?
            .permissions()
            .mode()
            & 0o777)
    };
    for folder in [backups, backup] {
        ensure!(mode(folder)? == 0o700, "{} is not 700", folder.display());
    }
    let mut names = Vec::new();
    let entries =
        std::fs::read_dir(backup).with_context(|| format!("reading {}", backup.display()))?;
    for entry in entries {
        let path = entry
            .with_context(|| format!("reading {}", backup.display()))?
            .path();
        ensure!(mode(&path)? == 0o600, "{} is not 600", path.display());
        names.push(path.to_string_lossy().into_owned());
    }
    ensure!(
        names.iter().any(|name| name.ends_with("/db.dump"))
            && names.iter().any(|name| name.ends_with("/files.tar")),
        "the backup lacks the database or the files: {names:?}"
    );
    let record = std::fs::read_to_string(backup.join("made")).context("reading made")?;
    ensure!(
        record
            .lines()
            .any(|line| line == format!("project={}", trial.project))
            && record.lines().any(|line| line == "file_storage=local"),
        "backup made lacks project or file_storage=local: {record}"
    );
    no_secret_in(trial, "a backup's name", &names.join(" "))
}

/// `backup.sh` with an `.env` that keeps the files in S3 dumps the database alone, and
/// says the bucket is the operator's.
fn backs_up_no_files_from_s3(trial: &Trial, directory: &Path) -> Result<()> {
    let env = secrets_file(directory, "s3.env")?;
    let text = std::fs::read_to_string(&trial.env_file).context("reading the trial's .env")?;
    super::write_private(&env, &format!("{text}\nFVOCI_FILE_STORAGE=s3\n"))?;
    let backups = folder(directory).join("s3");
    let made = run_script(
        trial,
        "backup.sh",
        &[
            "--env-file",
            &env.to_string_lossy(),
            "--dir",
            &backups.to_string_lossy(),
        ],
    );
    let _ = std::fs::remove_file(&env);
    let made = made?;
    let backup = only_backup(&backups)?;
    let files = backup.join("files.tar").exists();
    let dump = backup.join("db.dump").exists();
    let made_file = std::fs::read_to_string(backup.join("made")).context("reading made")?;
    std::fs::remove_dir_all(&backups).context("removing the backup of S3's settings")?;
    ensure!(
        made_file
            .lines()
            .any(|line| line == format!("project={}", trial.project))
            && made_file.lines().any(|line| line == "file_storage=s3"),
        "S3 backup made lacks project or file_storage=s3: {made_file}"
    );
    ensure!(
        dump && !files && made.contains("they are in S3, whose bucket is yours to back up"),
        "with S3, backup.sh did not dump the database alone, saying so: {made}"
    );
    say("    with the files in S3, the database alone, and the bucket is the operator's");
    Ok(())
}

/// `backup.sh` without `-p` takes the project compose names, also one that compose's
/// YAML quotes (`0123`): of a project that does not run, it says PostgreSQL does not.
fn names_a_quoted_project(trial: &Trial, directory: &Path) -> Result<()> {
    let text = std::fs::read_to_string(&trial.env_file).context("reading the trial's .env")?;
    let env = secrets_file(directory, "quoted-name.env")?;
    super::write_private(&env, &format!("{text}\nCOMPOSE_PROJECT_NAME=0123\n"))?;
    let mut command = Command::new(trial.root.join("docker/backup.sh"));
    command
        .arg("--env-file")
        .arg(&env)
        .arg("--file")
        .arg(trial.root.join("docker/compose.yaml"))
        .arg("--dir")
        .arg(folder(directory).join("quoted-name"));
    trial.scrub(&mut command);
    let outcome = script_output(command, "backup.sh");
    let _ = std::fs::remove_file(&env);
    let (succeeded, said) = outcome?;
    no_secret_in(trial, "backup.sh", &said)?;
    ensure!(
        !succeeded && said.contains("PostgreSQL of the project 0123 does not run"),
        "backup.sh without -p did not take the project 0123 that compose names: {said}"
    );
    say("    without -p, backup.sh takes the project compose names, one its YAML quotes too");
    Ok(())
}

/// `update.sh` with an `.env` that lacks a secret's line names it and stops before
/// anything changes: fvoci is the same container, started when it was.
fn refuses_a_lacking_env(trial: &Trial, directory: &Path) -> Result<()> {
    let before = fvoci_started(trial)?;
    let env = secrets_file(directory, "lacking.env")?;
    let text = std::fs::read_to_string(&trial.env_file).context("reading the trial's .env")?;
    let line = format!("{LACKING}=");
    let lines: Vec<&str> = text
        .lines()
        .filter(|kept| !kept.starts_with(&line))
        .collect();
    super::write_private(&env, &lines.join("\n"))?;
    let refused = script_output(
        trial.script(
            "update.sh",
            &[
                "--no-pull",
                "--env-file",
                &env.to_string_lossy(),
                "--dir",
                &folder(directory).join("main").to_string_lossy(),
            ],
        ),
        "update.sh",
    );
    let _ = std::fs::remove_file(&env);
    let (succeeded, said) = refused?;
    no_secret_in(trial, "update.sh", &said)?;
    ensure!(
        !succeeded
            && said.contains(&format!("lacks {LACKING}"))
            && said.contains("Nothing was changed"),
        "update.sh with an .env lacking {LACKING} did not refuse, naming it: {said}"
    );
    ensure!(
        fvoci_started(trial)? == before,
        "update.sh refused but fvoci was made or started anew"
    );
    say(&format!(
        "    update.sh names the lacking {LACKING} and leaves fvoci running"
    ));
    Ok(())
}

/// `update.sh` while a service of a profile runs (mailpit, of `mail`) and the profile is
/// not given: it refuses, naming the service, before anything changes.
fn refuses_a_profile_off(trial: &Trial, directory: &Path) -> Result<()> {
    let mailpit = Mailpit::start(trial)?;
    let before = fvoci_started(trial)?;
    let refused = script_output(
        trial.script(
            "update.sh",
            &[
                "--no-pull",
                "--dir",
                &folder(directory).join("main").to_string_lossy(),
            ],
        ),
        "update.sh",
    );
    mailpit.remove()?;
    let (succeeded, said) = refused?;
    no_secret_in(trial, "update.sh", &said)?;
    ensure!(
        !succeeded
            && said.contains("mailpit runs, but compose does not see it")
            && said.contains("Nothing was changed"),
        "update.sh with mailpit running and its profile off did not refuse, naming it: {said}"
    );
    ensure!(
        fvoci_started(trial)? == before,
        "update.sh refused but fvoci was made or started anew"
    );
    say("    update.sh refuses while mailpit runs with its profile off, and leaves fvoci running");
    Ok(())
}

/// mailpit of the profile `mail`, started for one check and removed whatever happens
/// after its start began: left running, it would make every later `update.sh` refuse.
struct Mailpit<'a> {
    trial: &'a Trial,
    removed: bool,
}

impl<'a> Mailpit<'a> {
    const PROFILE: [&'static str; 2] = ["--profile", "mail"];

    fn start(trial: &'a Trial) -> Result<Self> {
        // Made first: a start that fails may have made the container.
        let mailpit = Mailpit {
            trial,
            removed: false,
        };
        let up = ["up", "--detach", "--no-deps", "--wait", "mailpit"];
        let started = trial.compose(&[&Self::PROFILE[..], &up].concat())?;
        ensure!(
            started.succeeded,
            "mailpit did not start: {}",
            started.error
        );
        Ok(mailpit)
    }

    /// Removes the container with its volumes, and fails if one of mailpit is left.
    fn remove(mut self) -> Result<()> {
        self.removed = true;
        remove_mailpit(self.trial)
    }
}

impl Drop for Mailpit<'_> {
    fn drop(&mut self) {
        if !self.removed {
            // One that stays is found by the take-down's look for leftovers.
            let _ = remove_mailpit(self.trial);
        }
    }
}

fn remove_mailpit(trial: &Trial) -> Result<()> {
    let rm = ["rm", "--stop", "--force", "--volumes", "mailpit"];
    let removed = trial.compose(&[&Mailpit::PROFILE[..], &rm].concat())?;
    ensure!(
        removed.succeeded,
        "mailpit was not removed: {}",
        removed.error
    );
    let ps = ["ps", "--all", "--quiet", "mailpit"];
    let left = trial.compose(&[&Mailpit::PROFILE[..], &ps].concat())?;
    ensure!(
        left.succeeded && left.output.is_empty(),
        "mailpit's container is left: {} {}",
        left.output,
        left.error
    );
    Ok(())
}

/// The rows of every table, by table.
fn counted_rows(trial: &Trial) -> Result<String> {
    let answer = trial.compose(&[
        "exec",
        "-T",
        "postgres",
        "psql",
        "--username=postgres",
        "--dbname=fvoci",
        "--no-align",
        "--tuples-only",
        "--command",
        COUNT_ROWS,
    ])?;
    ensure!(answer.succeeded, "psql failed: {}", answer.error);
    let counts: BTreeMap<&str, &str> = answer
        .output
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    ensure!(
        counts.contains_key("users"),
        "no table users: {}",
        answer.output
    );
    Ok(counts
        .iter()
        .map(|(table, count)| format!("{table}={count}"))
        .collect::<Vec<_>>()
        .join(" "))
}

/// The files of the volume with their SHA-256, one a line.
fn listed_files(trial: &Trial) -> Result<String> {
    let answer = trial.compose(&["exec", "-T", "fvoci", "sh", "-c", LIST_FILES])?;
    ensure!(
        answer.succeeded,
        "listing the files failed: {}",
        answer.error
    );
    Ok(answer.output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_line_said_is_the_scripts_not_composes() {
        let said = "restore: starting\nrestore: restored x in 2.0 s\n Container a Healthy\n";
        assert_eq!(last_said(said, "restore"), "restore: restored x in 2.0 s");
        assert_eq!(last_said(said, "backup"), "");
        assert_eq!(last_said("restored: no", "restore"), "");
    }
}
