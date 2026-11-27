//! Every operation of the contract asked by a plain member (no administrator of the
//! workspace or of the organization) of the first organization and of one made later by
//! signing up: each is answered with the same status in both.

use std::collections::BTreeMap;

use reqwest::Method;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};

use super::common::{Client, PASSWORD, Setup, TestServer};
use super::{DbType, OrgRole, WorkspaceRole, db_type_of_the_tests, open};

/// What the sweep names in one organization, made by its founder, and the plain member
/// who asks.
struct Named {
    member: Client,
    member_id: String,
    org: String,
    workspace: String,
    board: String,
    view: String,
    card: String,
    card_comment: String,
    /// A wiki for everybody in the workspace, which the member joined, a page of it, and
    /// a comment of the founder's on the page.
    wiki: String,
    page: String,
    comment: String,
    /// A course for everybody in the workspace, which the member joined, a unit of it, and
    /// a comment of the founder's on the unit.
    course: String,
    unit: String,
    course_comment: String,
    meeting: String,
    exam: String,
    /// Personal review and study note the member created on `unit`.
    review: String,
    note: String,
    file: String,
    upload: String,
    category: String,
    invitation: String,
    invitation_id: String,
    read_token: String,
    /// The read token of the wiki, which is shared too: the pages of a link are a wiki's.
    wiki_token: String,
    /// The read token of the course, which is shared too: the units of a link are a course's.
    course_token: String,
    /// A team of the workspace with a role on the board, which the member is in.
    team: String,
    /// What the member was told: that the founder named them in a card of the board.
    notification: String,
    /// A running time entry the member started on the card.
    time_entry: String,
}

/// What a plain member who is on the board is answered 404 in the first organization
/// too, for what the sweep names: one board and one card for every operation. Any
/// other 404 fails the test, as an answer of the second organization equal to a
/// wrong first one would hide a lookup in another organization.
const NOT_THERE_FOR_A_MEMBER: [&str; 52] = [
    // No workspace of an organization has a diary: a diary is of a user's own space.
    "getDiaries",
    "createDiary",
    "getDiaryDays",
    "getDiaryRuns",
    "getDiaryEntriesOn",
    "createDiaryEntry",
    "getDiaryEntry",
    "patchDiaryEntry",
    "deleteDiaryEntry",
    "getDeletedDiaryEntries",
    "restoreDiaryEntry",
    "getDiaryEntryBodyVersions",
    "keepDiaryEntryBodyVersion",
    "getDiaryEntryBodyVersion",
    "restoreDiaryEntryBodyVersion",
    // The organization's own routes are its administrators'.
    "createOrgWorkspace",
    "deleteOidcFormerProvider",
    "deleteOidcPending",
    "deleteOidcProvider",
    "deleteOrgWorkspace",
    "getOidcProvider",
    "getOrgApiTokens",
    "getOrgAudit",
    "getOrgMembers",
    "getOrgWorkspaces",
    "getOrganization",
    "joinOrgWorkspace",
    "leaveOrgWorkspace",
    "patchOrganization",
    "putOidcProvider",
    "putOrgRole",
    "removeOrgMember",
    "renameOrgWorkspace",
    "restoreOrgWorkspace",
    "revokeOrgApiToken",
    "startOidcTrial",
    // A token the member never made.
    "revokeApiToken",
    // A link the member did not make, and the empty body of the sweep.
    "acceptInvitation",
    "revokeInvitation",
    // The card is no comment, and has no versions of one.
    "getCommentVersions",
    // The board is not in the trash, nor the card, nor the wiki's page, nor its comment.
    "restoreContainer",
    "restoreBlock",
    "restorePage",
    "restorePageComment",
    "restoreCardComment",
    "restoreUnit",
    "restoreUnitComment",
    // The member's subscription is to the card, by its own identifier.
    "deleteSubscription",
    // The upload asked of is done with by then, or was never begun in parts.
    "completeUpload",
    "putUploadPart",
    "importArchive",
    // The member's account waits for no change of its address: the sweep asked for
    // none it could, with its empty body.
    "resendEmailChange",
];

