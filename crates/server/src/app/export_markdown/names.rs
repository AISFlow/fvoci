//! The names of the files and folders of a Markdown export, and the text a file of it
//! begins with: a name any system the archive is opened on takes (no separator, no
//! control or invisible character that would turn the name around, no name Windows
//! keeps, nothing a link would read otherwise, none empty), one of its own in its
//! folder; and the front matter a YAML reader takes back the same.

use std::collections::HashSet;
use std::fmt::Write as _;

/// How many characters a name keeps of a title, its number or extension aside.
const NAME_CHARS: usize = 80;

/// What a name may not hold on Windows, besides control characters; and what a link to
/// it would read as more than a name (`#` its fragment, `%` an escape).
const NOT_IN_NAMES: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|', '#', '%'];

/// The names Windows keeps for its devices, whatever follows the first dot
/// (`CON.notes.md` is one).
const DEVICES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// Whether a character is one of format, invisible: a mark that turns the text
/// (U+202E), a joiner, a byte order mark, a tag; or a separator of lines or paragraphs
/// (U+2028, U+2029), which ends a line where a YAML or a Markdown reader reads one.
pub(super) fn is_format(character: char) -> bool {
    matches!(
        character,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{E0001}'
            | '\u{E0020}'..='\u{E007F}'
    )
}

/// `title` as a name, `fallback` where nothing of it is left: the characters no
/// system takes are `_`, the invisible ones go, spaces and dots at the ends go (Windows
/// drops them), and it is cut to [`NAME_CHARS`].
pub(super) fn name_of(title: &str, fallback: &str) -> String {
    let cleaned: String = title
        .chars()
        // A separator of lines parts words, as a line break does.
        .map(|character| match character {
            '\u{2028}' | '\u{2029}' => ' ',
            character => character,
        })
        .filter(|character| !is_format(*character))
        .map(|character| {
            if character.is_control() || NOT_IN_NAMES.contains(&character) {
                '_'
            } else {
                character
            }
        })
        .take(NAME_CHARS)
        .collect();
    let trimmed =
        cleaned.trim_matches(|character: char| character.is_whitespace() || character == '.');
    if trimmed.is_empty() {
        return fallback.to_owned();
    }
    let device = trimmed.split('.').next().unwrap_or_default().trim_end();
    if DEVICES.contains(&device.to_lowercase().as_str()) {
        return format!("_{trimmed}");
    }
    trimmed.to_owned()
}

/// The names given in one folder: each one of its own, whatever the case of its
/// letters (as a system that does not tell them apart reads them).
#[derive(Debug, Default)]
pub(super) struct Folder {
    taken: HashSet<String>,
}

impl Folder {
    /// A folder in which the names `reserved` are another's already: `files` at the top
    /// of an export, for its files.
    pub(super) fn reserving(reserved: &[&str]) -> Self {
        Self {
            taken: reserved.iter().map(|name| name.to_lowercase()).collect(),
        }
    }

    /// `stem` with `extension` (`.md`, or empty for a folder), made its own in the
    /// folder: ` (2)`, ` (3)` after the stem of one taken.
    pub(super) fn take(&mut self, stem: &str, extension: &str) -> String {
        let stem = self.take_stem(stem, &[extension]);
        format!("{stem}{extension}")
    }

    /// A stem of `stem` that is the folder's own with each of `extensions`, all at
    /// once: a page's file (`.md`) and the folder beside it of what is below it (no
    /// extension), neither of which may be another's (a page `Plan.md` beside a page
    /// `Plan` would have a folder `Plan.md` beside a file of that name).
    pub(super) fn take_stem(&mut self, stem: &str, extensions: &[&str]) -> String {
        let mut count = 1;
        loop {
            let candidate = if count == 1 {
                stem.to_owned()
            } else {
                format!("{stem} ({count})")
            };
            let names: Vec<String> = extensions
                .iter()
                .map(|extension| format!("{candidate}{extension}").to_lowercase())
                .collect();
            if names.iter().all(|name| !self.taken.contains(name)) {
                self.taken.extend(names);
                return candidate;
            }
            count += 1;
        }
    }
}

