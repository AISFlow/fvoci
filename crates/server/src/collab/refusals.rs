//! What the server answers when a worker refuses a request about a document, or cannot
//! answer it.

use crate::collab_worker::wire::{Limits, Refusal, Refused};
use crate::error::{AppError, Reason};

/// The error a refusal of the worker stands for. What the worker said goes to the log,
/// not to the client.
pub(super) fn refused(refused: &Refused, limits: &Limits) -> AppError {
    let error = match refused.refusal {
        Refusal::Unreadable | Refusal::Unappliable | Refusal::NoBody => {
            AppError::bad_request("the document does not take the update")
                .with_reason(Reason::UpdateRefused)
        }
        Refusal::MissingChanges => AppError::conflict(
            "the update builds on changes the document does not have; load the document again",
        )
        .with_reason(Reason::UpdateOutOfStep),
        Refusal::UpdateTooLarge => AppError::too_large(
            format_args!(
                "the update is larger than the {} bytes the server takes",
                limits.update_bytes
            ),
            limits.update_bytes,
        )
        .with_reason(Reason::UpdateTooLarge),
        Refusal::DocumentTooLarge | Refusal::TooManySteps => AppError::too_large(
            "with the update the document would be larger than it may be",
            limits.state_bytes,
        )
        .with_reason(Reason::DocumentTooLarge),
        Refusal::BodyTooLarge => AppError::too_large(
            "with the update the body would be past the limits of a body (its size, its nodes, its depth, its board views)",
            limits.body_bytes,
        )
        .with_reason(Reason::DocumentTooLarge),
        Refusal::TableTooLarge => AppError::bad_request(format_args!(
            "with the update a table would have more than {} rows or more than {} columns",
            fvoci_model::TABLE_MAX_ROWS,
            fvoci_model::TABLE_MAX_COLUMNS,
        ))
        .with_reason(Reason::TableTooLarge),
        Refusal::TooManyDocuments => {
            AppError::service_unavailable("the server holds as many documents as it may")
                .with_reason(Reason::TooManyDocuments)
        }
        Refusal::Closed => unavailable(),
        Refusal::UnknownDocument | Refusal::OutOfOrder => {
            return AppError::internal(format_args!("the worker refused a request: {refused}"));
        }
    };
    error.context(format_args!("the worker refused it: {refused}"))
}

/// The error of a request made against another making of the document than the one
/// there is.
pub(super) fn out_of_step() -> AppError {
    AppError::conflict(
        "the document was made again since the editor loaded it; load the document again",
    )
    .with_reason(Reason::UpdateOutOfStep)
}

/// The error of a document that cannot be worked on for now.
pub(super) fn unavailable() -> AppError {
    AppError::service_unavailable("the document cannot be worked on for now; try again later")
        .with_reason(Reason::DocumentUnavailable)
}

/// The error of an update of a user, or of an organization, whose updates ended a
/// worker a little before ([`super::senders`]). As a document that cannot be worked on
/// for now, which an editor sends again later.
pub(super) fn held_back() -> AppError {
    AppError::service_unavailable(
        "an update of the user, or of their organization, ended the server's worker a \
         little before: their updates are refused for a while; try again later",
    )
    .with_reason(Reason::DocumentUnavailable)
}

/// The error of an update from a connection that only reads the document.
pub(super) fn read_only() -> AppError {
    AppError::forbidden("the connection reads the document and does not write it")
        .with_reason(Reason::DocumentReadOnly)
}

/// The error of a connection that is in as many rooms as one may be.
pub(super) fn too_many_joined(limit: usize) -> AppError {
    // A count of a collection, which a setting of 32 bits gave.
    AppError::limited(
        "the connection is in as many rooms as it may be",
        Reason::TooManyDocumentsJoined,
    )
    .with_limit(u64::try_from(limit).unwrap_or(u64::MAX))
}

/// The error of a room that as many connections are in as may be.
pub(super) fn full(limit: usize) -> AppError {
    AppError::limited(
        "as many connections edit the body as may",
        Reason::DocumentFull,
    )
    .with_limit(u64::try_from(limit).unwrap_or(u64::MAX))
}

/// The error of a room that as many connections of one user are in as may be.
pub(super) fn too_many_of_user(limit: usize) -> AppError {
    AppError::limited(
        "as many connections of the user edit the body as may",
        Reason::TooManyConnectionsJoined,
    )
    .with_limit(u64::try_from(limit).unwrap_or(u64::MAX))
}

/// The error of a connection that sends more in a second than it may.
pub(super) fn too_many_frames(limit: u32) -> AppError {
    AppError::limited(
        "the connection sends more commands and frames in a second than it may",
        Reason::TooManyFrames,
    )
    .with_limit(u64::from(limit))
}

/// The error of a connection that does not read what it is sent.
pub(super) fn too_slow() -> AppError {
    AppError::limited(
        "the connection does not read what it is sent",
        Reason::ConnectionTooSlow,
    )
}