/// The operations the contract publishes: method, path and identifier, in its order.
fn operations() -> Vec<(String, String, String)> {
    let contract: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/openapi.json"
    )))
    .expect("the contract is JSON");
    let mut operations = Vec::new();
    for (template, methods) in contract["paths"].as_object().expect("paths") {
        for (method, operation) in methods.as_object().expect("operations") {
            if let Some(id) = operation["operationId"].as_str() {
                operations.push((method.to_uppercase(), template.clone(), id.to_owned()));
            }
        }
    }
    // What takes the member off the board, or a thing away, goes after the rest, then
    // the member's leaving the organization, whose session stays, and what ends the
    // member's session last, so that everything else is asked first.
    let rank = |id: &str| match id {
        "logout" => 3,
        "changePassword" | "leaveOrganization" => 2,
        "leaveContainer"
        | "deleteContainer"
        | "deleteBlock"
        | "batchDeleteCards"
        | "deletePage"
        | "deleteUnit"
        | "deleteCourseMeeting"
        | "deleteCourseExam"
        | "deleteUnitReview"
        | "deleteUnitNote"
        | "deleteCourseSchedule"
        | "deleteContainersAndBlocks"
        | "deleteUpload"
        | "deleteCategory"
        | "deleteTimeEntry"
        | "removeWorkspaceMember"
        | "removeMember" => 1,
        _ => 0,
    };
    operations.sort_by_key(|(_, _, id)| rank(id));
    operations
}

impl Named {
    fn path(&self, template: &str) -> String {
        // The pages of a link are read by the wiki's link, the units by the course's link,
        // and its other routes by the board's.
        let template = if template.starts_with("/api/v1/shared/{containerId}/pages") {
            template.replacen("{containerId}", &self.wiki, 1)
        } else if template.starts_with("/api/v1/shared/{containerId}/units") {
            template.replacen("{containerId}", &self.course, 1)
        } else {
            template.to_owned()
        };
        let comment = if template.contains("/courses/") {
            &self.course_comment
        } else if template.contains("/cards/") {
            &self.card_comment
        } else {
            &self.comment
        };
        let entry_id = if template.starts_with("/api/v1/users/me/time/") {
            &self.time_entry
        } else {
            &self.card
        };
        template
            .trim_start_matches("/api/v1")
            .replace("{workspaceId}", &self.workspace)
            .replace("{orgId}", &self.org)
            .replace("{containerId}", &self.board)
            .replace("{boardId}", &self.board)
            .replace("{viewId}", &self.view)
            .replace("{cardId}", &self.card)
            .replace("{blockId}", &self.card)
            .replace("{wikiId}", &self.wiki)
            .replace("{pageId}", &self.page)
            .replace("{commentId}", comment)
            .replace("{courseId}", &self.course)
            .replace("{unitId}", &self.unit)
            .replace("{meetingId}", &self.meeting)
            .replace("{examId}", &self.exam)
            .replace("{reviewId}", &self.review)
            .replace("{noteId}", &self.note)
            // No workspace of an organization has a diary: the board named as one is
            // none, in either organization.
            .replace("{diaryId}", &self.board)
            .replace("{entryId}", entry_id)
            .replace("{monthDay}", "02-29")
            .replace("{fileId}", &self.file)
            .replace("{uploadId}", &self.upload)
            .replace("{userId}", &self.member_id)
            .replace("{categoryId}", &self.category)
            .replace("{invitationId}", &self.invitation_id)
            .replace("{teamId}", &self.team)
            .replace("{notificationId}", &self.notification)
            .replace("{partNumber}", "1")
            .replace("{version}", "1")
            // The board's card is its first.
            .replace("{number}", "1")
            // A lease never given: its release is the member's answer as an editor's or not.
            .replace("{leaseId}", "a-lease-never-given")
            // A token never made: taking it back is the member's answer as its owner's or
            // an administrator's.
            .replace("{tokenId}", "a-token-never-made")
    }

    fn body(&self, operation: &str) -> Value {
        match operation {
            "getWorkspaceUsersById" => json!([self.member_id]),
            "resolveReferences" => json!([self.card, self.page, self.unit]),
            "createBlocks" | "reorderCardProperties" => json!([]),
            "importArchive" => json!({ "uploadId": self.upload }),
            "previewInvitation" => json!({ "token": self.invitation }),
            "patchMe" => json!({ "customName": "a member" }),
            "patchUnitNote" => json!({ "content": "updated note" }),
            "startTimeEntry" => {
                json!({ "workspaceId": self.workspace, "blockId": self.card, "kind": "work" })
            }
            "createTimeEntry" => json!({
                "workspaceId": self.workspace,
                "blockId": self.unit,
                "kind": "lecture",
                "startedAt": 1_000,
                "endedAt": 2_000,
            }),
            "patchTimeEntry" => json!({ "note": "updated" }),
            _ => json!({}),
        }
    }

