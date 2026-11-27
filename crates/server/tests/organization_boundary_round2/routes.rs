//! Inventory of every legacy workspace route, including body and query targets.

#[path = "routes/extra.rs"]
mod extra;
#[path = "routes/reach.rs"]
mod reach;

use std::collections::BTreeSet;

use fvoci_model::{
    Block, BlockType, CategoryContainers, Container, ContainerMember, ContainerVisibility,
    UserWorkspace, Workspace, WorkspaceRole, new_id,
};
use fvoci_store::DbType;
use reqwest::StatusCode;
use serde_json::{Value, json};

use super::common::ROOT_WS;
use super::common::{
    Client, HIDDEN_WORKSPACE_ID, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, UNKNOWN_ID,
};
use super::{answer, db_type_of_the_tests};
use reach::{REACHED_BY_ITEM, REFUSED_IN_REACH};

// Method, path suffix, operation ID: every operation under `/w/{workspaceId}`, also
// compared against the published contract. A parameter of the path is filled in with an
// identifier of nothing; `{partNumber}` with 1.
pub(super) const WORKSPACE_ROUTES: &[(&str, &str, &str)] = &[
    ("GET", "", "getWorkspace"),
    (
        "GET",
        "/containers/{containerId}/teams",
        "getContainerTeams",
    ),
    (
        "PUT",
        "/containers/{containerId}/teams/{teamId}",
        "setContainerTeam",
    ),
    (
        "DELETE",
        "/containers/{containerId}/teams/{teamId}",
        "removeContainerTeam",
    ),
    ("GET", "/teams", "getTeams"),
    ("POST", "/teams", "createTeam"),
    ("GET", "/teams/{teamId}", "getTeam"),
    ("PUT", "/teams/{teamId}", "renameTeam"),
    ("DELETE", "/teams/{teamId}", "deleteTeam"),
    ("PUT", "/teams/{teamId}/members/{userId}", "addTeamMember"),
    (
        "DELETE",
        "/teams/{teamId}/members/{userId}",
        "removeTeamMember",
    ),
    ("GET", "/archive/export", "exportWorkspaceArchive"),
    ("POST", "/archive/import", "importArchive"),
    ("POST", "/archive/uploads", "createImportUpload"),
    ("GET", "/audit", "getWorkspaceAudit"),
    ("POST", "/boards", "createBoard"),
    ("GET", "/boards", "getWorkspaceBoards"),
    ("GET", "/containers", "listWorkspaceContainers"),
    ("GET", "/overview", "getWorkspaceOverview"),
    (
        "GET",
        "/containers/{containerId}/files",
        "listContainerFiles",
    ),
    (
        "POST",
        "/cards/{cardId}/body/markdown",
        "appendCardMarkdown",
    ),
    (
        "GET",
        "/boards/{boardId}/cards/{cardId}/files",
        "listCardFiles",
    ),
    (
        "POST",
        "/containers-and-blocks",
        "createContainersAndBlocks",
    ),
    (
        "DELETE",
        "/containers-and-blocks",
        "deleteContainersAndBlocks",
    ),
    (
        "PATCH",
        "/containers-and-blocks",
        "patchContainersAndBlocks",
    ),
    ("GET", "/boards/search?q=secret", "searchWorkspaceBoards"),
    ("GET", "/search?q=secret", "searchWorkspaceContent"),
    ("DELETE", "/containers/{containerId}", "deleteContainer"),
    ("GET", "/containers/{containerId}", "getContainer"),
    ("PATCH", "/containers/{containerId}", "patchContainer"),
    (
        "GET",
        "/containers/{containerId}/archive/export",
        "exportContainerArchive",
    ),
    ("POST", "/boards/{boardId}/blocks", "createBlocks"),
    ("GET", "/boards/{boardId}/blocks", "getBlocks"),
    ("PATCH", "/boards/{boardId}/blocks", "patchBlocks"),
    (
        "DELETE",
        "/boards/{boardId}/blocks/{blockId}",
        "deleteBlock",
    ),
    ("PATCH", "/boards/{boardId}/blocks/{blockId}", "patchBlock"),
    (
        "POST",
        "/boards/{boardId}/blocks/{blockId}/duplicate",
        "duplicateBlock",
    ),
    (
        "POST",
        "/boards/{boardId}/blocks/{blockId}/restore",
        "restoreBlock",
    ),
    (
        "GET",
        "/boards/{boardId}/blocks/{blockId}/versions",
        "getCommentVersions",
    ),
    ("POST", "/boards/{boardId}/cards", "createCard"),
    ("GET", "/boards/{boardId}/cards", "getCards"),
    (
        "GET",
        "/boards/{boardId}/cards/by-number/{number}",
        "getCardByNumber",
    ),
    (
        "POST",
        "/containers/{containerId}/duplicate",
        "duplicateContainer",
    ),
    ("POST", "/containers/{containerId}/files", "uploadFile"),
    ("GET", "/containers/{containerId}/files/{fileId}", "getFile"),
    (
        "GET",
        "/containers/{containerId}/files/{fileId}/info",
        "getFileRecord",
    ),
    (
        "GET",
        "/containers/{containerId}/files/{fileId}/versions",
        "getFileVersions",
    ),
    (
        "POST",
        "/containers/{containerId}/files/{fileId}/versions",
        "addFileVersion",
    ),
    (
        "GET",
        "/containers/{containerId}/files/{fileId}/versions/{version}",
        "getFileVersion",
    ),
    (
        "POST",
        "/containers/{containerId}/files/{fileId}/versions/{version}/restore",
        "restoreFileVersion",
    ),
    (
        "GET",
        "/containers/{containerId}/files/{fileId}/lease",
        "getFileLease",
    ),
    (
        "PUT",
        "/containers/{containerId}/files/{fileId}/lease",
        "holdFileLease",
    ),
    (
        "DELETE",
        "/containers/{containerId}/files/{fileId}/lease/{leaseId}",
        "releaseFileLease",
    ),
    ("POST", "/containers/{containerId}/join", "joinContainer"),
    ("POST", "/containers/{containerId}/leave", "leaveContainer"),
    ("POST", "/containers/{containerId}/members", "addMember"),
    ("GET", "/containers/{containerId}/members", "getMembers"),
    (
        "DELETE",
        "/containers/{containerId}/members/{userId}",
        "removeMember",
    ),
    (
        "PUT",
        "/containers/{containerId}/members/{userId}",
        "updateMember",
    ),
    (
        "PUT",
        "/boards/{boardId}/property-order",
        "reorderCardProperties",
    ),
    (
        "GET",
        "/containers/{containerId}/public-link",
        "getPublicLink",
    ),
    (
        "POST",
        "/containers/{containerId}/public-link",
        "setPublicLink",
    ),
    ("GET", "/boards/{boardId}/trash/cards", "getDeletedCards"),
    (
        "POST",
        "/containers/{containerId}/restore",
        "restoreContainer",
    ),
    ("POST", "/containers/{containerId}/uploads", "createUpload"),
    ("GET", "/cards/{cardId}", "getCard"),
    ("PATCH", "/cards/{cardId}", "patchCard"),
    (
        "GET",
        "/cards/{cardId}/body-versions",
        "getCardBodyVersions",
    ),
    (
        "POST",
        "/cards/{cardId}/body-versions",
        "keepCardBodyVersion",
    ),
    (
        "GET",
        "/cards/{cardId}/body-versions/{version}",
        "getCardBodyVersion",
    ),
    (
        "POST",
        "/cards/{cardId}/body-versions/{version}/restore",
        "restoreCardBodyVersion",
    ),
    ("POST", "/categories", "createCategory"),
    ("GET", "/categories", "getCategories"),
    ("DELETE", "/categories/{categoryId}", "deleteCategory"),
    ("PUT", "/categories/{categoryId}", "updateCategory"),
    (
        "PUT",
        "/categories/{categoryId}/container-order",
        "reorderCategoryContainers",
    ),
    (
        "POST",
        "/categories/{categoryId}/containers/{containerId}",
        "setContainerCategory",
    ),
    (
        "PUT",
        "/categories/{categoryId}/containers/{containerId}/hide",
        "hideContainer",
    ),
    (
        "PUT",
        "/categories/{categoryId}/containers/{containerId}/unhide",
        "unhideContainer",
    ),
    ("PUT", "/category-order", "reorderCategories"),
    ("GET", "/members", "getWorkspaceMembers"),
    ("PUT", "/members/{user}", "setWorkspaceRole"),
    ("DELETE", "/members/{user}", "removeWorkspaceMember"),
    ("POST", "/onboard", "onboard"),
    ("POST", "/references/resolve", "resolveReferences"),
    ("GET", "/references/search?q=secret", "searchReferences"),
    ("GET", "/references/to/{blockId}", "getReferencesTo"),
    ("PUT", "/guest-invitations", "setGuestInvitations"),
    ("GET", "/invitations", "getInvitations"),
    ("POST", "/invitations", "createInvitation"),
    ("POST", "/invitations/mail", "emailInvitation"),
    ("DELETE", "/invitations/{invitation}", "revokeInvitation"),
    ("POST", "/subscriptions", "createSubscription"),
    (
        "DELETE",
        "/subscriptions/{blockId}/{userId}",
        "deleteSubscription",
    ),
    ("GET", "/subscriptions/{userId}", "getSubscriptions"),
    ("GET", "/templates", "getTemplates"),
    ("GET", "/trash/containers", "getDeletedContainers"),
    ("DELETE", "/uploads/{uploadId}", "deleteUpload"),
    ("GET", "/uploads/{uploadId}", "getUpload"),
    ("POST", "/uploads/{uploadId}/complete", "completeUpload"),
    (
        "PUT",
        "/uploads/{uploadId}/parts/{partNumber}",
        "putUploadPart",
    ),
    ("GET", "/users", "getWorkspaceUsers"),
    ("POST", "/users", "getWorkspaceUsersById"),
    ("GET", "/users/{userId}", "getWorkspaceUser"),
    ("GET", "/wikis", "getWikis"),
    ("POST", "/wikis", "createWiki"),
    ("GET", "/wikis/{wikiId}/pages", "getPageTree"),
    ("POST", "/wikis/{wikiId}/pages", "createPage"),
    ("GET", "/wikis/{wikiId}/pages/{pageId}", "getPage"),
    ("PATCH", "/wikis/{wikiId}/pages/{pageId}", "patchPage"),
    ("DELETE", "/wikis/{wikiId}/pages/{pageId}", "deletePage"),
    (
        "GET",
        "/wikis/{wikiId}/pages/{pageId}/comments",
        "getPageComments",
    ),
    (
        "POST",
        "/wikis/{wikiId}/pages/{pageId}/comments",
        "createPageComment",
    ),
    (
        "PATCH",
        "/wikis/{wikiId}/pages/{pageId}/comments/{commentId}",
        "patchPageComment",
    ),
    (
        "DELETE",
        "/wikis/{wikiId}/pages/{pageId}/comments/{commentId}",
        "deletePageComment",
    ),
    (
        "GET",
        "/wikis/{wikiId}/pages/{pageId}/comments/{commentId}/versions",
        "getPageCommentVersions",
    ),
    (
        "GET",
        "/wikis/{wikiId}/pages/{pageId}/body-versions",
        "getPageBodyVersions",
    ),
    (
        "POST",
        "/wikis/{wikiId}/pages/{pageId}/body-versions",
        "keepPageBodyVersion",
    ),
    (
        "GET",
        "/wikis/{wikiId}/pages/{pageId}/body-versions/{version}",
        "getPageBodyVersion",
    ),
    (
        "POST",
        "/wikis/{wikiId}/pages/{pageId}/body-versions/{version}/restore",
        "restorePageBodyVersion",
    ),
    ("POST", "/wikis/{wikiId}/pages/{pageId}/move", "movePage"),
    (
        "POST",
        "/wikis/{wikiId}/pages/{pageId}/duplicate",
        "duplicatePage",
    ),
    ("GET", "/wikis/{wikiId}/trash/pages", "getDeletedPages"),
    (
        "POST",
        "/wikis/{wikiId}/pages/{pageId}/restore",
        "restorePage",
    ),
    (
        "POST",
        "/wikis/{wikiId}/pages/{pageId}/comments/{commentId}/restore",
        "restorePageComment",
    ),
    ("GET", "/diaries", "getDiaries"),
    ("POST", "/diaries", "createDiary"),
    ("GET", "/diaries/{diaryId}/days", "getDiaryDays"),
    ("GET", "/diaries/{diaryId}/runs", "getDiaryRuns"),
    (
        "GET",
        "/diaries/{diaryId}/days/{monthDay}",
        "getDiaryEntriesOn",
    ),
    ("POST", "/diaries/{diaryId}/entries", "createDiaryEntry"),
    (
        "GET",
        "/diaries/{diaryId}/entries/{entryId}",
        "getDiaryEntry",
    ),
    (
        "PATCH",
        "/diaries/{diaryId}/entries/{entryId}",
        "patchDiaryEntry",
    ),
    (
        "DELETE",
        "/diaries/{diaryId}/entries/{entryId}",
        "deleteDiaryEntry",
    ),
    (
        "GET",
        "/diaries/{diaryId}/trash/entries",
        "getDeletedDiaryEntries",
    ),
    (
        "POST",
        "/diaries/{diaryId}/entries/{entryId}/restore",
        "restoreDiaryEntry",
    ),
    (
        "GET",
        "/diaries/{diaryId}/entries/{entryId}/body-versions",
        "getDiaryEntryBodyVersions",
    ),
    (
        "POST",
        "/diaries/{diaryId}/entries/{entryId}/body-versions",
        "keepDiaryEntryBodyVersion",
    ),
    (
        "GET",
        "/diaries/{diaryId}/entries/{entryId}/body-versions/{version}",
        "getDiaryEntryBodyVersion",
    ),
    (
        "POST",
        "/diaries/{diaryId}/entries/{entryId}/body-versions/{version}/restore",
        "restoreDiaryEntryBodyVersion",
    ),
    ("GET", "/courses", "getCourses"),
    ("POST", "/courses", "createCourse"),
    ("GET", "/courses/{courseId}/units", "getUnitTree"),
    ("POST", "/courses/{courseId}/units", "createUnit"),
    ("GET", "/courses/{courseId}/units/{unitId}", "getUnit"),
    ("PATCH", "/courses/{courseId}/units/{unitId}", "patchUnit"),
    ("DELETE", "/courses/{courseId}/units/{unitId}", "deleteUnit"),
    (
        "GET",
        "/courses/{courseId}/units/{unitId}/comments",
        "getUnitComments",
    ),
    (
        "POST",
        "/courses/{courseId}/units/{unitId}/comments",
        "createUnitComment",
    ),
    (
        "PATCH",
        "/courses/{courseId}/units/{unitId}/comments/{commentId}",
        "patchUnitComment",
    ),
    (
        "DELETE",
        "/courses/{courseId}/units/{unitId}/comments/{commentId}",
        "deleteUnitComment",
    ),
    (
        "GET",
        "/courses/{courseId}/units/{unitId}/comments/{commentId}/versions",
        "getUnitCommentVersions",
    ),
    (
        "POST",
        "/courses/{courseId}/units/{unitId}/comments/{commentId}/restore",
        "restoreUnitComment",
    ),
    (
        "GET",
        "/courses/{courseId}/units/{unitId}/body-versions",
        "getUnitBodyVersions",
    ),
    (
        "POST",
        "/courses/{courseId}/units/{unitId}/body-versions",
        "keepUnitBodyVersion",
    ),
    (
        "GET",
        "/courses/{courseId}/units/{unitId}/body-versions/{version}",
        "getUnitBodyVersion",
    ),
    (
        "POST",
        "/courses/{courseId}/units/{unitId}/body-versions/{version}/restore",
        "restoreUnitBodyVersion",
    ),
    (
        "POST",
        "/courses/{courseId}/units/{unitId}/move",
        "moveUnit",
    ),
    ("GET", "/courses/{courseId}/trash/units", "getDeletedUnits"),
    (
        "POST",
        "/courses/{courseId}/units/{unitId}/restore",
        "restoreUnit",
    ),
    ("GET", "/courses/{courseId}/progress", "getMyCourseProgress"),
    (
        "PUT",
        "/courses/{courseId}/units/{unitId}/progress",
        "putUnitProgress",
    ),
    (
        "GET",
        "/courses/{courseId}/progress/summary",
        "getCourseProgressSummary",
    ),
    ("GET", "/courses/{courseId}/meetings", "listCourseMeetings"),
    (
        "POST",
        "/courses/{courseId}/meetings",
        "createCourseMeeting",
    ),
    (
        "PATCH",
        "/courses/{courseId}/meetings/{meetingId}",
        "patchCourseMeeting",
    ),
    (
        "DELETE",
        "/courses/{courseId}/meetings/{meetingId}",
        "deleteCourseMeeting",
    ),
    (
        "PUT",
        "/courses/{courseId}/meetings/{meetingId}/attendance/{userId}",
        "putMeetingAttendance",
    ),
    ("GET", "/courses/{courseId}/exams", "listCourseExams"),
    ("POST", "/courses/{courseId}/exams", "createCourseExam"),
    (
        "PATCH",
        "/courses/{courseId}/exams/{examId}",
        "patchCourseExam",
    ),
    (
        "DELETE",
        "/courses/{courseId}/exams/{examId}",
        "deleteCourseExam",
    ),
    (
        "PUT",
        "/courses/{courseId}/exams/{examId}/my-score",
        "putMyExamScore",
    ),
    (
        "DELETE",
        "/courses/{courseId}/exams/{examId}/my-score",
        "deleteMyExamScore",
    ),
    (
        "POST",
        "/courses/{courseId}/exams/{examId}/results-publication",
        "publishCourseExamResults",
    ),
    (
        "DELETE",
        "/courses/{courseId}/exams/{examId}/results-publication",
        "unpublishCourseExamResults",
    ),
    ("GET", "/courses/{courseId}/reviews", "listMyCourseReviews"),
    (
        "POST",
        "/courses/{courseId}/units/{unitId}/reviews",
        "scheduleUnitReview",
    ),
    (
        "POST",
        "/courses/{courseId}/reviews/{reviewId}/complete",
        "completeUnitReview",
    ),
    (
        "DELETE",
        "/courses/{courseId}/reviews/{reviewId}",
        "deleteUnitReview",
    ),
    (
        "GET",
        "/courses/{courseId}/units/{unitId}/notes",
        "listMyUnitNotes",
    ),
    (
        "POST",
        "/courses/{courseId}/units/{unitId}/notes",
        "createUnitNote",
    ),
    (
        "PATCH",
        "/courses/{courseId}/notes/{noteId}",
        "patchUnitNote",
    ),
    (
        "DELETE",
        "/courses/{courseId}/notes/{noteId}",
        "deleteUnitNote",
    ),
    ("GET", "/learning/reviews/today", "getWorkspaceTodayReviews"),
    ("GET", "/courses/{courseId}/schedule", "getCourseSchedule"),
    ("PUT", "/courses/{courseId}/schedule", "putCourseSchedule"),
    (
        "DELETE",
        "/courses/{courseId}/schedule",
        "deleteCourseSchedule",
    ),
    (
        "POST",
        "/courses/{courseId}/schedule/redistribute",
        "redistributeCourseSchedule",
    ),
    (
        "POST",
        "/courses/{courseId}/my-schedule/redistribute",
        "redistributeMyCourseSchedule",
    ),
    (
        "PUT",
        "/courses/{courseId}/units/{unitId}/schedule",
        "putUnitSchedule",
    ),
    ("GET", "/learning/today", "getWorkspaceLearningToday"),
    ("GET", "/today", "getWorkspaceToday"),
    (
        "GET",
        "/containers/{containerId}/my-permissions",
        "getMyContainerPermissions",
    ),
    (
        "POST",
        "/containers/{containerId}/copy-to-workspace",
        "copyContainerToWorkspace",
    ),
    (
        "POST",
        "/boards/{boardId}/cards/{cardId}/duplicate",
        "duplicateCard",
    ),
    (
        "POST",
        "/boards/{boardId}/cards/batch-delete",
        "batchDeleteCards",
    ),
    (
        "POST",
        "/boards/{boardId}/cards/batch-restore",
        "batchRestoreCards",
    ),
    ("POST", "/boards/{boardId}/views", "createBoardView"),
    (
        "POST",
        "/boards/{boardId}/views/{viewId}/move",
        "moveBoardView",
    ),
    (
        "POST",
        "/boards/{boardId}/views/{viewId}/duplicate",
        "duplicateBoardView",
    ),
    (
        "POST",
        "/boards/{boardId}/views/{viewId}/card-moves",
        "moveViewCards",
    ),
    (
        "GET",
        "/boards/{boardId}/cards/{cardId}/comments",
        "getCardComments",
    ),
    (
        "POST",
        "/boards/{boardId}/cards/{cardId}/comments",
        "createCardComment",
    ),
    (
        "PATCH",
        "/boards/{boardId}/cards/{cardId}/comments/{commentId}",
        "patchCardComment",
    ),
    (
        "DELETE",
        "/boards/{boardId}/cards/{cardId}/comments/{commentId}",
        "deleteCardComment",
    ),
    (
        "POST",
        "/boards/{boardId}/cards/{cardId}/comments/{commentId}/restore",
        "restoreCardComment",
    ),
    (
        "GET",
        "/boards/{boardId}/cards/{cardId}/comments/{commentId}/versions",
        "getCardCommentVersions",
    ),
    (
        "POST",
        "/boards/{boardId}/cards/{cardId}/attachments",
        "createCardAttachment",
    ),
    ("PATCH", "/categories/{categoryId}", "patchCategory"),
    (
        "PUT",
        "/categories/{categoryId}/containers/{containerId}",
        "putCategoryContainer",
    ),
];

