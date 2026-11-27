//! Public import format, summary, warning, and security-limit types.

use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use utoipa::ToSchema;

use crate::error::AppError;

/// Maximum number of skipped items or warnings kept in [`ImportSummary`] to bound memory
/// and response payload size.
pub const MAX_REPORTED_WARNINGS: usize = 200;

/// Maximum characters stored in an [`ImportWarning::item`] field.
const MAX_WARNING_ITEM_CHARS: usize = 200;

/// Maximum characters stored in an [`ImportWarning::reason`] field.
const MAX_WARNING_REASON_CHARS: usize = 500;

/// The source format of an import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ImportFormat {
    /// Native fvoci version 1 `.fvoci` ZIP archive.
    #[default]
    Fvoci,
    /// Trello board JSON export.
    Trello,
    /// Notion Markdown and CSV ZIP export.
    Notion,
}

impl ImportFormat {
    /// Wire representation of the import format.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fvoci => "fvoci",
            Self::Trello => "trello",
            Self::Notion => "notion",
        }
    }

    /// Validates that an uploaded file name and byte size match this import format.
    ///
    /// - `fvoci`: requires `.fvoci` or `.zip` and non-empty bytes.
    /// - `trello`: requires `.json` and non-empty bytes.
    /// - `notion`: requires `.zip` and non-empty bytes.
    ///
    /// # Errors
    ///
    /// Returns 400 Bad Request when the upload is empty or its extension does not match
    /// this format.
    pub fn validate_upload(self, file_name: &str, size: i64) -> Result<(), AppError> {
        if size <= 0 {
            return Err(AppError::bad_request("cannot import an empty file"));
        }
        let ext = Path::new(file_name)
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let ok = match self {
            Self::Fvoci => matches!(ext.as_deref(), Some("fvoci" | "zip")),
            Self::Trello => matches!(ext.as_deref(), Some("json")),
            Self::Notion => matches!(ext.as_deref(), Some("zip")),
        };
        if !ok {
            let expected = match self {
                Self::Fvoci => ".fvoci or .zip",
                Self::Trello => ".json",
                Self::Notion => ".zip",
            };
            return Err(AppError::bad_request(format_args!(
                "import format `{self}` requires a {expected} file"
            )));
        }
        Ok(())
    }
}

impl fmt::Display for ImportFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Error returned when a string names no supported [`ImportFormat`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("unsupported import format: {0:?}")]
pub struct ParseImportFormatError(String);

impl FromStr for ImportFormat {
    type Err = ParseImportFormatError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "fvoci" => Ok(Self::Fvoci),
            "trello" => Ok(Self::Trello),
            "notion" => Ok(Self::Notion),
            other => Err(ParseImportFormatError(other.to_owned())),
        }
    }
}

/// Counts of entities created by an import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportCounts {
    /// Number of boards created (`ContainerKind::Board`).
    pub boards: usize,
    /// Number of wikis created (`ContainerKind::Wiki`).
    pub wikis: usize,
    /// Number of courses created (`ContainerKind::Course`).
    #[serde(default)]
    pub courses: usize,
    /// Number of cards created (`BlockType::Card`).
    pub cards: usize,
    /// Number of wiki pages created (`BlockType::Page`).
    pub pages: usize,
    /// Number of course units created (`BlockType::Unit`).
    #[serde(default)]
    pub units: usize,
    /// Number of comments created (`BlockType::Comment`).
    pub comments: usize,
    /// Number of files stored.
    pub files: usize,
}

/// Stable machine-readable category for a skipped or degraded import item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportWarningCode {
    /// An external Trello attachment URL was preserved as a link without fetching it.
    ExternalAttachmentLinked,
    /// An external Markdown image URL was preserved as a link without fetching it.
    ExternalImageLinked,
    /// A relative asset or page link pointed to a path absent from the bundle.
    MissingLocalAsset,
    /// A closed/archived source list or card was skipped.
    ClosedItemSkipped,
    /// A field or list was truncated to fit server bounds.
    ContentTruncated,
    /// A Markdown construct was degraded to fit the fvoci body allowlist.
    MarkdownDegraded,
    /// An unsupported or duplicate entry in the bundle was skipped.
    UnsupportedEntrySkipped,
    /// A source item referenced an unknown list, card, or container ID.
    UnresolvedReference,
    /// A CSV row or companion Markdown file produced a non-fatal notice.
    CsvRowWarning,
}