    /// Asks every operation as the member: the status of each, by its identifier.
    async fn sweep(&self) -> BTreeMap<String, u16> {
        // An answer that sends the member elsewhere (the sign-in callback's, to
        // `server_url`) is the answer: what is there is not the server's.
        let member = self.member.not_following_redirects();
        let mut answers = BTreeMap::new();
        for (method, template, operation) in operations() {
            let path = self.path(&template);
            assert!(
                !path.contains('{'),
                "unfilled parameter of {operation}: {path}"
            );
            let path = if template.starts_with("/api/v1/shared/{containerId}/pages") {
                format!("{path}?readToken={}", self.wiki_token)
            } else if template.starts_with("/api/v1/shared/{containerId}/units") {
                format!("{path}?readToken={}", self.course_token)
            } else if template.starts_with("/api/v1/shared/") {
                format!("{path}?readToken={}", self.read_token)
            } else {
                path
            };
            let request = member
                .request(method.parse().expect("a method"), &path)
                .json(&self.body(&operation));
            let response = Client::send(request).await;
            answers.insert(operation, response.status.as_u16());
        }
        answers
    }
}

/// Writes a body to the card: a version of it, which the member reads.
async fn write_a_body(founder: &Client, at: &str, card: &Value) {
    founder
        .patch(
            &format!("{at}/cards/{}", text(card, "id")),
            &json!({"readAt": card["updatedAt"], "body": {"type": "doc", "content": [
                {"type": "paragraph", "content": [{"type": "text", "text": "a body"}]}]}}),
        )
        .await
        .check_ok();
}

/// Shares `container` under workspace path `at` and returns its read token.
async fn share_token(founder: &Client, at: &str, container: &str) -> String {
    let sharing = founder
        .post(
            &format!("{at}/containers/{container}/public-link"),
            &json!({"enabled": true}),
        )
        .await
        .value();
    text(&sharing, "token")
}

/// Signs up `name` with an organization of that name, and makes, as its founder, what the sweep
/// names; then lets a plain member in by a link for members.
async fn organization(server: &TestServer, name: &str) -> Named {
    let founder = sign_up(server, name).await;
    let workspaces = founder.get("/users/me/workspaces").await.value();
    let workspace = text(&workspaces[0], "id");
    let org = text(&workspaces[0], "orgId");
    let at = format!("/w/{workspace}");
    let board = founder
        .post(
            &format!("{at}/boards"),
            &json!({"title": format!("{name} board"), "visibility": "open"}),
        )
        .await
        .value();
    let board = text(&board, "id");
    let view = founder
        .post(
            &format!("{at}/boards/{board}/views"),
            &json!({"title": format!("{name} view"), "viewType": "board"}),
        )
        .await
        .value();
    let view = text(&view["view"], "id");
    let card = founder
        .post(
            &format!("{at}/boards/{board}/cards"),
            &json!({"title": format!("{name} card")}),
        )
        .await
        .value();
    let card_comment = founder
        .post(
            &format!("{at}/boards/{board}/cards/{}/comments", text(&card, "id")),
            &json!({"body": {"type": "doc", "content": [{"type": "paragraph", "content": [{"type": "text", "text": "card comment"}]}]}}),
        )
        .await
        .value();
    let card_comment = text(&card_comment, "id");
    let (wiki, page, comment) = wiki_with_a_page(&founder, &at, name).await;
    let (course, unit, course_comment, meeting, exam) =
        course_with_a_unit(&founder, &at, name).await;
    write_a_body(&founder, &at, &card).await;
    let file = Client::send(
        founder
            .request(Method::POST, &format!("{at}/containers/{board}/files"))
            .multipart(Form::new().part("file", Part::bytes(b"of it".to_vec()).file_name("a.txt"))),
    )
    .await
    .value();
    let (invitation, member) = invited_member(server, &founder, &at, name).await;
    let token = text(&invitation, "token");
    let member_id = member.me().await.id;
    let categories = member.get(&format!("{at}/categories")).await.value();
    let read_token = share_token(&founder, &at, &board).await;
    let wiki_token = share_token(&founder, &at, &wiki).await;
    let course_token = share_token(&founder, &at, &course).await;
    // On the board, wiki and course, the member asks what their members are asked.
    for joined in [&board, &wiki, &course] {
        member
            .post(&format!("{at}/containers/{joined}/join"), &json!({}))
            .await
            .check_ok();
    }
    let (review, note) = member_review_and_note(&member, &at, &course, &unit).await;
    let notification = told_of_a_mention(server, &founder, &member, &at, &board).await;
    let card_id = text(&card, "id");
    let (upload, time_entry) =
        member_card_activity(&member, &at, &workspace, &board, &card_id, &member_id).await;
    let team = team_on_board(&founder, &at, &board, &member_id).await;
    plain_member(server, &member_id, &workspace).await;
    Named {
        member,
        member_id,
        category: text(&categories[0], "id"),
        view,
        card: card_id,
        card_comment,
        read_token,
        wiki_token,
        course_token,
        file: text(&file, "fileId"),
        upload,
        invitation: token,
        invitation_id: text(&invitation, "id"),
        org,
        workspace,
        board,
        wiki,
        page,
        comment,
        course,
        unit,
        course_comment,
        meeting,
        exam,
        review,
        note,
        team,
        notification,
        time_entry,
    }
}