#[test]
fn the_workspace_route_inventory_covers_every_operation_in_the_contract() {
    let contract: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/openapi.json"
    )))
    .expect("the contract is JSON");
    let operations: BTreeSet<&str> = contract["paths"]
        .as_object()
        .expect("paths")
        .iter()
        .filter(|(path, _)| path.starts_with("/api/v1/w/{workspaceId}"))
        .flat_map(|(_, methods)| methods.as_object().expect("operations").values())
        .filter_map(|operation| operation["operationId"].as_str())
        .collect();
    assert_eq!(
        operations,
        WORKSPACE_ROUTES
            .iter()
            .map(|(_, _, operation)| *operation)
            .collect(),
        "every workspace route is exercised, not only its read routes"
    );
}

/// A suffix of `WORKSPACE_ROUTES` with every parameter filled in with an identifier of
/// nothing, and a part number with 1.
pub(super) fn fill_with_nothing(suffix: &str) -> String {
    let mut filled = suffix.replace("{partNumber}", "1");
    while let Some(start) = filled.find('{') {
        let end = filled[start..].find('}').expect("a parameter ends") + start;
        filled.replace_range(start..=end, UNKNOWN_ID);
    }
    filled
}

pub(super) use extra::archive_entries;

/// Who asks: somebody who is no member of the hidden workspace, a guest of it, or
/// somebody who is no member of it and only a guest where they work, the root
/// workspace, who makes no board there.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Asker {
    Outsider,
    Guest,
    GuestElsewhere,
}