/// One skipped or degraded item recorded during an import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportWarning {
    /// Machine-readable reason code.
    pub code: ImportWarningCode,
    /// Bounded source path or item label.
    pub item: String,
    /// Short explanation of what happened.
    pub reason: String,
}

/// Result summary returned after a completed import.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ImportSummary {
    /// Format that was imported.
    pub format: ImportFormat,
    /// Counts of items stored in the workspace.
    pub imported: ImportCounts,
    /// Native archive members left out because they are not in the workspace.
    pub members_left_out: usize,
    /// Identifiers of the newly created containers, in creation order.
    pub container_ids: Vec<String>,
    /// Items skipped during conversion or import.
    pub skipped: Vec<ImportWarning>,
    /// Non-fatal degradation notices (e.g. external URLs kept as links).
    pub warnings: Vec<ImportWarning>,
    /// Number of additional skipped items or warnings omitted past [`MAX_REPORTED_WARNINGS`].
    #[serde(default, skip_serializing_if = "is_zero")]
    pub warnings_omitted: usize,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if passes a reference"
)]
fn is_zero(count: &usize) -> bool {
    *count == 0
}

impl ImportSummary {
    /// Creates an empty summary for `format`.
    #[must_use]
    pub fn new(format: ImportFormat) -> Self {
        Self {
            format,
            ..Self::default()
        }
    }

    /// Records a skipped item, bounded by [`MAX_REPORTED_WARNINGS`].
    pub fn push_skipped(
        &mut self,
        code: ImportWarningCode,
        item: impl AsRef<str>,
        reason: impl Into<String>,
    ) {
        if self.skipped.len() < MAX_REPORTED_WARNINGS {
            let reason = reason.into();
            self.skipped.push(ImportWarning {
                code,
                item: truncate_chars(item.as_ref(), MAX_WARNING_ITEM_CHARS),
                reason: truncate_chars(&reason, MAX_WARNING_REASON_CHARS),
            });
        } else {
            self.warnings_omitted = self.warnings_omitted.saturating_add(1);
        }
    }

    /// Records a non-fatal warning, bounded by [`MAX_REPORTED_WARNINGS`].
    pub fn push_warning(
        &mut self,
        code: ImportWarningCode,
        item: impl AsRef<str>,
        reason: impl Into<String>,
    ) {
        if self.warnings.len() < MAX_REPORTED_WARNINGS {
            let reason = reason.into();
            self.warnings.push(ImportWarning {
                code,
                item: truncate_chars(item.as_ref(), MAX_WARNING_ITEM_CHARS),
                reason: truncate_chars(&reason, MAX_WARNING_REASON_CHARS),
            });
        } else {
            self.warnings_omitted = self.warnings_omitted.saturating_add(1);
        }
    }
}

/// Truncates `text` to at most `max_chars` Unicode scalar values without double allocation.
pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => text[..idx].to_owned(),
        None => text.to_owned(),
    }
}