async fn member_card_activity(
    member: &Client,
    at: &str,
    workspace: &str,
    board: &str,
    card_id: &str,
    member_id: &str,
) -> (String, String) {
    member
        .post(
            &format!("{at}/subscriptions"),
            &json!({"blockType": "card", "blockId": card_id, "userId": member_id}),
        )
        .await
        .check_created();
    let upload = member
        .post(
            &format!("{at}/containers/{board}/uploads"),
            &json!({"name": "later.bin", "size": 1}),
        )
        .await
        .value();
    let started_time = member
        .post(
            "/users/me/time/start",
            &json!({"workspaceId": workspace, "blockId": card_id, "kind": "work"}),
        )
        .await
        .value();
    (
        text(&upload, "uploadId"),
        text(&started_time["entry"], "id"),
    )
}

/// A course of the workspace at `at` for everybody in it, with a unit (two body versions),
/// a comment on it, a meeting, and an exam.
async fn course_with_a_unit(
    founder: &Client,
    at: &str,
    name: &str,
) -> (String, String, String, String, String) {
    let course = founder
        .post(
            &format!("{at}/courses"),
            &json!({"title": format!("{name} course"), "visibility": "open"}),
        )
        .await
        .value();
    let course = text(&course, "id");
    let unit = founder
        .post(
            &format!("{at}/courses/{course}/units"),
            &json!({"title": format!("{name} unit")}),
        )
        .await
        .value();
    let said = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": format!("{name} unit comment")}]}]});
    let at_unit = format!("{at}/courses/{course}/units/{}", text(&unit, "id"));
    let mut read_at = unit["updatedAt"].clone();
    let again = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": format!("{name} unit body again")}]}]});
    for body in [said.clone(), again] {
        let written = founder
            .patch(&at_unit, &json!({"readAt": read_at, "body": body}))
            .await;
        written.check_ok();
        read_at = written.value()["updatedAt"].clone();
    }
    let unit = text(&unit, "id");
    let comment = founder
        .post(
            &format!("{at}/courses/{course}/units/{unit}/comments"),
            &json!({ "body": said }),
        )
        .await
        .value();
    let meeting = founder
        .post(
            &format!("{at}/courses/{course}/meetings"),
            &json!({"title": format!("{name} meeting"), "startsAt": 1_800_000_000_000_i64}),
        )
        .await
        .value();
    let exam = founder
        .post(
            &format!("{at}/courses/{course}/exams"),
            &json!({"title": format!("{name} exam"), "maxScore": 100.0}),
        )
        .await
        .value();
    (
        course,
        unit,
        text(&comment, "id"),
        text(&meeting, "id"),
        text(&exam, "id"),
    )
}

/// Schedules one personal review and creates one personal note for `member` on `unit`.
async fn member_review_and_note(
    member: &Client,
    at: &str,
    course: &str,
    unit: &str,
) -> (String, String) {
    let review = member
        .post(
            &format!("{at}/courses/{course}/units/{unit}/reviews"),
            &json!({"intervalDays": 3}),
        )
        .await
        .value();
    let note = member
        .post(
            &format!("{at}/courses/{course}/units/{unit}/notes"),
            &json!({"kind": "question", "content": "personal note"}),
        )
        .await
        .value();
    (text(&review, "id"), text(&note, "id"))
}