/// The workspace is looked at before the body is read: a body that is no JSON changes
/// nothing of the refusal of a workspace the asker is not in (a guest is in it, and is
/// told). A body that is JSON but names nothing found answers 404 of its own, which hid
/// a refusal made after the body was read (`importArchive` did so once).
async fn compare_malformed_bodies(
    client: &Client,
    hidden: &str,
    (method, suffix, operation): (&str, &str, &str),
) -> Option<String> {
    let malformed = async |workspace: &str| {
        client
            .send_raw_body(
                method.parse().expect("an HTTP method"),
                &format!("/w/{workspace}{suffix}"),
                "{",
            )
            .await
    };
    let (hidden, absent) = (malformed(hidden).await, malformed(UNKNOWN_ID).await);
    (hidden.status != StatusCode::NOT_FOUND || answer(&hidden) != answer(&absent)).then(|| {
        format!(
            "{method} {suffix} ({operation}) with a body that is no JSON: hidden={:?}, absent={:?}",
            answer(&hidden),
            answer(&absent)
        )
    })
}

/// The body an operation of the table is sent with: one it takes, naming nothing there.
fn route_body(operation: &str, user_id: &str, workspace: &str) -> Value {
    match operation {
        "createCategory" => json!({"name": "mine", "userId": user_id, "workspaceId": workspace}),
        "reorderCategories"
        | "reorderCategoryContainers"
        | "getWorkspaceUsersById"
        | "resolveReferences" => json!([]),
        "importArchive" => json!({"uploadId": "missing-upload"}),
        "createImportUpload" => json!({"name": "archive.zip", "size": 0}),
        "appendCardMarkdown" => json!({"markdown": "said"}),
        _ => json!({}),
    }
}