/// Security and resource bounds enforced while reading and converting external imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BundleLimits {
    /// Maximum total entries across the outer ZIP and any nested ZIPs (also caps total blocks).
    pub max_entries: u64,
    /// Maximum cumulative decompressed bytes across all entries (1 GiB by default).
    pub max_total_unpacked_bytes: u64,
    /// Maximum decompressed bytes for a single binary attachment/image entry.
    pub max_file_bytes: u64,
    /// Maximum decompressed bytes for a single Markdown or CSV entry.
    pub max_text_entry_bytes: u64,
    /// Maximum byte size of a Trello JSON export file.
    pub max_trello_json_bytes: u64,
    /// Maximum allowed decompressed-to-compressed ratio once past `min_ratio_check_bytes`.
    pub max_compression_ratio: u64,
    /// Minimum decompressed size before the compression ratio ceiling applies.
    pub min_ratio_check_bytes: u64,
    /// Maximum directory segment depth of any normalized bundle path.
    pub max_path_depth: usize,
    /// Maximum total byte length of any normalized bundle path.
    pub max_path_bytes: usize,
    /// Maximum Unicode scalar values in any single path component.
    pub max_entry_name_chars: usize,
    /// Maximum nested ZIP depth (0 = outer ZIP only, 2 = up to two nested ZIP layers).
    pub max_nested_zip_depth: usize,
    /// Maximum total number of nested `.zip` archives unpacked from one bundle.
    pub max_nested_zips: usize,
    /// Maximum decompressed bytes of a single nested ZIP archive.
    pub max_nested_zip_bytes: u64,
    /// Maximum cumulative decompressed bytes across all nested ZIP archives on temp disk.
    pub max_total_nested_zip_bytes: u64,
    /// Maximum nesting depth of `{` and `[` in JSON inputs.
    pub max_json_nesting_depth: usize,
    /// Maximum number of data rows in a single CSV database.
    pub max_csv_rows: usize,
    /// Maximum number of columns in a single CSV database.
    pub max_csv_columns: usize,
    /// Maximum number of lists in a Trello export.
    pub max_trello_lists: usize,
    /// Maximum number of distinct labels (board + inline card labels) in a Trello export.
    pub max_trello_labels: usize,
    /// Maximum number of cards in a Trello export.
    pub max_trello_cards: usize,
    /// Maximum number of comments in a Trello export.
    pub max_trello_comments: usize,
    /// Maximum number of checklists in a Trello export.
    pub max_trello_checklists: usize,
    /// Maximum total checklist items across all checklists in a Trello export.
    pub max_trello_check_items: usize,
    /// Maximum total attachments across all cards in a Trello export.
    pub max_trello_attachments: usize,
    /// Maximum container nesting depth in Markdown inputs (blockquotes, lists).
    pub max_markdown_depth: usize,
}

impl BundleLimits {
    /// Default maximum entries per bundle (`import::MAX_ENTRIES`).
    pub const DEFAULT_MAX_ENTRIES: u64 = super::MAX_ENTRIES;
    /// Default maximum total unpacked bytes (`import::MAX_UNPACKED_BYTES`, 1 GiB).
    pub const DEFAULT_MAX_TOTAL_UNPACKED_BYTES: u64 = super::MAX_UNPACKED_BYTES;
    /// Default maximum single binary file bytes (overridden by organization `largest_file`).
    pub const DEFAULT_MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;
    /// Default maximum single Markdown or CSV entry bytes (16 MiB).
    pub const DEFAULT_MAX_TEXT_ENTRY_BYTES: u64 = 16 * 1024 * 1024;
    /// Default maximum Trello JSON size (16 MiB).
    pub const DEFAULT_MAX_TRELLO_JSON_BYTES: u64 = 16 * 1024 * 1024;
    /// Default maximum compression ratio (100:1) to halt zip bombs early.
    pub const DEFAULT_MAX_COMPRESSION_RATIO: u64 = 100;
    /// Minimum unpacked size (64 KiB) before enforcing compression ratio.
    pub const DEFAULT_MIN_RATIO_CHECK_BYTES: u64 = 64 * 1024;
    /// Default maximum path depth inside a ZIP bundle.
    pub const DEFAULT_MAX_PATH_DEPTH: usize = 32;
    /// Default maximum normalized path length in bytes.
    pub const DEFAULT_MAX_PATH_BYTES: usize = 1024;
    /// Default maximum characters per path segment (`MAX_UPLOAD_NAME_CHARS`).
    pub const DEFAULT_MAX_ENTRY_NAME_CHARS: usize = crate::app::MAX_UPLOAD_NAME_CHARS;
    /// Default maximum nested ZIP depth (Notion exports wrap content in 1–2 nested ZIPs).
    pub const DEFAULT_MAX_NESTED_ZIP_DEPTH: usize = 2;
    /// Default maximum count of nested ZIP archives unpacked from one bundle.
    pub const DEFAULT_MAX_NESTED_ZIPS: usize = 16;
    /// Default maximum decompressed bytes for a single nested ZIP archive (256 MiB).
    pub const DEFAULT_MAX_NESTED_ZIP_BYTES: u64 = 256 * 1024 * 1024;
    /// Default maximum cumulative decompressed bytes for all nested ZIPs combined (256 MiB).
    pub const DEFAULT_MAX_TOTAL_NESTED_ZIP_BYTES: u64 = 256 * 1024 * 1024;
    /// Default maximum JSON nesting depth.
    pub const DEFAULT_MAX_JSON_NESTING_DEPTH: usize = 64;
    /// Default maximum CSV rows per database.
    pub const DEFAULT_MAX_CSV_ROWS: usize = 5_000;
    /// Default maximum CSV columns per database.
    pub const DEFAULT_MAX_CSV_COLUMNS: usize = 100;
    /// Default maximum Trello lists.
    pub const DEFAULT_MAX_TRELLO_LISTS: usize = 200;
    /// Default maximum Trello labels.
    pub const DEFAULT_MAX_TRELLO_LABELS: usize = 200;
    /// Default maximum Trello cards.
    pub const DEFAULT_MAX_TRELLO_CARDS: usize = 5_000;
    /// Default maximum Trello comments.
    pub const DEFAULT_MAX_TRELLO_COMMENTS: usize = 10_000;
    /// Default maximum Trello checklists.
    pub const DEFAULT_MAX_TRELLO_CHECKLISTS: usize = 5_000;
    /// Default maximum Trello checklist items across all checklists.
    pub const DEFAULT_MAX_TRELLO_CHECK_ITEMS: usize = 50_000;
    /// Default maximum Trello attachments across all cards.
    pub const DEFAULT_MAX_TRELLO_ATTACHMENTS: usize = 10_000;
    /// Default maximum Markdown container nesting depth (leaves room under `BODY_MAX_DEPTH = 60`).
    pub const DEFAULT_MAX_MARKDOWN_DEPTH: usize = 50;

