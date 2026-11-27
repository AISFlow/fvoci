//! Lines of an `.env` that compose refuses or warns of, quoting what was pasted (a key's
//! lines, a quote never closed, a `$` read as a variable): `backup.sh`, `update.sh` and
//! `restore.sh` stop before anything changes, and say what compose said without a word
//! of what was pasted. The table is compose's messages as the trial's compose writes
//! them. And values of many lines, which compose takes, whose lines read like fvoci's
//! settings: the scripts read past them.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, ensure};

use super::{Trial, fvoci_started, no_secret_in, no_values_in, script_output, secrets_file};
use crate::steps::say;

/// One line pasted wrong: what it is, the line made of three words of a secret, and
/// what the scripts show of compose's complaint.
struct Pasted {
    what: &'static str,
    line: fn(&[String; 3]) -> String,
    shown: &'static str,
}

/// The lines, each after the trial's `.env`. compose quotes the words of each but the
/// space's (it names the line) and the image's (it takes it, and the scripts refuse a
/// name that is not one), and the port's and the address's (lines it does not know are
/// counted, not shown).
const TABLE: &[Pasted] = &[
    Pasted {
        what: "a key's line",
        line: |w| format!("MIIEvQ+{}=", w[0]),
        shown: ": unexpected character <hidden> in variable name <hidden>",
    },
    Pasted {
        what: "a key's line in quotes",
        line: |w| format!("\"{}\"=x", w[0]),
        shown: ": unexpected character <hidden> in variable name <hidden>",
    },
    Pasted {
        what: "a second quoted value",
        line: |w| format!("FVOCI_PASTED=\"x\" \"{}\"", w[0]),
        shown: ": unexpected character <hidden> in variable name <hidden>",
    },
    Pasted {
        what: "a key's line ended by CR LF",
        line: |w| format!("MIIEvQ+{}=\r", w[0]),
        shown: ": unexpected character <hidden> in variable name <hidden>",
    },
    Pasted {
        what: "a line after a byte order mark",
        line: |w| format!("\u{feff}FVOCI_PASTED={}", w[0]),
        shown: ": unexpected character <hidden> in variable name <hidden>",
    },
    Pasted {
        what: "a space in a key's line",
        line: |w| format!("{} {}=x", w[0], w[1]),
        shown: ": key cannot contain a space",
    },
    Pasted {
        what: "a double quote never closed",
        line: |w| format!("FVOCI_PASTED=\"{}", w[0]),
        shown: ": unterminated quoted value <hidden>",
    },
    Pasted {
        what: "a double quote never closed, a quote escaped inside",
        line: |w| format!("FVOCI_PASTED=\"{}\\\"{}", w[0], w[1]),
        shown: ": unterminated quoted value <hidden>",
    },
    Pasted {
        what: "a single quote never closed, a quote escaped inside",
        line: |w| format!("FVOCI_PASTED='{}\\'{}", w[0], w[1]),
        shown: ": unterminated quoted value <hidden>",
    },
    Pasted {
        what: "single quotes inside a double quote never closed",
        line: |w| format!("FVOCI_PASTED=\"{}'{}'{}", w[0], w[1], w[2]),
        shown: ": unterminated quoted value <hidden>",
    },
    Pasted {
        what: "double quotes inside a single quote never closed",
        line: |w| format!("FVOCI_PASTED='{}\"{}\"{}", w[0], w[1], w[2]),
        shown: ": unterminated quoted value <hidden>",
    },
    Pasted {
        what: "a ${ never closed",
        line: |w| format!("FVOCI_PASTED={}${{{}", w[0], w[1]),
        shown: "compose: Invalid template: <hidden>",
    },
    Pasted {
        what: "a ${:?} whose variable and message are pasted",
        line: |w| format!("FVOCI_PASTED=${{{}:?{} {}}}", w[0], w[1], w[2]),
        shown: "compose: required variable <hidden> is missing a value",
    },
    Pasted {
        what: "a $ read as a variable",
        line: |w| format!("FVOCI_PASTED={}${}", w[0], w[1]),
        shown: "compose: warning: The <hidden> variable is not set.",
    },
    Pasted {
        what: "a ${} read as a variable in double quotes",
        line: |w| format!("FVOCI_PASTED=\"{}${{{}}}{}\"", w[0], w[1], w[2]),
        shown: "compose: warning: The <hidden> variable is not set.",
    },
    Pasted {
        what: "a word pasted beside an image's name",
        line: |w| format!("FVOCI_IMAGE={} ghcr.io/fvoci/fvoci-server:v1", w[0]),
        shown: ": an image named by the files or .env",
    },
    Pasted {
        what: "a word pasted after a long image name, which compose folds",
        line: |w| {
            format!(
                "FVOCI_IMAGE=ghcr.io/fvoci/fvoci-server@sha256:{} {}",
                "0".repeat(64),
                w[0]
            )
        },
        shown: ": an image named by the files or .env",
    },
    Pasted {
        what: "a long word pasted before an image's name, which compose folds",
        line: |w| {
            format!(
                "FVOCI_IMAGE={0}{1}{2}{0}{1}{2} ghcr.io/fvoci/fvoci-server:v1",
                w[0], w[1], w[2]
            )
        },
        shown: ": an image named by the files or .env",
    },
    Pasted {
        what: "a port pasted",
        line: |w| format!("FVOCI_HOST_PORT={}", w[0]),
        shown: "compose: 1 line(s) hidden: they may quote a value of the .env",
    },
    Pasted {
        what: "an address pasted",
        line: |w| format!("FVOCI_BIND={}", w[0]),
        shown: "compose: 1 line(s) hidden: they may quote a value of the .env",
    },
    Pasted {
        what: "a hidden error line whose value says variable is not set",
        line: |w| format!("FVOCI_HOST_PORT={} variable is not set", w[0]),
        shown: "compose: 1 line(s) hidden: they may quote a value of the .env",
    },
];