/// The founder names the member in a card of the board, which the member reads, and
/// the server tells them: the identifier of what the member was told.
async fn told_of_a_mention(
    server: &TestServer,
    founder: &Client,
    member: &Client,
    at: &str,
    board: &str,
) -> String {
    let member_id = member.me().await.id;
    founder
        .post(
            &format!("{at}/boards/{board}/cards"),
            &json!({"title": "A card that names the member",
                "body": {"type": "doc", "content": [{"type": "paragraph", "content": [
                    {"type": "mention", "attrs": {"userId": member_id}}]}]}}),
        )
        .await
        .check_created();
    fvoci_server::app::tend_notifications(&server.app).await;
    let told = member.get("/notifications").await.value();
    assert_eq!(
        told.as_array().map(Vec::len),
        Some(1),
        "the member is told of the one mention: {told}"
    );
    text(&told[0], "id")
}

/// The invitation for members the founder makes in the workspace at `at`, and the
/// member who signs up by it and is logged in.
async fn invited_member(
    server: &TestServer,
    founder: &Client,
    at: &str,
    name: &str,
) -> (Value, Client) {
    let invitation = founder
        .post(&format!("{at}/invitations"), &json!({"role": "member"}))
        .await
        .value();
    let member_name = format!("{name}-member");
    let accepted = server
        .anonymous()
        .post(
            "/invitations/accept",
            &json!({"token": text(&invitation, "token"), "account": {"username": member_name,
                "email": format!("{member_name}@example.test"), "password": PASSWORD}}),
        )
        .await;
    assert!(accepted.status.is_success(), "{}", accepted.text());
    (invitation, log_in(server, &member_name).await)
}

/// A wiki of the workspace at `at` for everybody in it, with a page and a comment on it:
/// the identifiers of the three.
async fn wiki_with_a_page(founder: &Client, at: &str, name: &str) -> (String, String, String) {
    let wiki = founder
        .post(
            &format!("{at}/wikis"),
            &json!({"title": format!("{name} wiki"), "visibility": "open"}),
        )
        .await
        .value();
    let wiki = text(&wiki, "id");
    let page = founder
        .post(
            &format!("{at}/wikis/{wiki}/pages"),
            &json!({"title": format!("{name} page")}),
        )
        .await
        .value();
    let said = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": format!("{name} comment")}]}]});
    // Two bodies written to the page are two versions of it: the member reads the first
    // and goes back to it, which writes it to the page again.
    let at_page = format!("{at}/wikis/{wiki}/pages/{}", text(&page, "id"));
    let mut read_at = page["updatedAt"].clone();
    let again = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": format!("{name} body again")}]}]});
    for body in [said.clone(), again] {
        let written = founder
            .patch(&at_page, &json!({"readAt": read_at, "body": body}))
            .await;
        written.check_ok();
        read_at = written.value()["updatedAt"].clone();
    }
    let page = text(&page, "id");
    let comment = founder
        .post(
            &format!("{at}/wikis/{wiki}/pages/{page}/comments"),
            &json!({ "body": said }),
        )
        .await
        .value();
    (wiki, page, text(&comment, "id"))
}

/// The member of the workspace `workspace` asks as a plain member: no administrator of
/// the workspace or of the organization.
async fn plain_member(server: &TestServer, member_id: &str, workspace: &str) {
    let access = server
        .app
        .store()
        .resolve_workspace(member_id, workspace)
        .await
        .expect("the directory answers")
        .expect("the member is in the workspace");
    assert_eq!(
        (access.workspace_role, access.org_role),
        (WorkspaceRole::Member, Some(OrgRole::Member)),
        "a plain member asks"
    );
}

/// A team of the workspace at `at` with the member in it and a role on the board: the
/// member asks its teams' routes too.
async fn team_on_board(founder: &Client, at: &str, board: &str, member_id: &str) -> String {
    let team = founder
        .post(&format!("{at}/teams"), &json!({"name": "Design"}))
        .await
        .value();
    let team = text(&team, "id");
    founder
        .put(
            &format!("{at}/teams/{team}/members/{member_id}"),
            &json!({}),
        )
        .await
        .check_no_content();
    founder
        .put(
            &format!("{at}/containers/{board}/teams/{team}"),
            &json!({"role": "viewer"}),
        )
        .await
        .check_no_content();
    team
}

