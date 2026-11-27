//! What a client may name a file it sends: one rule for `uploadFile`, `createUpload`
//! and the names an archive gives the files of a board.

use std::fmt;

use super::uploads::MAX_UPLOAD_NAME_CHARS;

/// Why a name a client gives is not taken as the name of a file. It reads as what
/// follows "the name" in a sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnfitName {
    /// It is longer than [`MAX_UPLOAD_NAME_CHARS`] characters.
    TooLong,
    /// It has U+0000 in it, which a record cannot keep.
    Nul,
    /// It has a control character or Unicode bidirectional/invisible formatting character.
    ControlOrBidi,
    /// Nothing but blanks follows its last `/` or `\`: it names a directory, or nothing.
    NoFile,
    /// What follows its last `/` or `\` is `.` or `..`: a directory.
    Dots,
}

impl fmt::Display for UnfitName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong => write!(f, "is longer than {MAX_UPLOAD_NAME_CHARS} characters"),
            Self::Nul => f.write_str("has U+0000 in it"),
            Self::ControlOrBidi => {
                f.write_str("has a control or bidirectional formatting character in it")
            }
            Self::NoFile => f.write_str(r"is blank after its last `/` or `\`"),
            Self::Dots => f.write_str(r"is `.` or `..` after its last `/` or `\`"),
        }
    }
}

impl std::error::Error for UnfitName {}

/// Whether `ch` is a control character, line/paragraph separator, zero-width formatting
/// mark, or Unicode bidirectional override/isolate character (`H-2`).
#[must_use]
pub(crate) fn is_control_or_bidi(ch: char) -> bool {
    ch.is_control()
        || matches!(
            ch,
            '\u{061C}' | '\u{200B}'..='\u{200F}' | '\u{2028}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'
        )
}

/// Checks a name a client gives a file: at most [`MAX_UPLOAD_NAME_CHARS`] characters,
/// without U+0000 or control/bidirectional formatting characters, and with a last segment,
/// whichever way its directories are parted, that is neither blank (`dir/`, `dir\`) nor
/// `.` or `..`.
///
/// What a record keeps of a name that passes is what follows its last `/`
/// ([`super::files::base_name`]).
///
/// # Errors
///
/// Returns the first of the causes, in the order of [`UnfitName`], that the name has.
pub(crate) fn check_file_name(name: &str) -> Result<(), UnfitName> {
    if name.chars().count() > MAX_UPLOAD_NAME_CHARS {
        return Err(UnfitName::TooLong);
    }
    if name.contains('\0') {
        return Err(UnfitName::Nul);
    }
    if name.chars().any(is_control_or_bidi) {
        return Err(UnfitName::ControlOrBidi);
    }
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name);
    if last.trim().is_empty() {
        return Err(UnfitName::NoFile);
    }
    if last == "." || last == ".." {
        return Err(UnfitName::Dots);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_of_a_file_passes_with_or_without_directories() {
        let longest = "가".repeat(MAX_UPLOAD_NAME_CHARS);
        for name in [
            "a.txt",
            "dir/a.txt",
            r"c:\dir\a.txt",
            "../a",
            "...",
            ".hidden",
            "a.",
            " a ",
            longest.as_str(),
        ] {
            assert_eq!(check_file_name(name), Ok(()), "{name:?}");
        }
    }

    #[test]
    fn a_name_that_is_none_says_why() {
        let longer = "가".repeat(MAX_UPLOAD_NAME_CHARS + 1);
        let long_directory = format!("{}/", "a".repeat(MAX_UPLOAD_NAME_CHARS));
        for (name, why) in [
            ("", UnfitName::NoFile),
            (" ", UnfitName::NoFile),
            ("dir/", UnfitName::NoFile),
            (r"dir\", UnfitName::NoFile),
            ("a/b/ ", UnfitName::NoFile),
            (r"a/b\ ", UnfitName::NoFile),
            (".", UnfitName::Dots),
            ("..", UnfitName::Dots),
            ("dir/..", UnfitName::Dots),
            (r"dir\.", UnfitName::Dots),
            ("a\0.txt", UnfitName::Nul),
            ("\0", UnfitName::Nul),
            ("a\n.txt", UnfitName::ControlOrBidi),
            ("a\x07.txt", UnfitName::ControlOrBidi),
            ("invoice\u{202E}fdp.exe", UnfitName::ControlOrBidi),
            ("doc\u{2066}x.txt", UnfitName::ControlOrBidi),
            ("doc\u{200F}x.txt", UnfitName::ControlOrBidi),
            ("doc\u{061C}x.txt", UnfitName::ControlOrBidi),
            ("\u{FEFF}a.txt", UnfitName::ControlOrBidi),
            (longer.as_str(), UnfitName::TooLong),
            (long_directory.as_str(), UnfitName::TooLong),
        ] {
            assert_eq!(check_file_name(name), Err(why), "{name:?}");
        }
    }
}