    /// Builds default limits with `max_file_bytes` clamped to the organization's file limit.
    #[must_use]
    pub const fn for_largest_file(largest_file: u64) -> Self {
        Self {
            max_entries: Self::DEFAULT_MAX_ENTRIES,
            max_total_unpacked_bytes: Self::DEFAULT_MAX_TOTAL_UNPACKED_BYTES,
            max_file_bytes: largest_file,
            max_text_entry_bytes: Self::DEFAULT_MAX_TEXT_ENTRY_BYTES,
            max_trello_json_bytes: Self::DEFAULT_MAX_TRELLO_JSON_BYTES,
            max_compression_ratio: Self::DEFAULT_MAX_COMPRESSION_RATIO,
            min_ratio_check_bytes: Self::DEFAULT_MIN_RATIO_CHECK_BYTES,
            max_path_depth: Self::DEFAULT_MAX_PATH_DEPTH,
            max_path_bytes: Self::DEFAULT_MAX_PATH_BYTES,
            max_entry_name_chars: Self::DEFAULT_MAX_ENTRY_NAME_CHARS,
            max_nested_zip_depth: Self::DEFAULT_MAX_NESTED_ZIP_DEPTH,
            max_nested_zips: Self::DEFAULT_MAX_NESTED_ZIPS,
            max_nested_zip_bytes: Self::DEFAULT_MAX_NESTED_ZIP_BYTES,
            max_total_nested_zip_bytes: Self::DEFAULT_MAX_TOTAL_NESTED_ZIP_BYTES,
            max_json_nesting_depth: Self::DEFAULT_MAX_JSON_NESTING_DEPTH,
            max_csv_rows: Self::DEFAULT_MAX_CSV_ROWS,
            max_csv_columns: Self::DEFAULT_MAX_CSV_COLUMNS,
            max_trello_lists: Self::DEFAULT_MAX_TRELLO_LISTS,
            max_trello_labels: Self::DEFAULT_MAX_TRELLO_LABELS,
            max_trello_cards: Self::DEFAULT_MAX_TRELLO_CARDS,
            max_trello_comments: Self::DEFAULT_MAX_TRELLO_COMMENTS,
            max_trello_checklists: Self::DEFAULT_MAX_TRELLO_CHECKLISTS,
            max_trello_check_items: Self::DEFAULT_MAX_TRELLO_CHECK_ITEMS,
            max_trello_attachments: Self::DEFAULT_MAX_TRELLO_ATTACHMENTS,
            max_markdown_depth: Self::DEFAULT_MAX_MARKDOWN_DEPTH,
        }
    }
}

impl Default for BundleLimits {
    fn default() -> Self {
        Self::for_largest_file(Self::DEFAULT_MAX_FILE_BYTES)
    }
}
