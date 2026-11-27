//! The file a client says it is about to send in parts, and what of it is refused
//! before a session is opened.

use std::num::NonZeroU64;

use fvoci_model::{RefusalReason, Refusals, UploadPurpose};

use super::MAX_UPLOAD_NAME_CHARS;
use crate::app::file_name::{UnfitName, check_file_name};
use crate::error::{AppError, Reason};

/// A session to open: the file the client is about to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileToUpload {
    /// The name of the file.
    pub name: String,
    /// Its size in bytes.
    pub size: i64,
    /// Its content type as the client knows it, or empty.
    pub content_type: String,
}

impl FileToUpload {
    /// Checks what a request says of the file it is going to send, all of it.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused: a name that is none of a file
    /// ([`check_file_name`]), a content type that is too long, a size below zero.
    pub(super) fn check(&self) -> Result<(), Refusals> {
        // Every member that is refused, not the first alone.
        let mut refused = Refusals::new();
        if let Err(unfit) = check_file_name(&self.name) {
            let reason = match unfit {
                UnfitName::TooLong => RefusalReason::TooLong,
                UnfitName::Nul | UnfitName::ControlOrBidi => RefusalReason::InvalidCharacters,
                UnfitName::NoFile => RefusalReason::Required,
                UnfitName::Dots => RefusalReason::InvalidFormat,
            };
            refused.refuse("name", reason, format!("the name {unfit}"));
        }
        if self.content_type.chars().count() > MAX_UPLOAD_NAME_CHARS {
            refused.refuse(
                "contentType",
                RefusalReason::TooLong,
                format!("the content type is at most {MAX_UPLOAD_NAME_CHARS} characters"),
            );
        }
        if self.size < 0 {
            refused.refuse(
                "size",
                RefusalReason::Invalid,
                "the size is 0 or more bytes",
            );
        }
        refused.into_result()
    }
}

/// The refusal of a file of more than `limit` bytes, the largest an upload for
/// `purpose` takes: 413 with the limit and the reason of the purpose.
pub(super) fn too_large(purpose: UploadPurpose, limit: NonZeroU64) -> AppError {
    let (what, reason) = match purpose {
        UploadPurpose::File => ("file", Reason::FileTooLarge),
        UploadPurpose::Import => ("archive", Reason::ArchiveTooLarge),
    };
    let limit = limit.get();
    AppError::too_large(
        format_args!("the {what} is larger than {limit} bytes"),
        limit,
    )
    .with_reason(reason)
}
