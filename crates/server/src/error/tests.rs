//! The tests of the errors and the bodies they are reported with.

use fvoci_model::{CardError, Category};

use super::*;

/// The status and the body an error of the store is reported with. The answer they
/// are sent in is `error_response`'s, and tested there.
fn reported(error: StoreError) -> (u16, Problem) {
    let error = AppError::from(error);
    (error.status(), error.body())
}

#[test]
fn no_two_problems_have_the_same_name() {
    let mut names: Vec<String> = ErrorKind::ALL
        .iter()
        .map(|kind| kind.name().to_owned())
        .chain(Reason::ALL.iter().map(|reason| reason.name()))
        .collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count, "{names:?}");
}

#[tokio::test]
async fn a_block_the_store_refused_is_found_in_the_list_of_the_request() {
    let refused = || {
        AppError::from(StoreError::InvalidBlock {
            id: "b2".to_owned(),
            source: BlockError::NulInTitle,
        })
    };
    let pointers = |error: AppError| -> Vec<Option<String>> {
        error
            .body()
            .errors
            .into_iter()
            .map(|entry| entry.pointer)
            .collect()
    };
    assert_eq!(
        pointers(refused()),
        [Some("/title".to_owned())],
        "a request that is the block"
    );
    assert_eq!(
        pointers(refused().among_blocks(["b1", "b2"], "/blockPatches")),
        [Some("/blockPatches/1/title".to_owned())]
    );
    assert_eq!(
        pointers(refused().among_blocks(["b1", "b2"], "")),
        [Some("/1/title".to_owned())],
        "the body is the list"
    );
    assert_eq!(
        pointers(refused().among_blocks(["b1"], "")),
        [],
        "a block the request does not list: nowhere to point"
    );
    let board = || {
        AppError::from(StoreError::InvalidContainer(ContainerError::NulCharacter {
            member: "properties",
        }))
    };
    assert_eq!(
        pointers(board().moved("/properties", "/updatedProperties")),
        [Some("/updatedProperties".to_owned())],
        "a patch names the member its own way"
    );
    assert_eq!(
        pointers(board().among_blocks(["b1"], "/blocks")),
        [],
        "one of several boards: nowhere to point"
    );
    assert_eq!(
        pointers(refused().outside_the_request()),
        [],
        "what is checked of an archive is no member of the request"
    );
    assert_eq!(
        refused().outside_the_request().body().detail,
        "the title holds the character U+0000",
        "the words still say what is wrong"
    );
    let other = AppError::bad_request("no").with_field("blockIds");
    assert_eq!(
        pointers(other.among_blocks(["b1"], "/blockPatches")),
        [Some("/blockIds".to_owned())],
        "an error that is about no block stays"
    );
}

#[test]
fn what_the_store_does_not_find_is_not_found_without_the_identifier() {
    for error in [
        StoreError::NotFound("board ID=bsecret".to_owned()),
        StoreError::NoRows,
        StoreError::NotAllFound {
            entity: "block",
            ids: vec!["csecret".to_owned()],
        },
    ] {
        let (status, body) = reported(error);
        assert_eq!(status, 404);
        assert_eq!(body.problem_type, "/problems/not-found");
        assert_eq!(body.detail, "not found", "no identifier, no database text");
    }
}

#[test]
fn what_a_client_sent_and_the_store_refused_is_a_bad_request() {
    let category = Category::default()
        .validate()
        .expect_err("an empty category");
    let block = |source| StoreError::InvalidBlock {
        id: "c1".to_owned(),
        source,
    };
    let errors = [
        StoreError::from(category),
        block(BlockError::TitleSizeLimitExceeded),
        block(BlockError::FieldsSizeLimitExceeded),
        StoreError::InvalidContainersAndBlocks(fvoci_model::ContainersAndBlocksError::NoContainers),
    ];
    for error in errors {
        let text = error.to_string();
        let (status, body) = reported(error);
        assert_eq!(status, 400, "{text}");
        assert_eq!(body.detail, text, "the store's message says what is wrong");
    }
}

#[test]
fn a_user_the_store_does_not_find_is_not_found_without_the_name() {
    let (status, body) = reported(StoreError::UserNotFound("u1".to_owned()));
    assert_eq!(status, 404);
    assert_eq!(body.detail, "user not found");
}

#[test]
fn any_other_store_error_is_a_server_error() {
    let errors = [
        block_without_board(),
        StoreError::Config("no such file".to_owned()),
    ];
    for error in errors {
        let (status, body) = reported(error);
        assert_eq!(status, 500);
        assert_eq!(body.detail, "internal server error");
    }
}

fn block_without_board() -> StoreError {
    StoreError::InvalidBlock {
        id: "c1".to_owned(),
        source: BlockError::EmptyContainerId,
    }
}

#[test]
fn card_errors_are_the_clients_or_the_servers() {
    assert_eq!(
        AppError::from(CardError::BoardIdMismatch).kind(),
        ErrorKind::BadRequest,
        "a card for another board"
    );
    assert_eq!(
        AppError::from(CardError::Invalid("an icon is at most one grapheme")).kind(),
        ErrorKind::BadRequest,
        "an invalid card"
    );
    assert_eq!(
        AppError::from(CardError::NotCardBlock).kind(),
        ErrorKind::Internal,
        "a stored block that is no card"
    );
}
