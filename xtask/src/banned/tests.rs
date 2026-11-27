use std::fmt::Write as _;
use std::process::Command;

use super::*;
use crate::files::testing;

/// Runs git in `root` and fails the test if git does.
fn git(root: &Path, arguments: &[&str]) {
    let status = Command::new("git")
        .args(arguments)
        .current_dir(root)
        .status()
        .expect("git runs");
    assert!(status.success(), "git {arguments:?} succeeds");
}

/// The list that bans the given words, written as [`BANNED`] is.
fn banning(banned: &[&str]) -> Words {
    listing(banned.iter().map(|word| (*word).to_owned()))
}

/// The list of the given made-up private names, folded and written as
/// [`PRIVATE_NAMES`] is.
fn naming(private: &[&str]) -> Words {
    listing(private.iter().map(|name| folded_name(name)))
}

/// A list of `entries` written as [`BANNED`] is: each one's characters, and digest.
fn listing(entries: impl Iterator<Item = String>) -> Words {
    let listed: Vec<(usize, String)> = entries
        .map(|entry| {
            let mut hexadecimal = String::new();
            for byte in Sha256::digest(entry.as_bytes()) {
                write!(hexadecimal, "{byte:02x}").expect("a string takes what is written");
            }
            (entry.chars().count(), hexadecimal)
        })
        .collect();
    let listed: Vec<(usize, &str)> = listed
        .iter()
        .map(|(letters, hexadecimal)| (*letters, hexadecimal.as_str()))
        .collect();
    Words::parse(&listed).expect("the list is read")
}

#[test]
fn a_word_is_found_in_any_case_and_inside_a_longer_word() {
    let words = banning(&["gadget", "가나다라"]);
    let nothing = naming(&[]);
    let mut reading = Reading::new(&words, &nothing);
    for text in [
        "see the gadget docs",
        "see the GADGET docs",
        "the Gadgets of old",
        "old_gadget_port = 1",
        "megaGadgetry",
        "https://gadget.example/docs",
        "가나다라의 문서",
        "옛가나다라",
    ] {
        assert!(reading.finds_in(text), "{text:?} holds a banned word");
    }
}

#[test]
fn a_plain_word_and_a_word_cut_apart_are_not_found() {
    let words = banning(&["gadget", "가나다라"]);
    let nothing = naming(&[]);
    let mut reading = Reading::new(&words, &nothing);
    for text in [
        "",
        "the original file name",
        "a gadge, and a t",
        "gad-get",
        "gadge7",
        "가나다 라",
        "다라가나",
    ] {
        assert!(!reading.finds_in(text), "{text:?} holds no banned word");
    }
}

#[test]
fn a_file_is_reported_by_its_name_and_by_its_lines() {
    let words = banning(&["gadget"]);
    let nothing = naming(&[]);
    let mut reading = Reading::new(&words, &nothing);
    assert_eq!(
        check_file(
            "docs/GADGET-notes.md",
            "our notes\nonce a Gadget\n\nand gadgets\nand gadgets again\n",
            &mut reading,
            &[]
        ),
        [
            "docs/GADGET-notes.md: in its name",
            "docs/GADGET-notes.md:2",
            "docs/GADGET-notes.md:4",
            "docs/GADGET-notes.md:5"
        ]
    );
    assert_eq!(
        check_file("docs/notes.md", "our notes\n", &mut reading, &[]),
        Vec::<String>::new()
    );
}

#[test]
fn every_file_git_lists_is_read_whatever_its_name_and_a_gone_one_by_its_name() {
    let words = banning(&["gadget", "가나다라"]);
    let root = testing::scratch("banned-find");
    testing::write(&root, "plain.md", "our notes\n");
    testing::write(&root, "notes/held.md", "our notes\nonce a Gadget\n");
    testing::write(&root, "기록/옛가나다라.md", "our notes\n");
    testing::write(&root, "기록/글.md", "첫 줄\n\n가나다라의 글\n");
    testing::write(&root, "gone-gadget.md", "our notes\n");
    std::os::unix::fs::symlink("notes/gadgets", root.join("link")).expect("the link is made");
    git(&root, &["init", "--quiet"]);
    git(&root, &["add", "gone-gadget.md"]);
    std::fs::remove_file(root.join("gone-gadget.md")).expect("the file is removed");

    let found = find(&root, &words, &naming(&[]), &[]).expect("the files are read");

    assert_eq!(found.files, 6);
    assert_eq!(
        found.hits,
        [
            "gone-gadget.md: in its name",
            "link:1",
            "notes/held.md:2",
            "기록/글.md:3",
            "기록/옛가나다라.md: in its name",
        ]
    );
}