/// A value of many lines, which compose takes and writes below its key (`KEY: |-`, or
/// on one line in double quotes when it holds a CR). Its lines read like fvoci's
/// settings and come before them (`E` before `F`, `environment` before `image`); its
/// image is no name, which a check of every `image:` line would refuse.
struct ManyLines {
    what: &'static str,
    line: fn(&[String; 3]) -> String,
}

const MANY_LINES: &[ManyLines] = &[
    ManyLines {
        what: "a value of lines written \\n in double quotes",
        line: |w| {
            format!(
                "FVOCI_EMAIL_VERIFICATION=\"{}\\nFVOCI_FILE_STORAGE: s3\\nimage: {}!\\n{}\"",
                w[0], w[1], w[2]
            )
        },
    },
    ManyLines {
        what: "a value of lines in double quotes",
        line: |w| {
            format!(
                "FVOCI_EMAIL_VERIFICATION=\"{}\nFVOCI_FILE_STORAGE: s3\nimage: {}!\n{}\"",
                w[0], w[1], w[2]
            )
        },
    },
    ManyLines {
        what: "a value of lines in single quotes",
        line: |w| {
            format!(
                "FVOCI_EMAIL_VERIFICATION='{}\nFVOCI_FILE_STORAGE: s3\nimage: {}!\n{}'",
                w[0], w[1], w[2]
            )
        },
    },
    ManyLines {
        what: "a value of lines ended by CR LF",
        line: |w| {
            format!(
                "FVOCI_EMAIL_VERIFICATION=\"{}\r\nFVOCI_FILE_STORAGE: s3\r\nimage: {}!\r\n{}\"\r",
                w[0], w[1], w[2]
            )
        },
    },
];

/// Gives each script an `.env` with each line of [`TABLE`] in turn, then each of
/// [`MANY_LINES`].
///
/// # Errors
///
/// Fails at the first script that does not stop, shows a word of what was pasted or
/// shows compose's complaint otherwise than the table says, or reads a line of a value
/// as fvoci's setting, or if fvoci was made or started anew.
pub(super) fn refused_hidden(trial: &Trial, directory: &Path) -> Result<()> {
    let before = fvoci_started(trial)?;
    let text = std::fs::read_to_string(&trial.env_file).context("reading the trial's .env")?;
    let env = secrets_file(directory, "pasted-wrong.env")?;
    let backups = super::backup::folder(directory).join("pasted-wrong");
    let outcome = TABLE
        .iter()
        .try_for_each(|pasted| try_line(trial, &text, &env, &backups, pasted))
        .and_then(|()| read_past(trial, &text, &env, directory));
    let _ = std::fs::remove_file(&env);
    outcome?;
    ensure!(
        !backups.exists(),
        "a script made {} from an .env it refused",
        backups.display()
    );
    ensure!(
        fvoci_started(trial)? == before,
        "a script refused an .env but fvoci was made or started anew"
    );
    say(&format!(
        "    {} lines pasted wrong, each in backup.sh, update.sh and restore.sh: refused, hidden",
        TABLE.len()
    ));
    say(&format!(
        "    {} values of many lines that read like fvoci's settings: read past (fvoci's image, its files' storage)",
        MANY_LINES.len()
    ));
    Ok(())
}