/// The operations of `WORKSPACE_ROUTES` that answer a guest of a workspace that has
/// nothing for them with an empty list: the boards, the wikis and the containers of any
/// kind the guest is on, what
/// the references of no body name, what a reference of the guest may name, and what a
/// search of their bodies finds.
const EMPTY_FOR_A_GUEST: &[&str] = &[
    "getWorkspaceBoards",
    "listWorkspaceContainers",
    "getWikis",
    "getCourses",
    "getWorkspaceTodayReviews",
    "resolveReferences",
    "searchReferences",
    "searchWorkspaceContent",
];

/// Asks to make a board, alone and with a card, whose body names the workspace
/// `hidden_id`, under the root workspace's path, and again one that names a workspace
/// that is not there: the two are refused the same.
async fn compare_workspaces_in_a_body(
    client: &Client,
    asker: Asker,
    hidden_id: &str,
) -> Vec<String> {
    let mut violations = Vec::new();
    for bulk in [false, true] {
        let send = async |workspace: &str| {
            let board = json!({"id": "board-ref", "workspaceId": workspace, "visibility": "private", "title": "new"});
            let (path, body) = if bulk {
                (
                    &format!("{ROOT_WS}/containers-and-blocks"),
                    json!({"containers": [board], "blocks": [{"id": "card-ref", "containerId": "board-ref", "type": "card", "createdAt": 1, "updatedAt": 1}]}),
                )
            } else {
                (&format!("{ROOT_WS}/boards"), board)
            };
            client.post(path, &body).await
        };
        let hidden = send(hidden_id).await;
        let absent = send(UNKNOWN_ID).await;
        // Refused for the path's workspace (400), or the path's workspace is not there;
        // a guest of the path's workspace makes no board in it, whatever the body names.
        let refused = match asker {
            Asker::GuestElsewhere => hidden.status == StatusCode::FORBIDDEN,
            Asker::Outsider | Asker::Guest => matches!(
                hidden.status,
                StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND
            ),
        };
        if !refused || answer(&hidden) != answer(&absent) {
            violations.push(format!(
                "workspace in body (bulk={bulk}): {:?} versus {:?}",
                answer(&hidden),
                answer(&absent)
            ));
        }
    }
    violations
}

/// Asks every route of the inventory, as `user_id`, about the workspace `hidden_id`
/// and about one that is not there, and answers where the two differ or the hidden
/// one is no 404 (but what a guest of it is let see).
pub(super) async fn compare_workspace_routes(
    client: &Client,
    user_id: &str,
    asker: Asker,
    hidden_id: &str,
) -> Vec<String> {
    let mut violations = Vec::new();
    for (method, suffix, operation) in WORKSPACE_ROUTES {
        let suffix = fill_with_nothing(suffix);
        let body = |workspace: &str| route_body(operation, user_id, workspace);
        let send = async |workspace: &str| {
            Client::send(
                client
                    .request(
                        method.parse().expect("an HTTP method"),
                        &format!("/w/{workspace}{suffix}"),
                    )
                    .json(&body(workspace)),
            )
            .await
        };
        let hidden = send(hidden_id).await;
        let absent = send(UNKNOWN_ID).await;
        let lists_boards = EMPTY_FOR_A_GUEST.contains(operation);
        let equal = if lists_boards && asker == Asker::Guest {
            // A guest of the workspace is a member of it: the boards, and the wikis, are
            // those the guest is on, none here; and what no reference names is nothing.
            hidden.status == StatusCode::OK
                && answer(&hidden).1 == json!([])
                && absent.status == StatusCode::NOT_FOUND
        } else if *operation == "getCategories" && asker == Asker::Guest {
            // The guest's own sidebar: the default category, with no board here.
            hidden.status == StatusCode::OK
                && answer(&hidden).1[0]["containerMetadata"] == json!([])
                && absent.status == StatusCode::NOT_FOUND
        } else if *operation == "getWorkspaceOverview" && asker == Asker::Guest {
            // The workspace as its guest has it: theirs to be in, with no container here.
            hidden.status == StatusCode::OK
                && answer(&hidden).1["containers"] == json!([])
                && answer(&hidden).1["workspace"]["myWorkspaceRole"] == "guest"
                && absent.status == StatusCode::NOT_FOUND
        } else if *operation == "getWorkspaceLearningToday" && asker == Asker::Guest {
            // A guest not enrolled in any course sees empty today/overdue lists.
            hidden.status == StatusCode::OK
                && answer(&hidden).1["todayUnits"] == json!([])
                && answer(&hidden).1["overdueUnits"] == json!([])
                && absent.status == StatusCode::NOT_FOUND
        } else if *operation == "getWorkspaceUsers" && asker == Asker::Guest {
            // The people a guest sees: who is on its boards, none here, and itself.
            hidden.status == StatusCode::OK
                && answer(&hidden).1.as_array().map(Vec::len) == Some(1)
                && absent.status == StatusCode::NOT_FOUND
        } else if *operation == "getWorkspaceUsersById" && asker == Asker::Guest {
            // The workspace is there for its guest, and the empty list is refused.
            hidden.status == StatusCode::BAD_REQUEST && absent.status == StatusCode::NOT_FOUND
        } else if asker == Asker::Guest && REACHED_BY_ITEM.contains(operation) {
            // The workspace is there for its guest: what is refused is what the path
            // names, the operation, or the empty body; nothing is done.
            REFUSED_IN_REACH.contains(&hidden.status) && absent.status == StatusCode::NOT_FOUND
        } else if *operation == "exportWorkspaceArchive" && asker == Asker::Guest {
            // The guest's archive has the boards the guest is on, none here.
            hidden.status == StatusCode::OK
                && archive_entries(&hidden.body).len() == 1
                && absent.status == StatusCode::NOT_FOUND
        } else {
            answer(&hidden) == answer(&absent) && hidden.status == StatusCode::NOT_FOUND
        };
        if *method != "GET" && asker != Asker::Guest {
            let route = (*method, suffix.as_str(), *operation);
            violations.extend(compare_malformed_bodies(client, hidden_id, route).await);
        }
        if !equal {
            violations.push(format!(
                "{method} {suffix} ({operation}): hidden={:?}, absent={:?}",
                answer(&hidden),
                answer(&absent)
            ));
        }
    }
    violations.extend(compare_workspaces_in_a_body(client, asker, hidden_id).await);
    let listed: Vec<UserWorkspace> = client.get("/users/me/workspaces").await.ok_json();
    // A guest is a member, listed with the role; anybody else does not see it.
    let hidden: Vec<WorkspaceRole> = listed
        .iter()
        .filter(|workspace| workspace.id == hidden_id)
        .map(|workspace| workspace.role)
        .collect();
    let expected = match asker {
        Asker::Outsider | Asker::GuestElsewhere => vec![],
        Asker::Guest => vec![WorkspaceRole::Guest],
    };
    if hidden != expected {
        violations.push(format!(
            "getMyWorkspaces listed the hidden workspace as {hidden:?}"
        ));
    }
    violations
}