#[test]
fn the_list_of_this_workspace_is_read_and_a_wrong_one_is_refused() {
    let words = Words::parse(&BANNED).expect("the list is read");
    assert_eq!(
        words.by_letters.values().map(Vec::len).sum::<usize>(),
        BANNED.len()
    );
    assert!(Words::parse(&[(0, BANNED[0].1)]).is_err(), "no letters");
    assert!(Words::parse(&[(6, "7267391c")]).is_err(), "a short digest");
    let not_hexadecimal = "z".repeat(64);
    assert!(
        Words::parse(&[(6, not_hexadecimal.as_str())]).is_err(),
        "digits that are not hexadecimal"
    );
}

#[test]
fn the_private_names_of_this_workspace_are_read() {
    let names = Words::parse(&PRIVATE_NAMES).expect("the list is read");
    assert_eq!(
        names.by_letters.values().map(Vec::len).sum::<usize>(),
        PRIVATE_NAMES.len()
    );
}

#[test]
fn a_private_name_is_found_however_its_parts_are_joined() {
    let words = banning(&[]);
    let names = naming(&[concat!("secret-", "repo"), concat!("/hidden", "-desk/")]);
    let mut reading = Reading::new(&words, &names);
    for text in [
        concat!("see secret-", "repo for that"),
        concat!("SECRET_", "REPO"),
        concat!("Secret", "Repo"),
        concat!("git clone https://example.com/org/secret-", "repo.git"),
        concat!("~/hidden-", "desk/secret-", "repo-carry/notes"),
        concat!("cd $HOME/hidden", "desk/other"),
        concat!("/home/someone/hidden_", "desk/a"),
    ] {
        assert!(reading.finds_name_in(text), "{text:?} holds a private name");
    }
    for text in [
        "a secret, the repo",
        "secret repo",
        "secrets",
        "repository",
        concat!("hidden", "desk"),
        concat!("/hidden", "desks/"),
    ] {
        assert!(
            !reading.finds_name_in(text),
            "{text:?} holds no private name"
        );
    }
    assert_eq!(
        check_file(
            concat!("docs/secret-", "repo/notes.md"),
            concat!("one\nnotes of Secret", "Repo\n"),
            &mut reading,
            &[]
        ),
        [
            concat!("docs/secret-", "repo/notes.md: in its name"),
            concat!("docs/secret-", "repo/notes.md:2"),
        ]
    );
    assert_eq!(
        check_file(
            "vendor/rhwp-studio/upstream/a.md",
            concat!("secret-", "repo\n"),
            &mut reading,
            &BANNED_PHRASES
        ),
        ["vendor/rhwp-studio/upstream/a.md:1"],
        "a private name is refused in vendored sources too"
    );
}

#[test]
fn banned_phrases_match_case_insensitively_without_false_positives() {
    for phrase in BANNED_PHRASES {
        assert_eq!(
            phrase,
            phrase.to_lowercase(),
            "banned phrase {phrase:?} must be lowercase"
        );
    }

    for text in [
        concat!("Ports ", "server/api/files.rs"),
        concat!("PORTS ", "SERVER/app/boards.rs"),
        concat!("matches the ", "original's response"),
        concat!("verified against The ", "Original Server"),
        concat!("The formats of ", "the original archive"),
        concat!("기존 원본", "의 동작"),
        concat!("원본", "과 같다"),
        concat!("원본", "과의 비교"),
        concat!("원본", "에서 옮김"),
        concat!("ordered As the ", "Original does"),
        concat!("as the ", "original did"),
        concat!("what the ", "original app showed"),
        concat!("as in the ", "original tree"),
        concat!("the cases of the ", "original `a.test.ts`"),
        // Refused although a copy could say it of its source: it says `the source`.
        concat!("with the same members as the ", "original"),
        concat!("a copy of the ", "original `Block`"),
    ] {
        assert!(
            finds_phrase(text, &BANNED_PHRASES),
            "{text:?} must match a banned phrase"
        );
    }

    for text in [
        "original file",
        "the original file name",
        "original card",
        "the original board exists",
        "the original card stays where it was",
        "copies of the original file",
        "originally",
        "원본 이미지",
        "원본 파일",
        "원본 데이터",
        "the same members as the source",
        "a copy of the source `Block`",
    ] {
        assert!(
            !finds_phrase(text, &BANNED_PHRASES),
            "{text:?} must not match any banned phrase"
        );
    }
}