/// Each of [`MANY_LINES`] after the trial's `.env`: `ops.sh` reads fvoci's image and
/// storage as without it, and `restore.sh`, given a backup without the files' archive,
/// says the installation keeps its files in its volume, with no word of the value.
fn read_past(trial: &Trial, text: &str, env: &Path, directory: &Path) -> Result<()> {
    let own = format!("{} local", trial.images.server.local);
    let read = image_and_storage(trial, &trial.env_file)?;
    ensure!(
        read == own,
        "ops.sh read the trial's own image and storage as \"{read}\", not \"{own}\""
    );
    let backup = super::backup::folder(directory).join("lines-backup");
    std::fs::create_dir_all(&backup).context("making a backup without files")?;
    super::write_private(&backup.join("db.dump"), "")?;
    let backup_text = backup.to_string_lossy();
    let env_text = env.to_string_lossy();
    for value in MANY_LINES {
        let words = [word()?, word()?, word()?];
        super::write_private(env, &format!("{text}\n{}\n", (value.line)(&words)))?;
        let read = image_and_storage(trial, env)?;
        ensure!(
            read == own,
            "ops.sh read fvoci's image and storage in {} as \"{}\", not \"{own}\"",
            value.what,
            hidden(&words, read)
        );
        let arguments = ["--env-file", env_text.as_ref(), backup_text.as_ref()];
        let (succeeded, said) =
            script_output(trial.script("restore.sh", &arguments), "restore.sh")?;
        no_secret_in(trial, "restore.sh", &said)?;
        let word_refs: Vec<&str> = words.iter().map(String::as_str).collect();
        no_values_in(&word_refs, "restore.sh", &said)?;
        ensure!(
            !succeeded && said.contains("while this installation keeps them in its volume"),
            "restore.sh with {} in the .env did not say the files are in the volume, without a word of it: {}",
            value.what,
            hidden(&words, said)
        );
    }
    let _ = std::fs::remove_dir_all(&backup);
    Ok(())
}

/// fvoci's image and its files' storage, as `ops.sh` reads them from compose's files
/// with `env`: `<image> <local or s3>`.
fn image_and_storage(trial: &Trial, env: &Path) -> Result<String> {
    let mut command = Command::new("sh");
    trial.scrub(&mut command);
    command
        .args([
            "-c",
            ". \"$0\"; script=ops.sh; env_file=$1
            installation=$(compose config --format yaml 2>/dev/null) || exit 1
            check_image_names
            printf '%s %s' \"$fvoci_image\" \"$(file_storage)\"",
        ])
        .arg(trial.root.join("docker/ops.sh"))
        .arg(env)
        .env("COMPOSE_FILE", trial.root.join("docker/compose.yaml"))
        .env("COMPOSE_PROJECT_NAME", &trial.project);
    let (succeeded, said) = script_output(command, "ops.sh")?;
    no_secret_in(trial, "ops.sh", &said)?;
    ensure!(
        succeeded,
        "ops.sh refused compose's files and the .env, or an image's name, with a value of many lines"
    );
    Ok(said)
}

/// Runs the three scripts with the trial's `.env` and `pasted`'s line after it.
fn try_line(trial: &Trial, text: &str, env: &Path, backups: &Path, pasted: &Pasted) -> Result<()> {
    let words = [word()?, word()?, word()?];
    let line = (pasted.line)(&words);
    super::write_private(env, &format!("{text}\n{line}\n"))?;
    let env_text = env.to_string_lossy();
    let backups_text = backups.to_string_lossy();
    // Never made: restore.sh reads compose's files before it looks at the folder.
    let never_made = backups.join("a-backup");
    let never_made_text = never_made.to_string_lossy();
    let runs: [(&str, &[&str]); 3] = [
        ("backup.sh", &["--dir", &backups_text]),
        ("update.sh", &["--no-pull", "--dir", &backups_text]),
        ("restore.sh", &[&never_made_text]),
    ];
    for (name, own) in runs {
        let arguments = [&["--env-file", env_text.as_ref()][..], own].concat();
        let (succeeded, said) = script_output(trial.script(name, &arguments), name)?;
        no_secret_in(trial, name, &said)?;
        let word_refs: Vec<&str> = words.iter().map(String::as_str).collect();
        no_values_in(&word_refs, name, &said)?;
        let expects_dollar_advice = pasted
            .shown
            .contains("warning: The <hidden> variable is not set");
        let gave_dollar_advice = said.contains("a $ in a value of the .env starts a variable");
        ensure!(
            !succeeded
                && said.contains(pasted.shown)
                && said.contains("Nothing was changed")
                && gave_dollar_advice == expects_dollar_advice,
            "{name} with {} in the .env did not stop showing \"{}\" (dollar_advice={gave_dollar_advice}, expected={expects_dollar_advice}): {}",
            pasted.what,
            pasted.shown,
            hidden(&words, said)
        );
    }
    Ok(())
}

/// `said` with each of `words` put as `<pasted>`, for a message of the trial.
fn hidden(words: &[String; 3], said: String) -> String {
    words
        .iter()
        .fold(said, |said, word| said.replace(word.as_str(), "<pasted>"))
}

/// A word of a secret, which a `$` can name: a letter, then random hexadecimal digits.
fn word() -> Result<String> {
    Ok(format!("S{}", super::hex(&super::random::<8>()?)))
}