/// Every route under `/w/{workspaceId}` answers for a workspace the asker is not in, a
/// workspace of another organization, and a workspace the asker is a guest of as for one
/// that does not exist; but `getWorkspaceBoards`, `getCategories`, `exportWorkspaceArchive`,
/// the overview and the users of the workspace, which let a guest see, file and export the
/// boards it is on and their people.
#[tokio::test]
async fn every_workspace_route_hides_the_workspace_from_nonmembers_guests_and_other_org_users() {
    const HIDDEN: &str = HIDDEN_WORKSPACE_ID;
    let ts = Setup::native().start().await.init_basic().await;
    let guest = ts.add_user("workspace-guest").await;
    let guest_id = guest.me().await.id;
    ts.app
        .store()
        .add_workspace_member(HIDDEN_WORKSPACE_ID, &guest_id, WorkspaceRole::Guest)
        .await
        .expect("a workspace guest");
    let mut violations = compare_workspace_routes(
        &ts.client,
        &ts.client.me().await.id,
        Asker::Outsider,
        HIDDEN,
    )
    .await;
    violations.extend(compare_workspace_routes(&guest, &guest_id, Asker::Guest, HIDDEN).await);
    if db_type_of_the_tests() == DbType::Postgres {
        let user_id = ts.client2.me().await.id;
        for table in ["workspace_members", "organization_members"] {
            ts.app
                .store()
                .execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{user_id}'"))
                .await
                .expect("the other user leaves the first org");
        }
        let workspace = Workspace {
            id: new_id(),
            ..Workspace::default()
        };
        let org = ts
            .app
            .store()
            .create_organization("Other", &workspace)
            .await
            .expect("another organization");
        ts.app
            .store()
            .for_org(&org)
            .expect("an org ID")
            .add_workspace_member(&workspace.id, &user_id, WorkspaceRole::Member)
            .await
            .expect("the user works elsewhere");
        violations
            .extend(compare_workspace_routes(&ts.client2, &user_id, Asker::Outsider, HIDDEN).await);
    }
    ts.teardown().await;
    assert!(
        violations.is_empty(),
        "legacy route refusals differ from absent workspaces:\n{}",
        violations.join("\n")
    );
}

#[tokio::test]
async fn a_category_path_rejects_a_normal_board_from_another_workspace() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let categories: Vec<CategoryContainers> = ts
        .client
        .get(&format!("{ROOT_WS}/categories"))
        .await
        .ok_json();
    let category = &categories[0].category;
    let hidden = ts
        .client
        .post(
            &format!(
                "{ROOT_WS}/categories/{}/containers/{}",
                category.id, board.id
            ),
            &json!({}),
        )
        .await;
    let absent = ts
        .client
        .post(
            &format!(
                "{ROOT_WS}/categories/{}/containers/{UNKNOWN_ID}",
                category.id
            ),
            &json!({}),
        )
        .await;
    let after: Vec<CategoryContainers> = ts
        .client
        .get(&format!("{ROOT_WS}/categories"))
        .await
        .ok_json();
    let filed = after
        .iter()
        .flat_map(|entry| &entry.container_metadata)
        .any(|metadata| metadata.container_id == board.id);
    ts.teardown().await;
    assert_eq!(
        (answer(&hidden), filed),
        (answer(&absent), false),
        "a board of another workspace must not be filed through this workspace's path"
    );
}

/// Hiding and showing a board through a category compare the workspaces as filing
/// does: a board of another workspace the user works in answers as one that is not
/// there. A built-in template, the system organization's, is in no workspace of the
/// user's and is filed, hidden and shown in any.
#[tokio::test]
async fn a_category_path_hides_no_board_of_another_workspace_but_takes_a_template() {
    let ts = Setup::native().start().await.init_basic().await;
    ts.app
        .init_templates()
        .await
        .expect("the built-in templates are installed");
    let template = ts
        .app
        .store()
        .get_builtin_templates()
        .await
        .expect("the built-in templates")
        .remove(0);
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let categories: Vec<CategoryContainers> = ts
        .client
        .get(&format!("{ROOT_WS}/categories"))
        .await
        .ok_json();
    let category = format!(
        "{ROOT_WS}/categories/{}/containers",
        categories[0].category.id
    );
    let mut answers = Vec::new();
    for change in ["hide", "unhide"] {
        let hidden = ts
            .client
            .put(&format!("{category}/{}/{change}", board.id), &json!({}))
            .await;
        let absent = ts
            .client
            .put(&format!("{category}/{UNKNOWN_ID}/{change}"), &json!({}))
            .await;
        answers.push((change, answer(&hidden) == answer(&absent), absent.status));
    }
    let filed = ts
        .client
        .post(&format!("{category}/{}", template.id), &json!({}))
        .await
        .status;
    let hidden = ts
        .client
        .put(&format!("{category}/{}/hide", template.id), &json!({}))
        .await
        .status;
    let after: Vec<CategoryContainers> = ts
        .client
        .get(&format!("{ROOT_WS}/categories"))
        .await
        .ok_json();
    let template_hidden = after
        .iter()
        .flat_map(|entry| &entry.container_metadata)
        .find(|metadata| metadata.container_id == template.id)
        .map(|metadata| metadata.hidden);
    ts.teardown().await;
    assert_eq!(
        answers,
        [
            ("hide", true, StatusCode::NOT_FOUND),
            ("unhide", true, StatusCode::NOT_FOUND),
        ],
        "a board of another workspace answers as one that is not there"
    );
    assert_eq!(
        (filed, hidden, template_hidden),
        (StatusCode::NO_CONTENT, StatusCode::NO_CONTENT, Some(true)),
        "a built-in template is filed and hidden in the workspace's category"
    );
}

