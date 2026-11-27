//! Uploads in parts: the session a client opens to send one file in numbered parts.

/// The largest number of parts an upload has: what an S3 multipart upload takes.
pub const MAX_UPLOAD_PARTS: i64 = 10_000;

/// What an upload is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UploadPurpose {
    /// A file of a board.
    #[default]
    File,
    /// An archive to import into a workspace.
    Import,
}

impl UploadPurpose {
    /// The name the store and the contract give it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Import => "import",
        }
    }

    /// The purpose of a name, if it is one.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "file" => Some(Self::File),
            "import" => Some(Self::Import),
            _ => None,
        }
    }
}

/// An upload session as it is stored.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Upload {
    /// 256 random bits, URL-safe base64.
    pub id: String,
    /// The user who opened the session; nobody else may use it.
    pub user_id: String,
    /// What the upload is for.
    pub purpose: UploadPurpose,
    /// The board the file is for; empty for an import.
    pub container_id: String,
    /// The workspace an archive is imported into; empty for a file.
    pub workspace_id: String,
    /// The name of the file as the client gave it.
    pub name: String,
    /// The size of the file in bytes.
    pub size: i64,
    /// The content type the client gave, or empty.
    pub content_type: String,
    /// The size of every part but the last.
    pub part_size: i64,
    /// When the session was opened, in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// When the session ends, in milliseconds since the Unix epoch.
    pub expires_at: i64,
    /// When the session was closed to new parts; zero while it is open.
    pub closed_at: i64,
    /// The object store's identifier of the multipart upload, or empty.
    pub multipart_id: String,
    /// Where the file goes in the object store, for a multipart upload; empty where the
    /// parts are kept on the local disk.
    pub path: String,
}

impl Upload {
    /// The number of parts the file comes in: none for an empty file.
    #[must_use]
    pub fn part_count(&self) -> i64 {
        if self.part_size <= 0 {
            return 0;
        }
        self.size.div_euclid(self.part_size) + i64::from(self.size.rem_euclid(self.part_size) > 0)
    }

    /// The size of part `number` (from 1), or `None` for a number the upload has no part
    /// of.
    #[must_use]
    pub fn part_len(&self, number: i64) -> Option<i64> {
        if number < 1 || number > self.part_count() {
            return None;
        }
        Some((self.size - (number - 1) * self.part_size).min(self.part_size))
    }
}

/// A part of an upload that was received.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UploadPart {
    /// The part's number, from 1.
    pub number: i64,
    /// Its size in bytes.
    pub size: i64,
    /// The object store's identifier of the part, or empty.
    pub part_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upload(size: i64, part_size: i64) -> Upload {
        Upload {
            size,
            part_size,
            ..Upload::default()
        }
    }

    #[test]
    fn parts_are_of_the_part_size_but_the_last() {
        assert_eq!(upload(0, 8).part_count(), 0, "an empty file has no part");
        assert_eq!(upload(8, 8).part_count(), 1);
        assert_eq!(upload(9, 8).part_count(), 2);
        assert_eq!(upload(16, 8).part_count(), 2);
        let odd = upload(20, 8);
        let lens: Vec<_> = (0..=4).map(|number| odd.part_len(number)).collect();
        assert_eq!(lens, [None, Some(8), Some(8), Some(4), None]);
        assert_eq!(
            upload(16, 8).part_len(2),
            Some(8),
            "a last part that is full"
        );
        assert_eq!(upload(0, 8).part_len(1), None);
    }
}
