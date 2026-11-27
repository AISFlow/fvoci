//! `cargo xtask check`, in the step "names": the retired names of the workspace.
//!
//! A workspace was once named a team, and a board's groups of people are teams now
//! (`teams`, `team_members`, `container_teams`). What the step refuses is the
//! identifiers that meant a workspace, or a board of one, and are gone: [`RETIRED`] and
//! the kind of an organization of that name however its key and value are written
//! ([`names_the_retired_kind`]), in any case, in the text and the path of every file git
//! tracks or would track, and [`RETIRED_ROUTES`], the routes of the workspaces under
//! that name, in the sources of the crates and of the web app (the tests of the server
//! keep asking for them, to see that nothing answers). Nothing is excepted.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use crate::steps;

/// The retired identifiers, written in halves so that this file passes its own check.
/// Matched in lowercase.
const RETIRED: [&str; 3] = [
    concat!("orgkind::", "team"),
    concat!("board_", "teams"),
    concat!("deleted_", "boards"),
];

/// The retired kind of an organization.
const RETIRED_KIND: &str = "team";

/// Whether `line`, in lowercase, gives a `kind` the value [`RETIRED_KIND`], however the
/// key and the value are written: `kind`, the quote that ends it if it is quoted, `:`
/// or `=` (`==`, `===` too), and the kind in quotes, with any spaces between. JSON
/// (`"kind":"…"`), a script (`kind: '…'`, `kind === "…"`) and SQL (`kind = '…'`) alike.
fn names_the_retired_kind(line: &str) -> bool {
    line.match_indices("kind").any(|(at, key)| {
        let rest = &line[at + key.len()..];
        let rest = rest.strip_prefix(['"', '\'']).unwrap_or(rest).trim_start();
        let after_the_sign = rest.strip_prefix(':').or_else(|| {
            let without = rest.trim_start_matches('=');
            (without.len() < rest.len()).then_some(without)
        });
        let Some(rest) = after_the_sign.map(str::trim_start) else {
            return false;
        };
        let Some(quote) = rest.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
            return false;
        };
        rest[quote.len_utf8()..]
            .strip_prefix(RETIRED_KIND)
            .is_some_and(|after| after.starts_with(quote))
    })
}

/// The routes of the workspaces under their retired name, refused in [`SOURCES`].
const RETIRED_ROUTES: [&str; 2] = [concat!("/api/v1/", "teams"), concat!("/api/v2/", "teams")];

/// Where [`RETIRED_ROUTES`] are refused: the sources of each crate and of the web app.
const SOURCES: [&str; 2] = ["crates/*/src/", "apps/web/src/"];

/// Fails if a file holds a retired name.
///
/// # Errors
///
/// Fails on what it finds, and if git cannot list the files or a file cannot be read.
pub(crate) fn run(root: &Path) -> Result<()> {
    let files = tracked_files(root)?;
    let mut hits = Vec::new();
    for file in &files {
        let text = tracked_text(&root.join(file)).with_context(|| format!("reading {file}"))?;
        hits.extend(check_file(file, &text));
    }
    if hits.is_empty() {
        steps::say(&format!(
            "    {} files without the retired names of the workspace",
            files.len()
        ));
        return Ok(());
    }
    for hit in &hits {
        steps::say(&format!("    {hit}"));
    }
    steps::say("    a workspace is a workspace, a board a container: rename what is named so");
    bail!("{} places have a retired name of the workspace", hits.len())
}

/// The names `file` is held to: [`RETIRED`], and in the sources [`RETIRED_ROUTES`].
fn names_for(file: &str) -> Vec<&'static str> {
    let mut names = RETIRED.to_vec();
    if SOURCES.iter().any(|source| is_under(file, source)) {
        names.extend(RETIRED_ROUTES);
    }
    names
}

/// Whether `file` is under `folder`, a `*` in it standing for one folder.
fn is_under(file: &str, folder: &str) -> bool {
    let mut parts = file.split('/');
    folder.trim_end_matches('/').split('/').all(|wanted| {
        parts
            .next()
            .is_some_and(|part| wanted == "*" || wanted == part)
    }) && parts.next().is_some()
}

/// The hits of one file: `path: in its name`, then `path:line` for each line.
fn check_file(file: &str, text: &str) -> Vec<String> {
    let names = names_for(file);
    let holds = |line: &str| {
        let line = line.to_lowercase();
        names.iter().any(|name| line.contains(name)) || names_the_retired_kind(&line)
    };
    let mut hits = Vec::new();
    if holds(file) {
        hits.push(format!("{file}: in its name"));
    }
    for (index, line) in text.lines().enumerate() {
        if holds(line) {
            hits.push(format!("{file}:{}", index + 1));
        }
    }
    hits
}