/// A board of a workspace the user is not in, where the user is still on the board,
/// is not there for them: a board role opens nothing outside the workspace. Through a
/// category of their own workspace it is not filed either, and answers as a board that
/// is not there.
#[tokio::test]
async fn a_category_path_rejects_a_board_the_user_is_on_in_a_workspace_they_are_not_in() {
    let ts = Setup::native().start().await.init_basic().await;
    let user_id = ts.client.me().await.id;
    let mut board = Container {
        id: new_id(),
        workspace_id: HIDDEN_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Private,
        ..Container::default()
    };
    // Stored past the rule that only who works in a workspace is made the admin of a
    // board there: the state is what the route must not be led astray by.
    ts.app
        .store()
        .insert_container(&mut board, &user_id)
        .await
        .expect("a board of a workspace the user is not in");
    ts.app
        .store()
        .save_member(&ContainerMember {
            container_id: board.id.clone(),
            user_id: user_id.clone(),
            role: fvoci_model::ContainerRole::Admin,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("the user is on a board of a workspace they are not in");
    let categories: Vec<CategoryContainers> = ts
        .client
        .get(&format!("{ROOT_WS}/categories"))
        .await
        .ok_json();
    let category = format!(
        "{ROOT_WS}/categories/{}/containers",
        categories[0].category.id
    );
    let seen = ts
        .client
        .get(&format!("{ROOT_WS}/containers/{}", board.id))
        .await
        .status;
    let filed = ts
        .client
        .post(&format!("{category}/{}", board.id), &json!({}))
        .await;
    let absent = ts
        .client
        .post(&format!("{category}/{UNKNOWN_ID}"), &json!({}))
        .await;
    ts.teardown().await;
    assert_eq!(
        seen,
        StatusCode::NOT_FOUND,
        "a board role outside the workspace opens nothing"
    );
    assert_eq!(
        answer(&filed),
        answer(&absent),
        "a board of a workspace the user is not in answers as one that is not there"
    );
}

/// A board of another workspace that was filed under a category before the categories
/// took the boards of their own workspace only does not keep the category from being
/// deleted: the boards of the workspace move to its default category, and that board
/// to the default category of its own workspace, which the user works in.
#[tokio::test]
async fn deleting_a_category_files_a_board_of_another_workspace_in_its_own() {
    let ts = Setup::native().start().await.init_basic().await;
    let user_id = ts.client.me().await.id;
    let ours = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let theirs = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let category = ts
        .app
        .create_category(fvoci_model::Category {
            name: "Earlier".to_owned(),
            user_id: user_id.clone(),
            workspace_id: ROOT_WORKSPACE_ID.to_owned(),
            ..fvoci_model::Category::default()
        })
        .await
        .expect("the category is made");
    ts.app
        .store()
        .add_update_category_container(
            &user_id,
            &category.id,
            &[ours.id.clone(), theirs.id.clone()],
        )
        .await
        .expect("both boards are filed as they could be before");
    let deleted = ts
        .client
        .delete(&format!("{ROOT_WS}/categories/{}", category.id))
        .await
        .status;
    let mut filed = Vec::new();
    for workspace_id in [ROOT_WORKSPACE_ID, TEST_WORKSPACE_ID] {
        let sidebar: Vec<CategoryContainers> = ts
            .client
            .get(&format!("/w/{workspace_id}/categories"))
            .await
            .ok_json();
        let boards: Vec<String> = sidebar
            .iter()
            .flat_map(|entry| &entry.container_metadata)
            .map(|metadata| metadata.container_id.clone())
            .collect();
        filed.push(boards);
    }
    ts.teardown().await;
    assert_eq!(deleted, StatusCode::NO_CONTENT, "the category is deleted");
    assert!(
        filed[0].contains(&ours.id) && !filed[0].contains(&theirs.id),
        "the workspace's board stayed in its sidebar, the other's left it: {filed:?}"
    );
    assert!(
        filed[1].contains(&theirs.id),
        "the other workspace's board is in its own sidebar: {filed:?}"
    );
}

/// A live link that lets members into the workspace, made by `maker`: its identifier.
async fn a_link_into(ts: &crate::common::TestServer, workspace_id: &str, maker: &str) -> String {
    ts.app
        .store()
        .create_invitation(&fvoci_store::NewLink {
            workspace_id,
            guest_of: None,
            token_hash: &format!("a link into {workspace_id}"),
            created_by: maker,
            now: 0,
            expires_at: i64::MAX,
            max_uses: None,
            mailed: None,
        })
        .await
        .expect("a link into the workspace")
        .id
}

/// What the routes of [`compare_resource_routes`] are asked about: the things of one
/// workspace that the paths name, and who is named in them.
pub(super) struct Items<'a> {
    pub(super) board: &'a Container,
    pub(super) card: &'a Block,
    /// A page of a wiki of the same workspace; its `board_id` is the wiki.
    pub(super) page: &'a Block,
    /// A comment on the page.
    pub(super) comment: &'a Block,
    /// An entry of a diary of the same workspace, where it is of a user's own space;
    /// its `container_id` is the diary. No other workspace has one.
    pub(super) entry: Option<&'a Block>,
    /// A unit of a course of the same workspace; its `container_id` is the course.
    pub(super) unit: &'a Block,
    /// A comment on the unit.
    pub(super) unit_comment: &'a Block,
    pub(super) file: &'a str,
    pub(super) upload_id: &'a str,
    pub(super) category: &'a str,
    /// A user of the items' workspace.
    pub(super) user: &'a str,
    /// A live link that lets members into the items' workspace.
    pub(super) invitation: &'a str,
    pub(super) team: &'a str,
    /// The subscriber of the path, and the one of the same path for nothing: the asker
    /// themselves for both when they are subscribed to the card.
    pub(super) subscriber: (&'a str, &'a str),
}

/// What a request of [`compare_resource_routes`] is about.
#[derive(Clone, Copy, PartialEq, Eq)]
enum About {
    /// The items.
    Them,
    /// Identifiers of nothing.
    Nothing,
}

impl Items<'_> {
    /// The path of `template` under `workspace` about these items, or about nothing:
    /// every parameter filled in.
    fn path(&self, template: &str, workspace: &str, about: About) -> String {
        let absent = about == About::Nothing;
        let pick = |there: &str, nothing: &'static str| -> String {
            if absent { nothing } else { there }.to_owned()
        };
        let subscriber = if absent {
            self.subscriber.1
        } else {
            self.subscriber.0
        };
        let comment_id = if template.contains("/courses/") {
            &self.unit_comment.id
        } else {
            &self.comment.id
        };
        let number = self.card.number.expect("the card has a number").to_string();
        template
            .trim_start_matches("/api/v1")
            .replace("{workspaceId}", workspace)
            .replace("{containerId}", &pick(&self.board.id, UNKNOWN_ID))
            .replace("{boardId}", &pick(&self.board.id, UNKNOWN_ID))
            .replace("{viewId}", &pick(&self.card.id, UNKNOWN_ID))
            .replace("{cardId}", &pick(&self.card.id, UNKNOWN_ID))
            .replace("{blockId}", &pick(&self.card.id, UNKNOWN_ID))
            .replace("{wikiId}", &pick(&self.page.container_id, UNKNOWN_ID))
            .replace("{pageId}", &pick(&self.page.id, UNKNOWN_ID))
            .replace("{commentId}", &pick(comment_id, UNKNOWN_ID))
            .replace(
                "{diaryId}",
                &pick(
                    self.entry.map_or(UNKNOWN_ID, |entry| &entry.container_id),
                    UNKNOWN_ID,
                ),
            )
            .replace(
                "{entryId}",
                &pick(self.entry.map_or(UNKNOWN_ID, |entry| &entry.id), UNKNOWN_ID),
            )
            .replace("{courseId}", &pick(&self.unit.container_id, UNKNOWN_ID))
            .replace("{unitId}", &pick(&self.unit.id, UNKNOWN_ID))
            .replace("{meetingId}", &pick(&self.unit.id, UNKNOWN_ID))
            .replace("{examId}", &pick(&self.unit.id, UNKNOWN_ID))
            .replace("{reviewId}", UNKNOWN_ID)
            .replace("{noteId}", UNKNOWN_ID)
            .replace("{monthDay}", "02-29")
            .replace("{fileId}", &pick(self.file, "missing-file"))
            .replace("{uploadId}", &pick(self.upload_id, "missing-upload"))
            .replace(
                "{userId}",
                &if template.contains("/subscriptions/") {
                    subscriber.to_owned()
                } else {
                    pick(self.user, UNKNOWN_ID)
                },
            )
            .replace("{invitationId}", &pick(self.invitation, UNKNOWN_ID))
            .replace("{teamId}", &pick(self.team, UNKNOWN_ID))
            .replace("{partNumber}", "1")
            .replace("{version}", "1")
            // The card's number on its board; of nothing, one that no card has.
            .replace("{number}", &pick(&number, "999999"))
            .replace("{leaseId}", UNKNOWN_ID)
            .replace("{categoryId}", &pick(self.category, UNKNOWN_ID))
    }
}