#[test]
fn a_phrase_cut_by_a_line_break_is_found_on_the_line_it_begins_on() {
    let words = banning(&[]);
    let nothing = naming(&[]);
    let mut reading = Reading::new(&words, &nothing);
    for (file, text) in [
        (
            "crates/a.rs",
            concat!(
                "// the name of the\n// ",
                "original server's configuration\n"
            ),
        ),
        (
            "crates/a.rs",
            concat!("    /// kept as\n    /// the ", "original kept it\n"),
        ),
        (
            "crates/a.rs",
            concat!("//! read as the\n//! ", "original does\n"),
        ),
        (
            "apps/web/a.ts",
            concat!(" * the cases of the\n * ", "original `x.test.ts`\n */\n"),
        ),
        (
            "a.yaml",
            concat!("# laid out as in the\n# ", "original tree\n"),
        ),
        ("a.sql", concat!("-- the formats of\n-- ", "the original\n")),
        ("a.md", concat!("what the\n", "original app showed\n")),
    ] {
        let found = check_file(file, text, &mut reading, &BANNED_PHRASES);
        assert_eq!(found, [format!("{file}:1")], "{text:?}");
    }
    // A phrase within one line is its line's, once; the line before it is clean.
    assert_eq!(
        check_file(
            "crates/a.rs",
            concat!("// first\n// as the ", "original did\n// last\n"),
            &mut reading,
            &BANNED_PHRASES
        ),
        ["crates/a.rs:2"]
    );
    // What two lines say together is not banned unless a phrase is.
    assert_eq!(
        check_file(
            "crates/a.rs",
            "// the copy keeps\n// the original board exists\n// as it was\n",
            &mut reading,
            &BANNED_PHRASES
        ),
        Vec::<String>::new()
    );
    // Two phrases that overlap begin on their own lines.
    assert_eq!(
        check_file(
            "crates/a.rs",
            concat!("// kept as\n// the\n// ", "original did\n"),
            &mut reading,
            &BANNED_PHRASES
        ),
        ["crates/a.rs:1", "crates/a.rs:2"]
    );
    // Nor across a line that says nothing, which ends what was said.
    assert_eq!(
        check_file(
            "crates/a.rs",
            concat!("// kept as\n//\n// the ", "original kept it\n"),
            &mut reading,
            &BANNED_PHRASES
        ),
        Vec::<String>::new()
    );
    // Four lines are more than are joined.
    assert_eq!(
        check_file(
            "crates/a.rs",
            concat!("// formats\n// of\n// the\n// ", "original\n"),
            &mut reading,
            &BANNED_PHRASES
        ),
        Vec::<String>::new()
    );
    assert_eq!(said("   /// text  "), "text");
    assert_eq!(said(" * text"), "text");
    assert_eq!(said("//! text"), "text");
    assert_eq!(said("-- text"), "text");
    assert_eq!(said("> > * text"), "text");
    assert_eq!(said("// a \t  b \\"), "a b");
    assert_eq!(said("plain text"), "plain text");
    assert_eq!(
        joined(&[said("// one"), said("  # two ")]).as_deref(),
        Some("one two")
    );
    assert_eq!(joined(&[said("// one"), said("//")]), None);
}

#[test]
fn a_phrase_is_found_through_marks_blanks_and_three_lines() {
    let words = banning(&[]);
    let nothing = naming(&[]);
    let mut reading = Reading::new(&words, &nothing);
    for (file, text) in [
        // Quotations, stacked marks and the comments of other languages.
        ("a.md", concat!("> kept as the\n> ", "original did\n")),
        ("a.md", concat!("> * kept as the\n> * ", "original did\n")),
        (
            "a.rs",
            concat!("// * kept as\n// * the ", "original kept it\n"),
        ),
        ("a.el", concat!(";; kept as the\n;; ", "original did\n")),
        ("a.tex", concat!("% kept as the\n% ", "original did\n")),
        // Blanks of any kind and number, and a line carried on to the next.
        ("a.rs", concat!("// kept as  the\t", "original did\n")),
        (
            "a.sh",
            concat!("echo \"kept as the \\\n  ", "original did\"\n"),
        ),
        // Three lines.
        (
            "a.rs",
            concat!("// kept as\n// the\n// ", "original kept it\n"),
        ),
    ] {
        let found = check_file(file, text, &mut reading, &BANNED_PHRASES);
        assert_eq!(found, [format!("{file}:1")], "{text:?}");
    }
}

