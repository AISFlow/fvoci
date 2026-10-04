//! Pure command validation and an exact immutable retry/preview binding.
use crate::api::personal_transfer::{PersonalTransferBody, PersonalTransferSelection};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub fn valid_selection(source: Uuid, selection: &PersonalTransferSelection) -> bool {
    source != selection.destination_workspace_id
        && selection.expected_document_version > 0
        && match selection.task_id {
            Some(_) => {
                selection
                    .expected_task_version
                    .is_some_and(|version| version > 0)
                    && selection.destination_status_id.is_some()
            }
            None => {
                selection.expected_task_version.is_none()
                    && selection.destination_status_id.is_none()
            }
        }
}

pub fn valid_command(source: Uuid, body: &PersonalTransferBody) -> bool {
    body.confirmed
        && valid_selection(source, &body.selection)
        && body.preview_digest.len() == 64
        && body
            .preview_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

/// Session is part of the captured operation, not a browser-supplied identity.
/// A new authenticated lifetime cannot silently replay an old mutation.
pub fn request_hash(
    source: Uuid,
    actor: Uuid,
    session: Uuid,
    body: &PersonalTransferBody,
) -> String {
    let canonical = serde_json::json!({"source":source,"actor":actor,"session":session,"selection":body.selection,"previewDigest":body.preview_digest});
    hex::encode(Sha256::digest(canonical.to_string().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::personal_transfer::PersonalTransferAction;

    fn command() -> PersonalTransferBody {
        PersonalTransferBody {
            request_id: Uuid::from_u128(1),
            selection: PersonalTransferSelection {
                action: PersonalTransferAction::Move,
                document_id: Uuid::from_u128(2),
                task_id: Some(Uuid::from_u128(3)),
                expected_document_version: 1,
                expected_task_version: Some(1),
                destination_workspace_id: Uuid::from_u128(4),
                destination_project_id: Uuid::from_u128(5),
                destination_status_id: Some(Uuid::from_u128(6)),
            },
            preview_digest: "a".repeat(64),
            confirmed: true,
        }
    }
    #[test]
    fn confirmation_and_affiliation_are_explicit() {
        let source = Uuid::from_u128(7);
        let mut body = command();
        assert!(valid_command(source, &body));
        body.confirmed = false;
        assert!(!valid_command(source, &body));
        body.confirmed = true;
        body.selection.destination_workspace_id = source;
        assert!(!valid_command(source, &body));
        body = command();
        body.selection.expected_task_version = None;
        assert!(!valid_command(source, &body));
        body = command();
        body.selection.task_id = None;
        assert!(!valid_command(source, &body));
        body.selection.expected_task_version = None;
        body.selection.destination_status_id = None;
        assert!(valid_command(source, &body));
        body.preview_digest = "A".repeat(64);
        assert!(!valid_command(source, &body));
    }
    #[test]
    fn retry_hash_binds_destination_version_action_and_authenticated_lifetime() {
        let source = Uuid::from_u128(7);
        let actor = Uuid::from_u128(8);
        let session = Uuid::from_u128(9);
        let original = command();
        let hash = request_hash(source, actor, session, &original);
        assert_eq!(hash, request_hash(source, actor, session, &original));
        for field in 0..5 {
            let mut changed = original.clone();
            match field {
                0 => changed.selection.destination_workspace_id = Uuid::from_u128(10),
                1 => changed.selection.destination_project_id = Uuid::from_u128(10),
                2 => changed.selection.expected_document_version += 1,
                3 => changed.selection.action = PersonalTransferAction::Copy,
                _ => changed.preview_digest = "b".repeat(64),
            }
            assert_ne!(hash, request_hash(source, actor, session, &changed));
        }
        assert_ne!(
            hash,
            request_hash(source, actor, Uuid::from_u128(10), &original)
        );
        assert_ne!(
            hash,
            request_hash(source, Uuid::from_u128(10), session, &original)
        );
    }
}
