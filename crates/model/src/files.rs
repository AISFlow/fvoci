//! The record kept about an uploaded file.

use serde::{Deserialize, Serialize};

/// What the server records about an uploaded file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct FileRecord {
    /// The record's identifier.
    pub id: String,
    /// The board the file belongs to. Never sent to clients: they ask for a file
    /// through its board.
    #[serde(skip)]
    pub container_id: String,
    /// The name the file is stored and asked for under: random, with the extension.
    /// Never sent to clients in this record; they have it from the upload's answer.
    #[serde(skip)]
    pub stored_name: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Where the file lives in the file storage. Never sent to clients.
    #[serde(skip)]
    pub path: String,
    /// The name the file was uploaded under.
    pub name: String,
    /// The lower-cased extension of the name, with its leading dot.
    pub extension: String,
    /// The size of the file in bytes.
    pub size: i64,
    /// The number of the file's newest version, from 1: what `path` and `size` are of.
    /// Never sent to clients in this record; the list of the file's versions has it.
    #[serde(skip)]
    pub version: i64,
    /// Who uploaded the file; empty where nobody is known to have. Never sent to
    /// clients in this record; the file's first version names them.
    #[serde(skip)]
    pub created_by: String,
}

/// A file of a board or a wiki as a list of them gives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ListedFile {
    /// The name the file is asked for under (`getFile`), and named by in a body
    /// (`fileId` of an image or a file block, `fvoci-file://{fileId}` in Markdown).
    pub file_id: String,
    /// The name the file was uploaded under.
    pub name: String,
    /// The lower-cased extension of the name, with its leading dot.
    pub extension: String,
    /// The size of the file in bytes.
    pub size: i64,
    /// When it was uploaded, in milliseconds since the Unix epoch.
    pub created_at: i64,
}

impl From<FileRecord> for ListedFile {
    fn from(record: FileRecord) -> Self {
        Self {
            file_id: record.stored_name,
            name: record.name,
            extension: record.extension,
            size: record.size,
            created_at: record.created_at,
        }
    }
}

impl Default for FileRecord {
    /// A record of nothing, at its first version.
    fn default() -> Self {
        Self {
            id: String::new(),
            container_id: String::new(),
            stored_name: String::new(),
            created_at: 0,
            path: String::new(),
            name: String::new(),
            extension: String::new(),
            size: 0,
            version: 1,
            created_by: String::new(),
        }
    }
}

/// Whether `path` is a place of the file storage written the one way the storage takes:
/// relative, its parts parted by single slashes, none of them empty, `.` or `..`, and
/// no backslash or NUL in it. The storage refuses any other spelling, and the store
/// takes no other into a record, a mark of writing or an upload session: a place has
/// one spelling, which the checks of whether an object is held compare.
#[must_use]
pub fn is_storage_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// The key of the file storage the server makes for a file:
/// `<organization>/<container>/<secret>`, three parts it made itself. The organization
/// comes first, so that one organization's files are the keys under its identifier,
/// whichever storage holds them; nothing of the name the file was uploaded with is in
/// it. The extension a file is asked for with is in its record, not in its key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileKey<'a> {
    /// The organization of the container.
    pub org_id: &'a str,
    /// The board or wiki the file is one of.
    pub container_id: &'a str,
    /// A secret of letters, digits, `-` and `_`.
    pub secret: &'a str,
}

impl<'a> FileKey<'a> {
    /// The key `path` is, if it has the shape of one the server makes: two identifiers
    /// and a secret. Anything else in the storage is none of the server's making,
    /// whatever names it.
    #[must_use]
    pub fn parse(path: &'a str) -> Option<Self> {
        let mut parts = path.split('/');
        let (Some(org_id), Some(container_id), Some(secret), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        let is_secret = !secret.is_empty()
            && secret
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        (crate::is_id(org_id) && crate::is_id(container_id) && is_secret).then_some(Self {
            org_id,
            container_id,
            secret,
        })
    }
}

impl std::fmt::Display for FileKey<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}/{}/{}",
            self.org_id, self.container_id, self.secret
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_place_has_one_spelling() {
        for path in ["o/b/x", "b/x.png", "x", "a/.hidden/b.c..d"] {
            assert!(is_storage_path(path), "{path}");
        }
        for path in [
            "",
            "/boards/x.png",
            "boards//x.png",
            "boards/x.png/",
            "./boards/x.png",
            "boards/./x.png",
            "boards/../x.png",
            "boards\\x.png",
            "boards/x\0.png",
        ] {
            assert!(!is_storage_path(path), "{path:?}");
        }
    }

    #[test]
    fn json_form_has_no_place_in_the_storage() {
        let record = FileRecord {
            id: "abc".to_owned(),
            container_id: "board".to_owned(),
            stored_name: "random.png".to_owned(),
            created_at: 1,
            path: "org/board/random".to_owned(),
            name: "picture.png".to_owned(),
            extension: ".png".to_owned(),
            size: 4,
            version: 3,
            created_by: "somebody".to_owned(),
        };
        let encoded = serde_json::to_value(&record).expect("a file record serializes");
        assert_eq!(
            encoded,
            json!({
                "id": "abc",
                "createdAt": 1,
                "name": "picture.png",
                "extension": ".png",
                "size": 4,
            }),
            "neither the path, nor the board, nor the stored name"
        );
    }

    #[test]
    fn a_key_the_server_makes_reads_back_and_nothing_else_does() {
        let org = "01890000-0000-7000-8000-00000000000a";
        let board = "01890000-0000-7000-8000-000000000001";
        let key = FileKey {
            org_id: org,
            container_id: board,
            secret: "Ab-_9",
        }
        .to_string();
        assert_eq!(key, format!("{org}/{board}/Ab-_9"), "organization first");
        assert_eq!(
            FileKey::parse(&key).map(|key| key.org_id),
            Some(org),
            "it reads back"
        );
        for other in [
            format!("{org}/{board}"),
            format!("{org}/{board}/x.png"),
            format!("{org}/{board}/x/y"),
            format!("{org}/not-an-id/x"),
            format!("not-an-id/{board}/x"),
            format!("{org}/{board}/"),
            format!("boards/20261112/{board}/x"),
            format!("{}/{board}/x", org.to_uppercase()),
            format!(".tmp/writing/{board}"),
        ] {
            assert_eq!(FileKey::parse(&other), None, "{other}");
        }
    }
}