/// Asks every operation under `/w/{workspace}` whose path names something (and the
/// subscriptions, whose body names a block) about `items`, and again about nothing:
/// the answers must be the same, and none a success. Where both requests are the same
/// (the asker's own subscriptions), the answer must not name the card. The operations
/// come from the contract, so a new one is compared as it is added.
pub(super) async fn compare_resource_routes(
    client: &Client,
    workspace: &str,
    items: &Items<'_>,
) -> Vec<String> {
    let Items { card, user, .. } = items;
    let caller = client.me().await.id;
    let contract: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/openapi.json"
    )))
    .expect("the contract");
    let mut violations = Vec::new();
    for (template, methods) in contract["paths"].as_object().expect("paths") {
        let Some(rest) = template.strip_prefix("/api/v1/w/{workspaceId}") else {
            continue;
        };
        if !(rest.contains('{') || rest.starts_with("/subscriptions")) {
            continue;
        }
        for (method, operation) in methods.as_object().expect("operations") {
            let Some(operation) = operation["operationId"].as_str() else {
                continue;
            };
            // The contract writes methods in lower case, which HTTP does not.
            let method: reqwest::Method = method.to_uppercase().parse().expect("an HTTP method");
            let path = |absent: bool| {
                let about = if absent { About::Nothing } else { About::Them };
                items.path(template, workspace, about)
            };
            let block = |absent: bool| if absent { UNKNOWN_ID } else { &card.id };
            let body = |absent: bool| match operation {
                "patchBlocks" => {
                    json!({"blockIds": [block(absent)], "blockPatches": [{"title": "changed"}]})
                }
                "reorderCardProperties" | "createBlocks" => json!([]),
                "createCard" => json!({"title": "new card"}),
                "createBoardView" => json!({"title": "new view", "viewType": "board"}),
                "batchDeleteCards" | "batchRestoreCards" | "moveViewCards" => {
                    json!({"cardIds": [block(absent)]})
                }
                "createCardAttachment" => json!({"fileId": "missing-file"}),
                "copyContainerToWorkspace" => json!({"targetWorkspaceId": workspace}),
                "createUnit" => json!({"title": "new unit"}),
                "moveUnit" => json!({"parentId": ""}),
                "createCourseMeeting" => {
                    json!({"title": "new meeting", "startsAt": 1_800_000_000_000_i64})
                }
                "patchCourseMeeting" => json!({"title": "changed meeting"}),
                "putMeetingAttendance" => json!({"status": "present"}),
                "createCourseExam" => json!({"title": "new exam", "maxScore": 100.0}),
                "patchCourseExam" => json!({"title": "changed exam"}),
                "putMyExamScore" => json!({"score": 90.0}),
                "patchCard" | "patchContainer" | "patchBlock" | "patchUnit" => {
                    json!({"title": "changed"})
                }
                "createDiaryEntry" => json!({"date": "2020-02-29"}),
                "patchDiaryEntry" => json!({"mood": "good"}),
                "createUpload" => json!({"name": "new.bin", "size": 0}),
                "addMember" => json!({"userId": user, "role": "viewer"}),
                "setPublicLink" => json!({"enabled": true}),
                "holdFileLease" => json!({"leaseId": ""}),
                "createSubscription" => {
                    json!({"blockId": block(absent), "blockType": "card", "userId": caller})
                }
                _ => json!({}),
            };
            if path(false).contains('{') {
                violations.push(format!(
                    "{operation} ({method} {template}): a parameter is not filled in"
                ));
                continue;
            }
            let ask = async |absent: bool| {
                Client::send(
                    client
                        .request(method.clone(), &path(absent))
                        .json(&body(absent)),
                )
                .await
            };
            let (hidden, absent) = (ask(false).await, ask(true).await);
            let refused = if path(false) == path(true) && body(false) == body(true) {
                !hidden.text().contains(&card.id)
            } else {
                !hidden.status.is_success()
            };
            if !refused || answer(&hidden) != answer(&absent) {
                violations.push(format!(
                    "{operation} ({method} {template}): hidden={:?}, absent={:?}",
                    answer(&hidden),
                    answer(&absent)
                ));
            }
        }
    }
    violations
}

/// Takes the second user out of the first organization and makes them a member of a
/// workspace of a new one.
async fn move_to_another_organization(ts: &super::common::TestServer) {
    let user = ts.client2.me().await.id;
    for table in ["workspace_members", "organization_members"] {
        ts.app
            .store()
            .execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{user}'"))
            .await
            .expect("remove first-org admission");
    }
    let workspace = Workspace {
        id: new_id(),
        ..Workspace::default()
    };
    let org = ts
        .app
        .store()
        .create_organization("ID inventory", &workspace)
        .await
        .expect("another organization");
    ts.app
        .store()
        .for_org(&org)
        .expect("org ID")
        .add_workspace_member(&workspace.id, &user, WorkspaceRole::Member)
        .await
        .expect("admission elsewhere");
}

/// A new user who works in `workspace` and in no other: a user is stored in the
/// default workspace of the organization, the root one, and is taken out of it.
async fn user_of_only(ts: &super::common::TestServer, workspace: &str) -> String {
    let mut user = fvoci_model::User {
        id: new_id(),
        username: format!("only-{workspace}"),
        email: format!("only-{workspace}@sample.com"),
        ..fvoci_model::User::default()
    };
    let store = ts.app.store();
    store
        .create_user(&mut user)
        .await
        .expect("the user is stored");
    store
        .execute_unchecked(&format!(
            "DELETE FROM workspace_members WHERE user_id = '{}' AND workspace_id = '{ROOT_WORKSPACE_ID}'",
            user.id
        ))
        .await
        .expect("the user leaves the root workspace");
    store
        .add_workspace_member(workspace, &user.id, WorkspaceRole::Member)
        .await
        .expect("the user works in the workspace");
    user.id
}

/// A category of the owner in the hidden workspace.
async fn hidden_category(ts: &super::common::TestServer, owner: &str) -> String {
    ts.app
        .create_category(fvoci_model::Category {
            name: "Hidden category".to_owned(),
            user_id: owner.to_owned(),
            workspace_id: HIDDEN_WORKSPACE_ID.to_owned(),
            ..fvoci_model::Category::default()
        })
        .await
        .expect("a category of the hidden workspace")
        .id
}

/// Makes the user follow the card.
async fn follow(store: &fvoci_store::Store, card: &Block, user_id: &str) {
    store
        .create_subscription(fvoci_model::Subscription {
            block_type: BlockType::Card,
            block_id: card.id.clone(),
            user_id: user_id.to_owned(),
            notified_at: 0,
            created_at: 0,
            deleted_at: None,
        })
        .await
        .expect("the user follows the card");
}

/// A page of a new open wiki of the workspace, made by `owner`.
pub(super) async fn a_page_in(
    store: &fvoci_store::Store,
    workspace_id: &str,
    owner: &str,
) -> Block {
    let mut wiki = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Open,
        kind: fvoci_model::ContainerKind::Wiki,
        title: "Hidden wiki".to_owned(),
        ..Container::default()
    };
    store
        .insert_container_with_admin(&mut wiki, owner)
        .await
        .expect("a wiki of the workspace");
    let new = fvoci_model::NewPage {
        title: "Hidden page".to_owned(),
        ..fvoci_model::NewPage::default()
    };
    store
        .create_page(&wiki.id, &new, fvoci_model::PageLimits::default(), owner)
        .await
        .expect("its page")
        .page
}

/// A unit of a new open course of the workspace, made by `owner`.
pub(super) async fn a_unit_in(
    store: &fvoci_store::Store,
    workspace_id: &str,
    owner: &str,
) -> Block {
    let mut course = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Open,
        kind: fvoci_model::ContainerKind::Course,
        title: "Hidden course".to_owned(),
        ..Container::default()
    };
    store
        .insert_container_with_admin(&mut course, owner)
        .await
        .expect("a course of the workspace");
    let new = fvoci_model::NewUnit {
        title: "Hidden unit".to_owned(),
        ..fvoci_model::NewUnit::default()
    };
    store
        .create_unit(&course.id, &new, fvoci_model::CourseLimits::DEFAULT, owner)
        .await
        .expect("its unit")
        .unit
}

/// A comment on the page, by its maker.
pub(super) async fn a_comment_on(store: &fvoci_store::Store, page: &Block) -> Block {
    let mut comment = Block {
        id: new_id(),
        container_id: page.container_id.clone(),
        parent_id: page.id.clone(),
        block_type: BlockType::Comment,
        ..Block::default()
    };
    store
        .insert_block(&mut comment, &page.created_by)
        .await
        .expect("a comment on the page");
    comment
}