/// A value of the front matter, in double quotes: what YAML reads back as it was.
pub(super) fn yaml_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if character.is_control() || is_format(character) => {
                let code = u32::from(character);
                if code > 0xFFFF {
                    let _ = write!(out, "\\U{code:08X}");
                } else {
                    let _ = write!(out, "\\u{code:04X}");
                }
            }
            character => out.push(character),
        }
    }
    out.push('"');
    out
}

/// The front matter of a file: `---`, a line each of `fields` whose value is not
/// empty and of `lists` that are not, and `---`.
pub(super) fn front_matter(fields: &[(&str, String)], lists: &[(&str, Vec<String>)]) -> String {
    let mut out = String::from("---\n");
    for (key, value) in fields {
        if !value.is_empty() {
            let _ = writeln!(out, "{key}: {}", yaml_text(value));
        }
    }
    for (key, values) in lists {
        if !values.is_empty() {
            let values: Vec<String> = values.iter().map(|value| yaml_text(value)).collect();
            let _ = writeln!(out, "{key}: [{}]", values.join(", "));
        }
    }
    out.push_str("---\n\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_any_system_takes() {
        assert_eq!(
            name_of("Plan / Q3: draft?", "untitled"),
            "Plan _ Q3_ draft_"
        );
        assert_eq!(name_of("../../etc/passwd", "untitled"), "_.._etc_passwd");
        assert_eq!(name_of("..", "untitled"), "untitled");
        assert_eq!(name_of("  \u{202E}gpj.exe  ", "untitled"), "gpj.exe");
        assert_eq!(name_of("tab\tand\u{0}null", "untitled"), "tab_and_null");
        assert_eq!(name_of("CON", "untitled"), "_CON");
        assert_eq!(name_of("con.notes", "untitled"), "_con.notes");
        assert_eq!(name_of("Nul .tar", "untitled"), "_Nul .tar");
        assert_eq!(name_of("Console.notes", "untitled"), "Console.notes");
        assert_eq!(name_of("one\u{2028}two\u{2029}", "untitled"), "one two");
        assert_eq!(name_of("", "untitled"), "untitled");
        assert_eq!(name_of(&"가".repeat(200), "x").chars().count(), NAME_CHARS);
    }

    #[test]
    fn two_of_one_name_in_a_folder_are_told_apart() {
        let mut folder = Folder::default();
        assert_eq!(folder.take("Plan", ".md"), "Plan.md");
        assert_eq!(folder.take("plan", ".md"), "plan (2).md");
        assert_eq!(folder.take("Plan", ".md"), "Plan (3).md");
        assert_eq!(folder.take("Plan", ""), "Plan");
        // A page's file and its folder are taken together, and neither is another's.
        let mut folder = Folder::reserving(&["files"]);
        assert_eq!(folder.take_stem("Plan", &[".md", ""]), "Plan");
        assert_eq!(folder.take_stem("Plan.md", &[".md", ""]), "Plan.md (2)");
        assert_eq!(folder.take_stem("Files", &[""]), "Files (2)");
        assert_eq!(folder.take("files", ".md"), "files.md");
    }

    #[test]
    fn the_front_matter_reads_back_as_it_was() {
        let fields = [
            ("title", "say \"hi\"\n\\".to_owned()),
            ("icon", String::new()),
        ];
        let lists = [
            ("attachments", vec!["../files/a.pdf".to_owned()]),
            ("none", vec![]),
        ];
        assert_eq!(
            front_matter(&fields, &lists),
            "---\ntitle: \"say \\\"hi\\\"\\n\\\\\"\nattachments: [\"../files/a.pdf\"]\n---\n\n"
        );
        assert_eq!(yaml_text("a\u{202E}b"), "\"a\\u202Eb\"");
        assert_eq!(yaml_text("a\u{2028}b"), "\"a\\u2028b\"");
        assert_eq!(yaml_text("\u{E0001}"), "\"\\U000E0001\"");
        assert_eq!(name_of("Q#3 at 100%", "x"), "Q_3 at 100_");
    }
}