/// The files git tracks or would track, by their paths from the root.
///
/// The paths are asked for as they are written, ended by a NUL: listed by lines, git
/// quotes a path that has a letter outside ASCII, and the quoted path names no file.
pub(crate) fn tracked_files(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .args([
            "-c",
            "core.quotePath=false",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .current_dir(root)
        .output()
        .context("running git ls-files")?;
    if !output.status.success() {
        bail!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let listed = String::from_utf8(output.stdout).context("git lists the files as text")?;
    let mut files: Vec<String> = listed
        .split('\0')
        .filter(|file| !file.is_empty())
        .map(str::to_owned)
        .collect();
    files.sort();
    Ok(files)
}

/// The text of a file git tracks, as git holds it: a link is the path it points at (a
/// link to a folder, as the skills' under `.claude/skills/`, has no text of its own),
/// a file its bytes read as text whatever their encoding, and a file deleted from the
/// working tree none, though its name is still checked.
pub(crate) fn tracked_text(path: &Path) -> std::io::Result<String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_symlink() => {
            Ok(std::fs::read_link(path)?.to_string_lossy().into_owned())
        }
        Ok(_) => Ok(String::from_utf8_lossy(&std::fs::read(path)?).into_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::testing;

    #[test]
    fn a_link_is_read_as_the_path_it_points_at_and_a_gone_file_as_nothing() {
        let root = testing::scratch("names-tracked-text");
        testing::write(&root, "skills/a/SKILL.md", "a skill\n");
        std::fs::create_dir_all(root.join("links")).expect("the folder of links is made");
        std::os::unix::fs::symlink("../skills/a", root.join("links/a")).expect("the link is made");

        assert_eq!(
            tracked_text(&root.join("links/a")).expect("a link is read"),
            "../skills/a"
        );
        assert_eq!(
            tracked_text(&root.join("skills/a/SKILL.md")).expect("a file is read"),
            "a skill\n"
        );
        assert_eq!(
            tracked_text(&root.join("gone.md")).expect("a gone file is read"),
            ""
        );
    }

    #[test]
    fn a_retired_name_is_found_in_any_case_in_the_text_and_the_path() {
        for (file, text) in [
            ("crates/model/src/org.rs", concat!("OrgKind::", "Team =>\n")),
            ("apps/web/src/a.ts", concat!("x = { kind: '", "team' };\n")),
            ("contracts/a.json", concat!("{\"kind\": \"", "team\"}\n")),
            (
                "crates/store/schema/sqlite/0001_baseline.sql",
                concat!("CREATE TABLE BOARD_", "TEAMS (\n"),
            ),
            ("docs/a.md", concat!("`deleted_", "boards`\n")),
        ] {
            assert_eq!(check_file(file, text), [format!("{file}:1")], "{text}");
        }
        let named = concat!("crates/server/tests/api/board_", "teams.rs");
        assert_eq!(check_file(named, ""), [format!("{named}: in its name")]);
    }

    #[test]
    fn the_retired_kind_is_found_however_its_key_and_value_are_written() {
        for text in [
            concat!("{\"kind\":\"", "team\"}"),
            concat!("{\"kind\" :  \"", "team\"}"),
            concat!("x = { kind: \"", "team\" };"),
            concat!("UPDATE orgs SET kind = '", "team'"),
            concat!("WHERE org_kind='", "TEAM'"),
            concat!("if (org.kind === '", "team') {"),
        ] {
            assert_eq!(
                check_file("apps/web/src/a.ts", text),
                ["apps/web/src/a.ts:1"],
                "{text}"
            );
        }
        for text in [
            "kind: 'shared'",
            "\"kind\": \"teamwork\"",
            "kind: 'team\"",
            "kind team",
            "the kind of a team",
        ] {
            assert_eq!(
                check_file("apps/web/src/a.ts", text),
                Vec::<String>::new(),
                "{text}"
            );
        }
    }

    #[test]
    fn the_groups_of_people_keep_the_name() {
        for line in [
            "CREATE TABLE teams (id TEXT)",
            "SELECT * FROM team_members",
            "container_teams, deleted_containers",
            "a team of the board",
            "/api/v1/w/{workspace}/teams",
            "kind: 'shared'",
        ] {
            assert_eq!(
                check_file("crates/store/src/teams.rs", line),
                Vec::<String>::new(),
                "{line}"
            );
        }
    }

    #[test]
    fn the_retired_routes_are_refused_in_the_sources_alone() {
        let route = concat!("GET /api/v1/", "teams/1/channels\n");
        for source in [
            "crates/server/src/api/paths.rs",
            "crates/store/src/a.rs",
            "apps/web/src/lib/api/client.ts",
        ] {
            assert_eq!(check_file(source, route), [format!("{source}:1")]);
        }
        assert_eq!(
            check_file("apps/web/src/b.ts", concat!("'/api/v2/", "teams'\n")),
            ["apps/web/src/b.ts:1"]
        );
        for asking in [
            "crates/server/tests/api/paths.rs",
            "crates/server/tests/regression/golden/search.json",
            "apps/web/e2e/a.spec.ts",
            "crates/src/a.rs",
        ] {
            assert_eq!(check_file(asking, route), Vec::<String>::new(), "{asking}");
        }
    }
}