#[tokio::test]
async fn identifier_only_inventory_hides_resources_of_an_unjoined_workspace() {
    let ts = Setup::native().start().await.init_basic().await;
    let owner = user_of_only(&ts, HIDDEN_WORKSPACE_ID).await;
    let mut board = Container {
        id: new_id(),
        workspace_id: HIDDEN_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Open,
        title: "Hidden board".to_owned(),
        ..Container::default()
    };
    ts.app
        .store()
        .insert_container_with_admin(&mut board, &owner)
        .await
        .expect("a hidden workspace's board");
    let mut card = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title: "Hidden card".to_owned(),
        ..Block::default()
    };
    ts.app
        .store()
        .insert_block(&mut card, &owner)
        .await
        .expect("its card");
    let mut data = &b"hidden workspace file"[..];
    let file = ts
        .app
        .save_file(&mut data, &board.id, "hidden.txt")
        .await
        .expect("its file");
    let upload = ts
        .app
        .create_upload(
            &owner,
            &board.id,
            fvoci_server::app::FileToUpload {
                name: "hidden-upload.txt".to_owned(),
                size: 0,
                content_type: String::new(),
            },
        )
        .await
        .expect("a real upload owned by the hidden workspace's member");
    let guest = ts.add_user("id-inventory-guest").await;
    let guest_id = guest.me().await.id;
    ts.app
        .store()
        .add_workspace_member(HIDDEN_WORKSPACE_ID, &guest_id, WorkspaceRole::Guest)
        .await
        .expect("a guest who has no board invitation");
    let category = hidden_category(&ts, &owner).await;
    let invitation = a_link_into(&ts, HIDDEN_WORKSPACE_ID, &owner).await;
    let team = a_team_in(&ts, &board.workspace_id, &board.created_by).await;
    let page = a_page_in(ts.app.store(), &board.workspace_id, &board.created_by).await;
    let unit = a_unit_in(ts.app.store(), &board.workspace_id, &board.created_by).await;
    let comment = a_comment_on(ts.app.store(), &page).await;
    let unit_comment = a_comment_on(ts.app.store(), &unit).await;
    let items = Items {
        board: &board,
        card: &card,
        page: &page,
        comment: &comment,
        entry: None,
        unit: &unit,
        unit_comment: &unit_comment,
        file: &file,
        upload_id: &upload.id,
        category: &category,
        user: &owner,
        invitation: &invitation,
        team: &team,
        subscriber: (&owner, UNKNOWN_ID),
    };
    let mut violations = compare_resource_routes(&ts.client, ROOT_WORKSPACE_ID, &items).await;
    violations.extend(compare_resource_routes(&guest, ROOT_WORKSPACE_ID, &items).await);
    // A role in another organization must not change the answer of any ID route.
    if db_type_of_the_tests() == DbType::Postgres {
        move_to_another_organization(&ts).await;
        violations.extend(compare_resource_routes(&ts.client2, ROOT_WORKSPACE_ID, &items).await);
    }
    ts.teardown().await;
    assert!(
        violations.is_empty(),
        "ID-only route differences:\n{}",
        violations.join("\n")
    );
}

/// What is of one workspace is not there under another, also for somebody who works in
/// both and may do anything with it in its own: every operation that names something in
/// its path answers about a board, card, file, upload, category and user of the test
/// workspace under the root one as about nothing, and the asker's subscription to the
/// card is not among theirs there; so does a body or a query that names the board.
#[tokio::test]
async fn what_is_of_one_workspace_is_not_there_under_another() {
    let ts = Setup::native().start().await.init_basic().await;
    let me = ts.client.me().await.id;
    let elsewhere = user_of_only(&ts, TEST_WORKSPACE_ID).await;
    let mut board = Container {
        id: new_id(),
        workspace_id: TEST_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Open,
        title: "Sibling board".to_owned(),
        ..Container::default()
    };
    let store = ts.app.store();
    store
        .insert_container_with_admin(&mut board, &me)
        .await
        .expect("a board of the test workspace that the asker administers");
    let mut card = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title: "Sibling card".to_owned(),
        ..Block::default()
    };
    store.insert_block(&mut card, &me).await.expect("its card");
    let file = ts
        .app
        .save_file(
            &mut &b"sibling workspace file"[..],
            &board.id,
            "sibling.txt",
        )
        .await
        .expect("its file");
    let upload = ts
        .app
        .create_upload(
            &me,
            &board.id,
            fvoci_server::app::FileToUpload {
                name: "sibling-upload.txt".to_owned(),
                size: 0,
                content_type: String::new(),
            },
        )
        .await
        .expect("the asker's upload to it");
    let category = ts
        .app
        .create_category(fvoci_model::Category {
            name: "Sibling category".to_owned(),
            user_id: me.clone(),
            workspace_id: TEST_WORKSPACE_ID.to_owned(),
            ..fvoci_model::Category::default()
        })
        .await
        .expect("the asker's category there")
        .id;
    follow(store, &card, &me).await;
    let page = a_page_in(ts.app.store(), &board.workspace_id, &board.created_by).await;
    let unit = a_unit_in(ts.app.store(), &board.workspace_id, &board.created_by).await;
    let invitation = a_link_into(&ts, TEST_WORKSPACE_ID, &me).await;
    let team = a_team_in(&ts, &board.workspace_id, &board.created_by).await;
    let comment = a_comment_on(ts.app.store(), &page).await;
    let unit_comment = a_comment_on(ts.app.store(), &unit).await;
    let items = Items {
        board: &board,
        card: &card,
        page: &page,
        comment: &comment,
        entry: None,
        unit: &unit,
        unit_comment: &unit_comment,
        file: &file,
        upload_id: &upload.id,
        category: &category,
        user: &elsewhere,
        invitation: &invitation,
        team: &team,
        subscriber: (&me, &me),
    };
    assert_reachable_in_workspace(&ts.client, TEST_WORKSPACE_ID, &me, &items).await;
    let violations = compare_resource_routes(&ts.client, ROOT_WORKSPACE_ID, &items).await;
    // Named in a body or a query (the boards of a combined request, the board files are
    // copied from), the board is as one that does not exist too.
    let own = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    extra::compare_query_targets(&ts.client, &own, &board).await;
    ts.teardown().await;
    assert!(
        violations.is_empty(),
        "what is of the test workspace, asked for under the root one:\n{}",
        violations.join("\n")
    );
}

async fn assert_reachable_in_workspace(
    client: &Client,
    workspace_id: &str,
    me: &str,
    items: &Items<'_>,
) {
    for path in [
        format!("/w/{workspace_id}/containers/{}", items.board.id),
        format!(
            "/w/{workspace_id}/wikis/{}/pages/{}",
            items.page.container_id, items.page.id
        ),
        format!(
            "/w/{workspace_id}/courses/{}/units/{}",
            items.unit.container_id, items.unit.id
        ),
        format!("/w/{workspace_id}/cards/{}", items.card.id),
        format!("/w/{workspace_id}/uploads/{}", items.upload_id),
        format!("/w/{workspace_id}/users/{}", items.user),
    ] {
        client.get(&path).await.check_ok();
    }
    let subscriptions = client
        .get(&format!("/w/{workspace_id}/subscriptions/{me}"))
        .await;
    assert!(
        subscriptions.text().contains(&items.card.id),
        "followed in its workspace"
    );
}

/// A real group for every resource guard, including bodies that do not decode.
async fn a_team_in(ts: &crate::common::TestServer, workspace_id: &str, owner: &str) -> String {
    ts.app.store().execute_unchecked(&format!(
        "UPDATE workspace_members SET role = 'admin' WHERE workspace_id = '{workspace_id}' AND user_id = '{owner}'"
    )).await.expect("the fixture owner administers its workspace");
    ts.app
        .store()
        .create_team(workspace_id, "Resource guard group", owner)
        .await
        .expect("a team in the resource's workspace")
}