#[test]
fn a_phrase_in_a_path_is_found() {
    let words = banning(&[]);
    let nothing = naming(&[]);
    let mut reading = Reading::new(&words, &nothing);
    assert_eq!(
        check_file(
            concat!("crates/http.serve", "content.rs"),
            "clean\n",
            &mut reading,
            &BANNED_PHRASES
        ),
        [concat!("crates/http.serve", "content.rs: in its name")]
    );
}

#[test]
fn vendored_sources_skip_phrases_but_still_enforce_banned_words() {
    let words = banning(&["gadget"]);
    let nothing = naming(&[]);
    let mut reading = Reading::new(&words, &nothing);
    let phrase_line = concat!("-- the ", "original's board block\n");
    let word_line = "-- uses gadget here\n";

    assert_eq!(
        check_file(
            "crates/store/schema/sqlite/0001_baseline.sql",
            phrase_line,
            &mut reading,
            &BANNED_PHRASES
        ),
        ["crates/store/schema/sqlite/0001_baseline.sql:1"]
    );
    assert_eq!(
        check_file(
            "vendor/rhwp-studio/upstream/src/example.ts",
            concat!("// 원본", "과 비교\n"),
            &mut reading,
            &BANNED_PHRASES
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        check_file(
            "vendor/rhwp-studio/import.mjs",
            concat!("// 원본", "과 비교\n"),
            &mut reading,
            &BANNED_PHRASES
        ),
        ["vendor/rhwp-studio/import.mjs:1"],
        "what is written here beside the vendored sources is held to the phrases"
    );
    assert_eq!(
        check_file(
            "vendor/rhwp-studio/upstream/src/example.ts",
            word_line,
            &mut reading,
            &BANNED_PHRASES
        ),
        ["vendor/rhwp-studio/upstream/src/example.ts:1"]
    );
    assert_eq!(
        check_file("docs/notes.md", phrase_line, &mut reading, &BANNED_PHRASES),
        ["docs/notes.md:1"]
    );
}

#[test]
fn retired_tokens_are_found_in_any_case_inside_names_and_in_paths() {
    let words = banning(&[]);
    let nothing = naming(&[]);
    let mut reading = Reading::new(&words, &nothing);
    for hit_line in [
        concat!("pub ", "create", "_at: i64,"),
        concat!("SELECT b.", "update", "_at FROM blocks b"),
        concat!(r#"{"id":"1",""#, "create", "At", r#"":10}"#),
        concat!("card.", "update", "At"),
        concat!("CREATE INDEX blocks_parent_type_", "create", "_at_idx"),
        concat!("const old", "Create", "At = 1;"),
        concat!("ALTER TABLE b DROP COLUMN ", "CREATE", "_AT"),
        concat!("let Preferences = ", "UserPreferences", "Patch::default();"),
    ] {
        assert!(
            finds_token(hit_line, &RETIRED_TOKENS),
            "{hit_line:?} must match a retired token"
        );
        for file in [
            "crates/model/src/lib.rs",
            "crates/store/schema/sqlite/0001_baseline.sql",
        ] {
            assert_eq!(
                check_file(file, hit_line, &mut reading, &[]),
                [format!("{file}:1")]
            );
        }
        assert_eq!(
            check_file(
                "vendor/rhwp-studio/upstream/lib/a.ts",
                hit_line,
                &mut reading,
                &[]
            ),
            Vec::<String>::new(),
            "vendored sources are not ours to rename"
        );
        assert_eq!(
            check_file("vendor/rhwp-studio/import.mjs", hit_line, &mut reading, &[]),
            ["vendor/rhwp-studio/import.mjs:1"]
        );
    }
    let named = concat!("crates/store/src/", "create", "_at.rs");
    assert_eq!(
        check_file(named, "", &mut reading, &[]),
        [format!("{named}: in its name")]
    );

    for clean_line in [
        "pub created_at: i64,",
        "pub updated_at: i64,",
        "CREATED_AT and UPDATED_AT",
        r#"{"createdAt":10,"updatedAt":20}"#,
        "backend.createAttachment(boardId, cardId, file)",
        "editor.commands.updateAttributes('codeBlock', {})",
        "const NO_CREATED_AT: &str = \"missing\";",
        "blocks_parent_type_created_at_idx",
    ] {
        assert!(
            !finds_token(clean_line, &RETIRED_TOKENS),
            "{clean_line:?} must not match any retired token"
        );
    }
}