fn text(value: &Value, field: &str) -> String {
    value[field]
        .as_str()
        .unwrap_or_else(|| panic!("{field} in {value}"))
        .to_owned()
}

async fn sign_up(server: &TestServer, name: &str) -> Client {
    server
        .anonymous()
        .post(
            "/register",
            &json!({"username": name, "email": format!("{name}@example.test"),
                "password": PASSWORD, "organizationName": name}),
        )
        .await
        .check_created();
    log_in(server, name).await
}

async fn log_in(server: &TestServer, name: &str) -> Client {
    let logged_in = server
        .anonymous()
        .post("/login", &json!({"username": name, "password": PASSWORD}))
        .await;
    assert_eq!(logged_in.status.as_u16(), 204, "{name} signs in");
    Client::new(
        &server.base_url,
        &logged_in.session_cookie().expect("a session cookie"),
    )
}

/// Read operations a plain member reaches with 200 in their workspace in both organizations.
const MEMBER_READ_OPERATIONS: &[&str] = &[
    "getWorkspace",
    "getCategories",
    "getWorkspaceBoards",
    "searchWorkspaceBoards",
    "getContainer",
    "getBlocks",
    "getCards",
    "getCard",
    "getCardByNumber",
    "getWikis",
    "getPageTree",
    "getPage",
    "getCourses",
    "getUnitTree",
    "getUnit",
    "getMyCourseProgress",
    "getCourseProgressSummary",
    "listCourseMeetings",
    "listCourseExams",
    "listMyCourseReviews",
    "listMyUnitNotes",
    "getWorkspaceTodayReviews",
    "getCourseSchedule",
    "getWorkspaceLearningToday",
    "getUnitComments",
    "getUnitCommentVersions",
    "getUnitBodyVersions",
    "getUnitBodyVersion",
    "getPageComments",
    "getPageCommentVersions",
    "getPageBodyVersions",
    "getPageBodyVersion",
    "getCardBodyVersions",
    "getCardBodyVersion",
    "getUpload",
    "getSharedContainer",
    "getSharedCards",
    "getSharedPageTree",
    "getSharedPage",
    "getSharedUnitTree",
    "getSharedUnit",
    "getTemplates",
    "getWorkspaceUsers",
    "resolveReferences",
    "getReferencesTo",
    "searchReferences",
    "getMyMemberships",
    "getMyOrganizations",
    "getActiveTimeEntry",
    "listTimeEntries",
];

#[tokio::test]
async fn every_contract_operation_answers_a_plain_member_alike_in_both_organizations() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let server = Setup::local_mode()
        .configure(open)
        .on_an_empty_database()
        .with_templates()
        .start()
        .await;
    let first = organization(&server, "member-first").await;
    let second = organization(&server, "member-other").await;
    assert_ne!(first.org, second.org, "two organizations");
    let of_first = first.sweep().await;
    let of_second = second.sweep().await;
    assert_eq!(
        of_first.len(),
        operations().len(),
        "every operation of the contract is asked"
    );
    // What a member reads of their workspace and its board is there for them in the
    // second one too.
    for &read in MEMBER_READ_OPERATIONS {
        assert_eq!(
            of_second.get(read),
            Some(&200),
            "{read} for a member: {of_second:?}"
        );
    }
    let failed: Vec<_> = of_first
        .iter()
        .chain(&of_second)
        .filter(|(_, status)| **status >= 500)
        .collect();
    assert!(failed.is_empty(), "the server failed: {failed:?}");
    let unexpected: Vec<&String> = of_first
        .iter()
        .filter(|(operation, status)| {
            **status == 404 && !NOT_THERE_FOR_A_MEMBER.contains(&operation.as_str())
        })
        .map(|(operation, _)| operation)
        .collect();
    assert!(
        unexpected.is_empty(),
        "not there for a member of the first organization: {unexpected:?}"
    );
    let differing: Vec<String> = of_first
        .iter()
        .filter(|(operation, status)| of_second.get(*operation) != Some(status))
        .map(|(operation, status)| {
            format!("{operation}: {status} and {:?}", of_second.get(operation))
        })
        .collect();
    assert!(
        differing.is_empty(),
        "a plain member is answered otherwise in the second organization:\n{}",
        differing.join("\n")
    );
    server.teardown().await;
}
