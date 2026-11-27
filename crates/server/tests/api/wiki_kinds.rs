//! Which operations reach a board, which a wiki, which a diary, and which several.
//!
//! A diary is a board of the kind `diary`, in the workspace of a user's own space
//! alone: the operations on its entries reach it and nothing else, what is about the
//! people of a board (its members, its teams, its link, a copy of it) reaches boards and
//! wikis and no diary, and the container itself, its files, its place in the sidebar and
//! its archive are the same for all three.
//!
//! A wiki is a board of the kind `wiki`. An operation that is about what a board holds
//! (cards, views) answers for a wiki as for a board that is not there,
//! an operation on a wiki's pages answers so for a board, and what is the same for
//! both (the container, who is on it, its sharing, its files, its place in the sidebar
//! and the trash, a copy of it whole and its archive) reaches either. [`REACHED`] says which is which, in one place, and
//! the test asks every operation of the contract that names a board or something of
//! one: an operation that is added without a row fails it.

use std::collections::BTreeSet;

use fvoci_model::{ContainerKind, ContainerKinds};
use reqwest::Method;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TestServer, UNKNOWN_ID,
};

/// The set of container kinds an operation reaches by what it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Reaches(ContainerKinds);

impl Reaches {
    const BOARDS: Self = Self(ContainerKinds::BOARDS);
    const WIKIS: Self = Self(ContainerKinds::WIKIS);
    const DIARIES: Self = Self(ContainerKinds::DIARIES);
    const COURSES: Self = Self(ContainerKinds::COURSES);
    /// Boards, wikis and courses alike; a diary named there is not there.
    const SHARED: Self = Self(ContainerKinds::SHARED);
    /// Boards, wikis and courses alike; a diary named there is not there.
    const LINKED: Self = Self(ContainerKinds::LINKED);
    /// Boards, wikis and courses alike; a diary named there is not there.
    const COPIED: Self = Self(ContainerKinds::COPIED);
    /// Boards, wikis, diaries and courses alike.
    const EVERY: Self = Self(ContainerKinds::EVERY);

    /// Whether `kind` is reached by an operation with this set.
    fn holds(self, kind: ContainerKind) -> bool {
        self.0.holds(kind)
    }
}

/// Every operation whose path names a board, a wiki, a diary, a course, or something that
/// is of one (a block, a file, an upload), and the set of kinds it reaches. What names one
/// in its body or its query is asked in
/// [`what_names_a_wiki_in_a_body_or_a_query_is_refused_as_what_names_nothing`].
const REACHED: &[(&str, Reaches)] = &[
    // The container itself, in the trash too.
    ("getContainer", Reaches::EVERY),
    ("getMyContainerPermissions", Reaches::EVERY),
    ("patchContainer", Reaches::EVERY),
    ("deleteContainer", Reaches::EVERY),
    ("restoreContainer", Reaches::EVERY),
    // Who is on it (boards, wikis and courses; diaries have no people).
    ("getMembers", Reaches::SHARED),
    ("addMember", Reaches::SHARED),
    ("updateMember", Reaches::SHARED),
    ("removeMember", Reaches::SHARED),
    ("joinContainer", Reaches::SHARED),
    ("leaveContainer", Reaches::SHARED),
    ("getContainerTeams", Reaches::SHARED),
    ("setContainerTeam", Reaches::SHARED),
    ("removeContainerTeam", Reaches::SHARED),
    // Its sharing: a board's link reads its cards, a wiki's its pages.
    ("getPublicLink", Reaches::LINKED),
    ("setPublicLink", Reaches::LINKED),
    // Its files.
    ("uploadFile", Reaches::EVERY),
    ("getFile", Reaches::EVERY),
    ("getFileRecord", Reaches::EVERY),
    ("listContainerFiles", Reaches::EVERY),
    ("getFileVersions", Reaches::EVERY),
    ("getFileVersion", Reaches::EVERY),
    ("addFileVersion", Reaches::EVERY),
    ("restoreFileVersion", Reaches::EVERY),
    ("getFileLease", Reaches::EVERY),
    ("holdFileLease", Reaches::EVERY),
    ("releaseFileLease", Reaches::EVERY),
    ("createUpload", Reaches::EVERY),
    ("getUpload", Reaches::EVERY),
    ("putUploadPart", Reaches::EVERY),
    ("completeUpload", Reaches::EVERY),
    ("deleteUpload", Reaches::EVERY),
    // Whoever writes a card or a page watches it, and may stop.
    ("deleteSubscription", Reaches::LINKED),
    // Its place in the sidebar.
    ("setContainerCategory", Reaches::EVERY),
    ("putCategoryContainer", Reaches::EVERY),
    ("hideContainer", Reaches::EVERY),
    ("unhideContainer", Reaches::EVERY),
    // A copy of it whole, and its archive.
    ("duplicateContainer", Reaches::COPIED),
    ("copyContainerToWorkspace", Reaches::COPIED),
    ("exportContainerArchive", Reaches::EVERY),
    // What a board holds.
    ("reorderCardProperties", Reaches::BOARDS),
    ("createBoardView", Reaches::BOARDS),
    ("moveBoardView", Reaches::BOARDS),
    ("duplicateBoardView", Reaches::BOARDS),
    ("moveViewCards", Reaches::BOARDS),
    ("createBlocks", Reaches::BOARDS),
    ("getBlocks", Reaches::BOARDS),
    ("patchBlocks", Reaches::BOARDS),
    ("patchBlock", Reaches::BOARDS),
    ("deleteBlock", Reaches::BOARDS),
    ("restoreBlock", Reaches::BOARDS),
    ("duplicateBlock", Reaches::BOARDS),
    ("getCommentVersions", Reaches::BOARDS),
    ("createCard", Reaches::BOARDS),
    ("duplicateCard", Reaches::BOARDS),
    ("batchDeleteCards", Reaches::BOARDS),
    ("batchRestoreCards", Reaches::BOARDS),
    ("getCards", Reaches::BOARDS),
    ("getCardByNumber", Reaches::BOARDS),
    ("getCard", Reaches::BOARDS),
    ("patchCard", Reaches::BOARDS),
    ("appendCardMarkdown", Reaches::BOARDS),
    ("listCardFiles", Reaches::BOARDS),
    ("getCardComments", Reaches::BOARDS),
    ("createCardComment", Reaches::BOARDS),
    ("patchCardComment", Reaches::BOARDS),
    ("deleteCardComment", Reaches::BOARDS),
    ("restoreCardComment", Reaches::BOARDS),
    ("getCardCommentVersions", Reaches::BOARDS),
    ("createCardAttachment", Reaches::BOARDS),
    ("getCardBodyVersions", Reaches::BOARDS),
    ("getCardBodyVersion", Reaches::BOARDS),
    ("restoreCardBodyVersion", Reaches::BOARDS),
    ("keepCardBodyVersion", Reaches::BOARDS),
    ("getDeletedCards", Reaches::BOARDS),
    // What a wiki holds.
    ("getPageTree", Reaches::WIKIS),
    ("createPage", Reaches::WIKIS),
    ("getPage", Reaches::WIKIS),
    ("patchPage", Reaches::WIKIS),
    ("deletePage", Reaches::WIKIS),
    ("getPageComments", Reaches::WIKIS),
    ("createPageComment", Reaches::WIKIS),
    ("patchPageComment", Reaches::WIKIS),
    ("deletePageComment", Reaches::WIKIS),
    ("getPageCommentVersions", Reaches::WIKIS),
    ("restorePageComment", Reaches::WIKIS),
    ("getPageBodyVersions", Reaches::WIKIS),
    ("getPageBodyVersion", Reaches::WIKIS),
    ("restorePageBodyVersion", Reaches::WIKIS),
    ("keepPageBodyVersion", Reaches::WIKIS),
    ("movePage", Reaches::WIKIS),
    ("duplicatePage", Reaches::WIKIS),
    ("getDeletedPages", Reaches::WIKIS),
    ("restorePage", Reaches::WIKIS),
    // What a diary holds.
    ("getDiaryDays", Reaches::DIARIES),
    ("getDiaryRuns", Reaches::DIARIES),
    ("getDiaryEntriesOn", Reaches::DIARIES),
    ("createDiaryEntry", Reaches::DIARIES),
    ("getDiaryEntry", Reaches::DIARIES),
    ("patchDiaryEntry", Reaches::DIARIES),
    ("deleteDiaryEntry", Reaches::DIARIES),
    ("getDeletedDiaryEntries", Reaches::DIARIES),
    ("restoreDiaryEntry", Reaches::DIARIES),
    ("getDiaryEntryBodyVersions", Reaches::DIARIES),
    ("getDiaryEntryBodyVersion", Reaches::DIARIES),
    ("restoreDiaryEntryBodyVersion", Reaches::DIARIES),
    ("keepDiaryEntryBodyVersion", Reaches::DIARIES),
    // What a course holds.
    ("getUnitTree", Reaches::COURSES),
    ("createUnit", Reaches::COURSES),
    ("getUnit", Reaches::COURSES),
    ("patchUnit", Reaches::COURSES),
    ("deleteUnit", Reaches::COURSES),
    ("getUnitComments", Reaches::COURSES),
    ("createUnitComment", Reaches::COURSES),
    ("patchUnitComment", Reaches::COURSES),
    ("deleteUnitComment", Reaches::COURSES),
    ("getUnitCommentVersions", Reaches::COURSES),
    ("restoreUnitComment", Reaches::COURSES),
    ("getUnitBodyVersions", Reaches::COURSES),
    ("getUnitBodyVersion", Reaches::COURSES),
    ("restoreUnitBodyVersion", Reaches::COURSES),
    ("keepUnitBodyVersion", Reaches::COURSES),
    ("moveUnit", Reaches::COURSES),
    ("getDeletedUnits", Reaches::COURSES),
    ("restoreUnit", Reaches::COURSES),
    ("getMyCourseProgress", Reaches::COURSES),
    ("putUnitProgress", Reaches::COURSES),
    ("getCourseProgressSummary", Reaches::COURSES),
    ("listCourseMeetings", Reaches::COURSES),
    ("createCourseMeeting", Reaches::COURSES),
    ("patchCourseMeeting", Reaches::COURSES),
    ("deleteCourseMeeting", Reaches::COURSES),
    ("putMeetingAttendance", Reaches::COURSES),
    ("listCourseExams", Reaches::COURSES),
    ("createCourseExam", Reaches::COURSES),
    ("patchCourseExam", Reaches::COURSES),
    ("deleteCourseExam", Reaches::COURSES),
    ("putMyExamScore", Reaches::COURSES),
    ("deleteMyExamScore", Reaches::COURSES),
    ("publishCourseExamResults", Reaches::COURSES),
    ("unpublishCourseExamResults", Reaches::COURSES),
    ("listMyCourseReviews", Reaches::COURSES),
    ("scheduleUnitReview", Reaches::COURSES),
    ("completeUnitReview", Reaches::COURSES),
    ("deleteUnitReview", Reaches::COURSES),
    ("listMyUnitNotes", Reaches::COURSES),
    ("createUnitNote", Reaches::COURSES),
    ("patchUnitNote", Reaches::COURSES),
    ("deleteUnitNote", Reaches::COURSES),
    ("getCourseSchedule", Reaches::COURSES),
    ("putCourseSchedule", Reaches::COURSES),
    ("deleteCourseSchedule", Reaches::COURSES),
    ("redistributeCourseSchedule", Reaches::COURSES),
    ("redistributeMyCourseSchedule", Reaches::COURSES),
    ("putUnitSchedule", Reaches::COURSES),
    // What names a card, page or unit from a body.
    ("getReferencesTo", Reaches::LINKED),
    // What the holder of a read token reads.
    ("getSharedContainer", Reaches::LINKED),
    ("getSharedBlocks", Reaches::BOARDS),
    ("getSharedCards", Reaches::BOARDS),
    ("getSharedFile", Reaches::LINKED),
    ("getSharedFileRecord", Reaches::LINKED),
    ("getSharedPageTree", Reaches::WIKIS),
    ("getSharedPage", Reaches::WIKIS),
    ("getSharedUnitTree", Reaches::COURSES),
    ("getSharedUnit", Reaches::COURSES),
];

/// The parameters of a path that name a container or something that is of one.
const NAMING: [&str; 17] = [
    "{diaryId}",
    "{entryId}",
    "{courseId}",
    "{unitId}",
    "{meetingId}",
    "{examId}",
    "{reviewId}",
    "{noteId}",
    "{containerId}",
    "{boardId}",
    "{wikiId}",
    "{cardId}",
    "{blockId}",
    "{pageId}",
    "{commentId}",
    "{uploadId}",
    "{fileId}",
];

/// A board or a wiki of the first user with one of everything a path names; or nothing
/// at all, under identifiers of nothing.
#[derive(Debug, Clone)]
struct Things {
    /// The workspace they are in.
    workspace: String,
    container: String,
    /// A card of the board; a page of the wiki.
    block: String,
    /// A view of the board (otherwise an unknown identifier).
    view: String,
    /// A comment on the card; on the page.
    comment: String,
    /// A study meeting of a course (otherwise an unknown identifier).
    meeting: String,
    /// An exam of a course (otherwise an unknown identifier).
    exam: String,
    /// A review on a course unit; `UNKNOWN_ID` for other kinds.
    review: String,
    /// A note on a course unit; `UNKNOWN_ID` for other kinds.
    note: String,
    file: String,
    upload: String,
    /// The read token of the container, which is shared.
    token: String,
}

impl Things {
    fn nothing() -> Self {
        Self::nothing_in(ROOT_WORKSPACE_ID)
    }

    fn nothing_in(workspace: &str) -> Self {
        Self {
            workspace: workspace.to_owned(),
            container: UNKNOWN_ID.to_owned(),
            block: UNKNOWN_ID.to_owned(),
            view: UNKNOWN_ID.to_owned(),
            comment: UNKNOWN_ID.to_owned(),
            meeting: UNKNOWN_ID.to_owned(),
            exam: UNKNOWN_ID.to_owned(),
            review: UNKNOWN_ID.to_owned(),
            note: UNKNOWN_ID.to_owned(),
            file: "missing-file".to_owned(),
            upload: "missing-upload".to_owned(),
            token: "no-token".to_owned(),
        }
    }
}

/// The server, and who and what the requests name besides a board or a wiki.
struct World {
    ts: TestServer,
    me: String,
    other: String,
    team: String,
    category: String,
    /// The workspace of the first user's own space, and their category there.
    own: String,
    own_category: String,
}

impl World {
    async fn start() -> Self {
        let ts = Setup::local_mode().start().await.init_basic().await;
        let (me, other) = (ts.client.me().await.id, ts.client2.me().await.id);
        let team = ts
            .client
            .post(&format!("{ROOT_WS}/teams"), &json!({"name": "Readers"}))
            .await
            .value();
        let categories = ts
            .client
            .get(&format!("{ROOT_WS}/categories"))
            .await
            .value();
        let own = crate::own_space::own_workspace(&ts.client).await.id;
        let own_categories = ts.client.get(&format!("/w/{own}/categories")).await.value();
        Self {
            me,
            other,
            team: text(&team, "id"),
            category: text(&categories[0], "id"),
            own_category: text(&own_categories[0], "id"),
            own,
            ts,
        }
    }

    /// Makes a board, a wiki, or a course, with the second user and the team on it, a
    /// file, and an open upload; boards and wikis also have a link. A board holds a card
    /// and a comment, a wiki a page and a comment, and a course a unit.
    async fn make(&self, kind: ContainerKind) -> Things {
        let client = &self.ts.client;
        let made_at = match kind {
            ContainerKind::Wiki => format!("{ROOT_WS}/wikis"),
            ContainerKind::Course => format!("{ROOT_WS}/courses"),
            _ => format!("{ROOT_WS}/boards"),
        };
        let made = client
            .post(&made_at, &json!({"visibility": "private"}))
            .await;
        made.check_done();
        let container = text(&made.value(), "id");
        let at = format!("{ROOT_WS}/containers/{container}");
        let member = json!({"containerId": container, "userId": self.other, "role": "editor"});
        client
            .post(&format!("{at}/members"), &member)
            .await
            .check_done();
        let on_team = format!("{at}/teams/{}", self.team);
        let assigned = client.put(&on_team, &json!({"role": "viewer"})).await;
        assert!(assigned.status.is_success(), "{}", assigned.text());
        let file = Part::bytes(b"kept".to_vec()).file_name("kept.txt");
        let uploaded = Client::send(
            client
                .request(Method::POST, &format!("{at}/files"))
                .multipart(Form::new().part("file", file)),
        )
        .await;
        uploaded.check_done();
        let opened = client
            .post(
                &format!("{at}/uploads"),
                &json!({"name": "parts.bin", "size": 4}),
            )
            .await;
        opened.check_done();
        let token = if ContainerKinds::LINKED.holds(kind) {
            client
                .post(&format!("{at}/public-link"), &json!({"enabled": true}))
                .await
                .check_done();
            let sharing = client.get(&format!("{at}/public-link")).await.value();
            text(&sharing, "token")
        } else {
            "no-token".to_owned()
        };
        let (block, view, comment) = match kind {
            ContainerKind::Wiki => {
                let (b, c) = self.wiki_page_with_a_comment(&container).await;
                (b, UNKNOWN_ID.to_owned(), c)
            }
            ContainerKind::Course => {
                let (b, c) = self.course_unit_with_a_comment(&container).await;
                (b, UNKNOWN_ID.to_owned(), c)
            }
            _ => self.card_with_a_comment(&container).await,
        };
        let (meeting, exam, review, note) = if kind == ContainerKind::Course {
            let (m, e) = self.course_meeting_and_exam(&container).await;
            let (r, n) = self.course_review_and_note(&container, &block).await;
            (m, e, r, n)
        } else {
            (
                UNKNOWN_ID.to_owned(),
                UNKNOWN_ID.to_owned(),
                UNKNOWN_ID.to_owned(),
                UNKNOWN_ID.to_owned(),
            )
        };
        Things {
            workspace: ROOT_WORKSPACE_ID.to_owned(),
            container,
            block,
            view,
            comment,
            meeting,
            exam,
            review,
            note,
            file: text(&uploaded.value(), "fileId"),
            upload: text(&opened.value(), "uploadId"),
            token,
        }
    }

    async fn wiki_page_with_a_comment(&self, wiki: &str) -> (String, String) {
        let client = &self.ts.client;
        let pages = format!("{ROOT_WS}/wikis/{wiki}/pages");
        let page = client.post(&pages, &json!({"title": "a page"})).await;
        page.check_done();
        let page = page.value();
        // A body written to the page is a version of it, as the card's is: an
        // operation on a card's versions that reached it would find one.
        let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": "written"}
        ]}]});
        let page_id = text(&page, "id");
        client
            .patch(
                &format!("{pages}/{page_id}"),
                &json!({"readAt": page["updatedAt"], "body": body}),
            )
            .await
            .check_ok();
        let comment = client
            .post(
                &format!("{pages}/{page_id}/comments"),
                &json!({ "body": body }),
            )
            .await;
        comment.check_done();
        (page_id, text(&comment.value(), "id"))
    }

    async fn course_meeting_and_exam(&self, course: &str) -> (String, String) {
        let client = &self.ts.client;
        let meeting = client
            .post(
                &format!("{ROOT_WS}/courses/{course}/meetings"),
                &json!({"title": "study", "startsAt": 1_800_000_000_000_i64}),
            )
            .await;
        meeting.check_done();
        let exam = client
            .post(
                &format!("{ROOT_WS}/courses/{course}/exams"),
                &json!({"title": "midterm", "maxScore": 100.0}),
            )
            .await;
        exam.check_done();
        (text(&meeting.value(), "id"), text(&exam.value(), "id"))
    }

    async fn course_unit_with_a_comment(&self, course: &str) -> (String, String) {
        let client = &self.ts.client;
        let units = format!("{ROOT_WS}/courses/{course}/units");
        let unit = client.post(&units, &json!({"title": "a unit"})).await;
        unit.check_done();
        let unit = unit.value();
        let unit_id = text(&unit, "id");
        client
            .patch(
                &format!("{units}/{unit_id}"),
                &json!({"readAt": unit["updatedAt"], "body": said()}),
            )
            .await
            .check_ok();
        let comment = client
            .post(
                &format!("{units}/{unit_id}/comments"),
                &json!({ "body": said() }),
            )
            .await;
        comment.check_done();
        (unit_id, text(&comment.value(), "id"))
    }

    async fn course_review_and_note(&self, course: &str, unit: &str) -> (String, String) {
        let client = &self.ts.client;
        let review = client
            .post(
                &format!("{ROOT_WS}/courses/{course}/units/{unit}/reviews"),
                &json!({"intervalDays": 3}),
            )
            .await;
        review.check_done();
        let note = client
            .post(
                &format!("{ROOT_WS}/courses/{course}/units/{unit}/notes"),
                &json!({"kind": "question", "content": "a study question"}),
            )
            .await;
        note.check_done();
        (text(&review.value(), "id"), text(&note.value(), "id"))
    }

    async fn card_with_a_comment(&self, board: &str) -> (String, String, String) {
        let client = &self.ts.client;
        let at = format!("{ROOT_WS}/boards/{board}");
        let view_res = client
            .post(&format!("{at}/views"), &json!({"title": "Board"}))
            .await;
        view_res.check_done();
        let view = text(&view_res.value()["view"], "id");
        let card = client
            .post(&format!("{at}/cards"), &json!({"title": "a card"}))
            .await;
        card.check_done();
        let made = card.value();
        let card = text(&made, "id");
        let text_of = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
            {"type": "text", "text": "said"}
        ]}]});
        // A body written to the card is the first version of it.
        client
            .patch(
                &format!("{ROOT_WS}/cards/{card}"),
                &json!({"readAt": made["updatedAt"], "body": text_of}),
            )
            .await
            .check_ok();
        let comment = client
            .post(
                &format!("{at}/cards/{card}/comments"),
                &json!({"body": text_of}),
            )
            .await;
        comment.check_done();
        let subscribed = client
            .post(
                &format!("{ROOT_WS}/subscriptions"),
                &json!({
                    "blockId": card, "blockType": "card", "userId": self.me,
                }),
            )
            .await;
        subscribed.check_done();
        (card, view, text(&comment.value(), "id"))
    }

    /// Brings `things` to where the operation does something: what it brings back is
    /// in the trash first.
    async fn prepare(&self, operation: &str, things: &Things) {
        let at = format!("{ROOT_WS}/containers/{}", things.container);
        let deleted = match operation {
            "restoreContainer" => self.ts.client.delete(&at).await,
            "restoreBlock" | "batchRestoreCards" => {
                let block = format!(
                    "{ROOT_WS}/boards/{}/blocks/{}",
                    things.container, things.block
                );
                self.ts.client.delete(&block).await
            }
            "restoreCardComment" => {
                let comment = format!(
                    "{ROOT_WS}/boards/{}/cards/{}/comments/{}",
                    things.container, things.block, things.comment
                );
                self.ts.client.delete(&comment).await
            }
            "restorePage" => {
                let page = format!(
                    "{ROOT_WS}/wikis/{}/pages/{}",
                    things.container, things.block
                );
                self.ts.client.delete(&page).await
            }
            "restorePageComment" => {
                let comment = format!(
                    "{ROOT_WS}/wikis/{}/pages/{}/comments/{}",
                    things.container, things.block, things.comment
                );
                self.ts.client.delete(&comment).await
            }
            "restoreUnit" => {
                let unit = format!(
                    "{ROOT_WS}/courses/{}/units/{}",
                    things.container, things.block
                );
                self.ts.client.delete(&unit).await
            }
            "restoreUnitComment" => {
                let comment = format!(
                    "{ROOT_WS}/courses/{}/units/{}/comments/{}",
                    things.container, things.block, things.comment
                );
                self.ts.client.delete(&comment).await
            }
            _ => return,
        };
        // Whether it went is what the operation itself shows: a wiki's card cannot go.
        drop(deleted);
    }

    /// Sends the operation about `things`.
    async fn ask(
        &self,
        method: &Method,
        template: &str,
        operation: &str,
        things: &Things,
    ) -> ApiResponse {
        let block = if operation == "getCommentVersions" {
            &things.comment
        } else {
            &things.block
        };
        let path = template
            .trim_start_matches("/api/v1")
            .replace("{workspaceId}", &things.workspace)
            .replace("{containerId}", &things.container)
            .replace("{diaryId}", &things.container)
            .replace("{entryId}", block)
            .replace("{courseId}", &things.container)
            .replace("{unitId}", block)
            .replace("{meetingId}", &things.meeting)
            .replace("{examId}", &things.exam)
            .replace("{reviewId}", &things.review)
            .replace("{noteId}", &things.note)
            .replace("{monthDay}", "11-21")
            .replace("{boardId}", &things.container)
            .replace("{wikiId}", &things.container)
            .replace("{cardId}", block)
            .replace("{viewId}", &things.view)
            .replace("{blockId}", block)
            .replace("{pageId}", block)
            .replace("{commentId}", &things.comment)
            .replace("{fileId}", &things.file)
            .replace("{uploadId}", &things.upload)
            .replace(
                "{userId}",
                if template.contains("/subscriptions/") {
                    &self.me
                } else {
                    &self.other
                },
            )
            .replace("{teamId}", &self.team)
            .replace(
                "{categoryId}",
                if things.workspace == self.own {
                    &self.own_category
                } else {
                    &self.category
                },
            )
            .replace("{partNumber}", "1")
            .replace("{version}", "1")
            // The board's card is its first.
            .replace("{number}", "1")
            .replace("{leaseId}", "no-lease");
        // The one operation of the table that asks its query for something.
        let path = if operation == "getDiaryDays" {
            format!("{path}?from=2026-11-01&to=2026-11-30")
        } else {
            path
        };
        assert!(
            !path.contains('{'),
            "{operation}: a parameter of {path} is not filled in"
        );
        if template.starts_with("/api/v1/shared/") {
            let path = format!("{path}?readToken={}", things.token);
            return self.ts.anonymous().get(&path).await;
        }
        let request = self.ts.client.request(method.clone(), &path);
        Client::send(match operation {
            "uploadFile" => {
                let file = Part::bytes(b"more".to_vec()).file_name("more.txt");
                request.multipart(Form::new().part("file", file))
            }
            "putUploadPart" => request.body(b"part".to_vec()),
            _ => request.json(&self.body(operation, things)),
        })
        .await
    }

    /// The body an operation is sent with: one it takes.
    fn body(&self, operation: &str, things: &Things) -> Value {
        match operation {
            "patchContainer" | "patchBlock" | "patchCard" | "patchPage" | "patchUnit" => {
                json!({"title": "changed"})
            }
            "createUnit" => json!({"title": "another unit"}),
            "moveUnit" => json!({"parentId": ""}),
            "putUnitProgress" => json!({"completed": true, "lastPosition": "section-1"}),
            "createCourseMeeting" => {
                json!({"title": "another study", "startsAt": 1_800_000_000_000_i64})
            }
            "patchCourseMeeting" => json!({"title": "changed study"}),
            "putMeetingAttendance" => json!({"status": "present"}),
            "createCourseExam" => json!({"title": "another exam", "maxScore": 100.0}),
            "patchCourseExam" => json!({"title": "changed exam"}),
            "putMyExamScore" => json!({"score": 90.0}),
            "scheduleUnitReview" => json!({"intervalDays": 7}),
            "createUnitNote" => json!({"kind": "mistake", "content": "check edge case"}),
            "patchUnitNote" => json!({"kind": "review", "content": "updated note"}),
            "putCourseSchedule" => {
                json!({"startDate": "2026-11-01", "targetDate": "2026-11-30", "studyDays": 31})
            }
            "redistributeCourseSchedule" | "redistributeMyCourseSchedule" => {
                json!({"fromDate": "2026-11-01", "targetDate": "2026-11-30", "studyDays": 31})
            }
            "putUnitSchedule" => json!({"plannedDate": "2026-11-24"}),
            "createDiaryEntry" => json!({"date": "2020-02-29"}),
            "patchDiaryEntry" => json!({"mood": "good"}),
            "addMember" | "updateMember" => {
                json!({"containerId": things.container, "userId": self.other, "role": "viewer"})
            }
            "setContainerTeam" => json!({"role": "editor"}),
            "setPublicLink" => json!({"enabled": false}),
            "createUpload" => json!({"name": "more.bin", "size": 4}),
            // The file itself is no version of it: refused as what it is, where the file is.
            "addFileVersion" => json!({"baseVersion": 1, "fileId": things.file}),
            "restoreFileVersion" => json!({"baseVersion": 1}),
            "holdFileLease" => json!({"leaseId": ""}),
            "patchBlocks" => {
                json!({"blockIds": [things.block], "blockPatches": [{"title": "changed"}]})
            }
            "createBlocks" | "reorderCardProperties" => json!([]),
            "createCard" => json!({"title": "another card"}),
            "createBoardView" => json!({"title": "Board view"}),
            "moveBoardView" => json!({"after": null}),
            "moveViewCards" | "batchDeleteCards" | "batchRestoreCards" => {
                json!({"cardIds": [things.block]})
            }
            "createCardAttachment" => json!({"fileId": things.file, "title": "kept.txt"}),
            "appendCardMarkdown" => json!({"markdown": "said"}),
            "copyContainerToWorkspace" => json!({"targetWorkspaceId": things.workspace}),
            "createCardComment" | "createPageComment" | "createUnitComment" => {
                json!({"body": said()})
            }
            "patchCardComment" | "patchPageComment" | "patchUnitComment" => {
                json!({"body": said(), "readAt": 1})
            }
            _ => json!({}),
        }
    }
}

/// What the fixtures need of an answer: that the request did what it was sent to do.
trait Done {
    fn check_done(&self);
}

impl Done for ApiResponse {
    #[track_caller]
    fn check_done(&self) {
        assert!(self.status.is_success(), "{}: {}", self.status, self.text());
    }
}

/// A text, as a body or a comment says it.
fn said() -> Value {
    json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": "said"}
    ]}]})
}

fn text(value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} in {value}"))
        .to_owned()
}

/// The operations of the contract whose path names a board, a wiki or something of
/// one: method, path, operation.
fn operations_that_name_one() -> Vec<(Method, String, String)> {
    let contract: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../contracts/openapi.json"
    )))
    .expect("the contract is JSON");
    let mut found = Vec::new();
    for (path, methods) in contract["paths"].as_object().expect("paths") {
        // Personal time entry routes use `{entryId}` for a time entry, not a diary entry.
        if path.starts_with("/api/v1/users/me/time/")
            || !NAMING.iter().any(|parameter| path.contains(parameter))
        {
            continue;
        }
        for (method, operation) in methods.as_object().expect("operations") {
            if let Some(operation) = operation["operationId"].as_str() {
                let method = method.to_uppercase().parse().expect("an HTTP method");
                found.push((method, path.clone(), operation.to_owned()));
            }
        }
    }
    found
}

#[test]
fn the_table_has_a_row_for_every_operation_that_names_a_board_and_for_no_other() {
    let named: BTreeSet<String> = operations_that_name_one()
        .into_iter()
        .map(|(_, _, operation)| operation)
        .collect();
    let rows: BTreeSet<String> = REACHED.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(rows.len(), REACHED.len(), "an operation has one row");
    assert_eq!(
        rows, named,
        "an operation whose path names a board, a wiki or something of one has a row \
         that says which kinds it reaches"
    );
}

/// Every operation that names a board or something of one is asked about a wiki, about
/// a board and about nothing, each time about ones made for it. One that reaches boards
/// answers for the wiki, and for what is the wiki's, exactly as for nothing; one that
/// reaches wikis answers so for the board; one that reaches both does to the wiki what
/// it does to the board.
#[tokio::test]
async fn every_operation_reaches_the_kinds_of_its_row_and_answers_for_the_other_as_for_nothing() {
    let world = World::start().await;
    let mut wrong = Vec::new();
    for (method, template, operation) in operations_that_name_one() {
        let Some((_, reaches)) = REACHED.iter().find(|(name, _)| *name == operation) else {
            continue;
        };
        let (board, wiki, course) = (
            world.make(ContainerKind::Board).await,
            world.make(ContainerKind::Wiki).await,
            world.make(ContainerKind::Course).await,
        );
        for things in [&board, &wiki, &course] {
            world.prepare(&operation, things).await;
        }
        let ask = async |things: &Things| world.ask(&method, &template, &operation, things).await;
        let nothing = ask(&Things::nothing()).await;
        let (of_board, of_wiki, of_course) =
            (ask(&board).await, ask(&wiki).await, ask(&course).await);
        let matches_row = |kind: ContainerKind, answer: &ApiResponse| {
            if reaches.holds(kind) {
                answer.problem() != nothing.problem()
            } else {
                answer.problem() == nothing.problem() && !answer.status.is_success()
            }
        };
        let right = matches_row(ContainerKind::Board, &of_board)
            && matches_row(ContainerKind::Wiki, &of_wiki)
            && matches_row(ContainerKind::Course, &of_course);
        // A user has only so many uploads open at once.
        for things in [&board, &wiki, &course] {
            let upload = format!("{ROOT_WS}/uploads/{}", things.upload);
            drop(world.ts.client.delete(&upload).await);
        }
        if !right {
            wrong.push(format!(
                "{operation} ({method} {template}) reaches {reaches:?}: board={:?}, wiki={:?}, \
                 course={:?}, nothing={:?}",
                of_board.problem(),
                of_wiki.problem(),
                of_course.problem(),
                nothing.problem()
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    world.ts.teardown().await;
}

/// A board, a wiki and a diary of the first user in the workspace of their own space,
/// each with a file, an open upload, and what it holds written once: a card, a page, the
/// entry of a day, each with a body. Nobody else is on any of them, and none has a link:
/// a space has no people.
async fn make_in_own_space(world: &World, own: &str) -> [Things; 4] {
    let client = &world.ts.client;
    let ws = format!("/w/{own}");
    let body = json!({"readAt": 0, "body": said()});
    let written = |made: &Value| {
        let mut body = body.clone();
        body["readAt"] = made["updatedAt"].clone();
        body
    };
    let mut made = Vec::new();
    for kind in ["boards", "wikis", "diaries", "courses"] {
        let new = if kind == "diaries" {
            json!({})
        } else {
            json!({"visibility": "private"})
        };
        let container = client.post(&format!("{ws}/{kind}"), &new).await;
        container.check_done();
        let container = text(&container.value(), "id");
        let at = format!("{ws}/containers/{container}");
        let block = match kind {
            "boards" => {
                let card = client
                    .post(
                        &format!("{ws}/boards/{container}/cards"),
                        &json!({"title": "a card"}),
                    )
                    .await;
                card.check_done();
                let id = text(&card.value(), "id");
                let patched = client
                    .patch(&format!("{ws}/cards/{id}"), &written(&card.value()))
                    .await;
                patched.check_ok();
                id
            }
            "wikis" => {
                let pages = format!("{ws}/wikis/{container}/pages");
                let page = client.post(&pages, &json!({"title": "a page"})).await;
                page.check_done();
                let id = text(&page.value(), "id");
                client
                    .patch(&format!("{pages}/{id}"), &written(&page.value()))
                    .await
                    .check_ok();
                id
            }
            "courses" => {
                let units = format!("{ws}/courses/{container}/units");
                let unit = client.post(&units, &json!({"title": "a unit"})).await;
                unit.check_done();
                let id = text(&unit.value(), "id");
                client
                    .patch(&format!("{units}/{id}"), &written(&unit.value()))
                    .await
                    .check_ok();
                id
            }
            _ => {
                let entries = format!("{ws}/diaries/{container}/entries");
                let entry = client.post(&entries, &json!({"date": "2024-11-21"})).await;
                entry.check_done();
                let id = text(&entry.value(), "id");
                client
                    .patch(&format!("{entries}/{id}"), &written(&entry.value()))
                    .await
                    .check_ok();
                id
            }
        };
        let file = Part::bytes(b"kept".to_vec()).file_name("kept.txt");
        let uploaded = Client::send(
            client
                .request(Method::POST, &format!("{at}/files"))
                .multipart(Form::new().part("file", file)),
        )
        .await;
        uploaded.check_done();
        let opened = client
            .post(
                &format!("{at}/uploads"),
                &json!({"name": "parts.bin", "size": 4}),
            )
            .await;
        opened.check_done();
        made.push(Things {
            container,
            block,
            file: text(&uploaded.value(), "fileId"),
            upload: text(&opened.value(), "uploadId"),
            ..Things::nothing_in(own)
        });
    }
    made.try_into().expect("four were made")
}

/// In the workspace of a user's own space, where a board, a wiki and a diary are side
/// by side, every operation that names one of them is asked about a diary made for it
/// and about nothing. One whose row holds a diary does something of its own for the
/// diary; any other answers for the diary, and for what is the diary's, exactly as for
/// nothing, and does nothing. An operation on a diary's entries answers so for the
/// board and the wiki beside it.
#[tokio::test]
async fn a_diary_is_there_to_the_operations_of_its_rows_and_to_no_other() {
    let world = World::start().await;
    let client = &world.ts.client;
    let own = world.own.clone();
    let mut wrong = Vec::new();
    for (method, template, operation) in operations_that_name_one() {
        let Some((_, reaches)) = REACHED.iter().find(|(name, _)| *name == operation) else {
            continue;
        };
        let [board, wiki, diary, course] = make_in_own_space(&world, &own).await;
        let entry = format!(
            "/w/{own}/diaries/{}/entries/{}",
            diary.container, diary.block
        );
        match operation.as_str() {
            "restoreContainer" => {
                let at = format!("/w/{own}/containers/{}", diary.container);
                client.delete(&at).await.check_done();
            }
            "restoreDiaryEntry" => client.delete(&entry).await.check_done(),
            _ => {}
        }
        let ask = async |things: &Things| world.ask(&method, &template, &operation, things).await;
        let nothing = ask(&Things::nothing_in(&own)).await;
        let of_diary = ask(&diary).await;
        let as_nothing = |answer: &ApiResponse| {
            answer.problem() == nothing.problem() && !answer.status.is_success()
        };
        let mut right = if reaches.holds(ContainerKind::Diary) {
            of_diary.problem() != nothing.problem()
        } else {
            as_nothing(&of_diary)
        };
        if *reaches == Reaches::DIARIES {
            right = right
                && of_diary.status.is_success()
                && as_nothing(&ask(&board).await)
                && as_nothing(&ask(&wiki).await)
                && as_nothing(&ask(&course).await);
        }
        // A user has only so many uploads open at once, and a space so many diaries.
        for things in [&board, &wiki, &diary, &course] {
            let upload = format!("/w/{own}/uploads/{}", things.upload);
            drop(client.delete(&upload).await);
            let at = format!("/w/{own}/containers/{}", things.container);
            drop(client.delete(&at).await);
        }
        if !right {
            wrong.push(format!(
                "{operation} ({method} {template}) reaches {reaches:?}: diary={:?}, nothing={:?}",
                of_diary.problem(),
                nothing.problem()
            ));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    world.ts.teardown().await;
}

/// The lists of a user's own space are of one kind each, the diary's among them, and no
/// workspace of an organization of people lists a diary or makes one.
#[tokio::test]
async fn the_lists_of_an_own_space_are_of_their_kinds() {
    let world = World::start().await;
    let client = &world.ts.client;
    let own = world.own.clone();
    let ws = format!("/w/{own}");
    let [board, wiki, diary, course] = make_in_own_space(&world, &own).await;
    for (list, of) in [
        ("boards", &board),
        ("wikis", &wiki),
        ("diaries", &diary),
        ("courses", &course),
    ] {
        let listed = client.get(&format!("{ws}/{list}")).await;
        assert_eq!(ids(&listed), std::slice::from_ref(&of.container), "{list}");
    }
    let diaries = client.get(&format!("{ws}/diaries")).await.value();
    assert_eq!(
        (&diaries[0]["kind"], &diaries[0]["visibility"]),
        (&json!("diary"), &json!("private"))
    );
    for things in [&board, &wiki, &diary, &course] {
        let at = format!("{ws}/containers/{}", things.container);
        client
            .patch(&at, &json!({"title": "findable"}))
            .await
            .check_done();
    }
    let found = client.get(&format!("{ws}/boards/search?q=findable")).await;
    assert_eq!(ids(&found), std::slice::from_ref(&board.container));
    // The workspace of an organization of people lists no diary, and makes none.
    let elsewhere = client.get(&format!("{ROOT_WS}/diaries")).await;
    let nowhere = client.get(&format!("/w/{UNKNOWN_ID}/diaries")).await;
    assert_eq!(elsewhere.problem(), nowhere.problem());
    assert_eq!(elsewhere.status, reqwest::StatusCode::NOT_FOUND);
    let made = client.post(&format!("{ROOT_WS}/diaries"), &json!({})).await;
    assert_eq!(made.problem(), nowhere.problem());

    world.ts.teardown().await;
}

/// What makes or changes boards by a body takes no diary: a board is not made a diary
/// of, one at a time or with blocks, what changes or deletes boards together finds no
/// diary, and an entry is no block to make on a board.
#[tokio::test]
async fn what_names_a_diary_in_a_body_is_refused() {
    let world = World::start().await;
    let client = &world.ts.client;
    let own = world.own.clone();
    let ws = format!("/w/{own}");
    let [board, _, diary, _] = make_in_own_space(&world, &own).await;
    let one = client
        .post(
            &format!("{ws}/boards"),
            &json!({"visibility": "private", "kind": "diary"}),
        )
        .await;
    let as_diary =
        json!({"id": "made", "visibility": "private", "kind": "diary", "title": "no board"});
    let many = client
        .post(
            &format!("{ws}/containers-and-blocks"),
            &json!({"containers": [as_diary], "blocks": [{
                "id": "card", "containerId": "made", "type": "card", "createdAt": 1, "updatedAt": 1,
            }]}),
        )
        .await;
    for (refused, at) in [(one, "/kind"), (many, "/containers/0/kind")] {
        refused.check_bad_request();
        assert_eq!(refused.refused_at(), at, "{}", refused.text());
    }
    let together = format!("{ws}/containers-and-blocks");
    for method in [Method::PATCH, Method::DELETE] {
        let body = |id: &str| {
            if method == Method::PATCH {
                json!({"containerIds": [id], "containerPatches": [{"title": "taken"}],
                       "blockIds": [], "blockPatches": []})
            } else {
                json!({"containers": [id], "blocks": []})
            }
        };
        let send = async |id: &str| {
            Client::send(client.request(method.clone(), &together).json(&body(id))).await
        };
        let (of_diary, nothing) = (send(&diary.container).await, send(UNKNOWN_ID).await);
        assert_eq!(of_diary.problem(), nothing.problem(), "{method}");
        assert!(
            !of_diary.status.is_success(),
            "{method}: {}",
            of_diary.text()
        );
    }
    let entry_as_block = client
        .post(
            &format!("{ws}/boards/{}/blocks", board.container),
            &json!([{"containerId": board.container, "type": "entry", "title": "2026-11-21",
                     "fields": {"date": "2026-11-21"}, "createdAt": 1, "updatedAt": 1}]),
        )
        .await;
    entry_as_block.check_bad_request();
    // Nor is a diary entry a block to subscribe to.
    let subscribe = |block: &str| {
        json!({
            "blockId": block, "blockType": "entry", "userId": world.me,
        })
    };
    let (to_entry, to_nothing) = (
        client
            .post(&format!("{ws}/subscriptions"), &subscribe(&diary.block))
            .await,
        client
            .post(&format!("{ws}/subscriptions"), &subscribe(UNKNOWN_ID))
            .await,
    );
    to_entry.check_not_found();
    assert_eq!(to_entry.problem(), to_nothing.problem());
    assert_eq!(ids(&client.get(&format!("{ws}/diaries")).await).len(), 1);
    world.ts.teardown().await;
}

/// The archive and the trash of a user's own space hold all three kinds, and the trash
/// says of each which it is.
#[tokio::test]
async fn the_archive_and_the_trash_of_an_own_space_hold_every_kind() {
    let world = World::start().await;
    let client = &world.ts.client;
    let own = world.own.clone();
    let ws = format!("/w/{own}");
    let [board, wiki, diary, course] = make_in_own_space(&world, &own).await;
    let exported = client.get(&format!("{ws}/archive/export")).await;
    exported.check_done();
    let archive = zip::ZipArchive::new(std::io::Cursor::new(exported.body))
        .expect("the export is an archive");
    let exported: BTreeSet<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .filter_map(|name| name.strip_suffix("/container.jsonl").map(str::to_owned))
        .collect();
    let all: BTreeSet<String> = [&board, &wiki, &diary, &course]
        .into_iter()
        .map(|things| things.container.clone())
        .collect();
    assert_eq!(
        exported, all,
        "the export is of boards, wikis, diaries and courses"
    );

    for things in [&board, &wiki, &diary, &course] {
        let at = format!("{ws}/containers/{}", things.container);
        client.delete(&at).await.check_done();
    }
    let trash = client.get(&format!("{ws}/trash/containers")).await;
    trash.check_done();
    let mut listed: Vec<(String, String)> = trash
        .value()
        .as_array()
        .expect("a list")
        .iter()
        .map(|entry| (text(entry, "kind"), text(entry, "id")))
        .collect();
    listed.sort();
    assert_eq!(
        listed,
        [
            ("board".to_owned(), board.container.clone()),
            ("course".to_owned(), course.container.clone()),
            ("diary".to_owned(), diary.container.clone()),
            ("wiki".to_owned(), wiki.container.clone()),
        ]
    );
    world.ts.teardown().await;
}

/// The identifiers of what a list answers with, in its order.
fn ids(listed: &ApiResponse) -> Vec<String> {
    let listed = listed.value();
    let listed = listed.as_array().expect("a list");
    listed.iter().map(|entry| text(entry, "id")).collect()
}

/// The lists of boards and the search are of one kind each; the trash of boards and the
/// archive of the workspace hold both, and the trash says of each which it is.
#[tokio::test]
async fn every_list_is_of_its_kinds() {
    let world = World::start().await;
    let client = &world.ts.client;
    let (board, wiki, course) = (
        world.make(ContainerKind::Board).await,
        world.make(ContainerKind::Wiki).await,
        world.make(ContainerKind::Course).await,
    );
    let (the_board, the_wiki, the_course) = (
        std::slice::from_ref(&board.container),
        std::slice::from_ref(&wiki.container),
        std::slice::from_ref(&course.container),
    );

    let boards = client.get(&format!("{ROOT_WS}/boards")).await;
    assert_eq!(ids(&boards), the_board);
    let wikis = client.get(&format!("{ROOT_WS}/wikis")).await;
    assert_eq!(ids(&wikis), the_wiki);
    let courses = client.get(&format!("{ROOT_WS}/courses")).await;
    assert_eq!(ids(&courses), the_course);
    assert_eq!(courses.value()[0]["kind"], "course");
    assert_eq!(wikis.value()[0]["kind"], "wiki");
    assert_eq!(boards.value()[0]["kind"], "board");
    let titled = json!({"title": "findable"});
    for things in [&board, &wiki, &course] {
        let at = format!("{ROOT_WS}/containers/{}", things.container);
        client.patch(&at, &titled).await.check_done();
    }
    let found = client
        .get(&format!("{ROOT_WS}/boards/search?q=findable"))
        .await;
    assert_eq!(ids(&found), the_board, "the search is for boards");
    let exported = client.get(&format!("{ROOT_WS}/archive/export")).await;
    exported.check_done();
    let archive = zip::ZipArchive::new(std::io::Cursor::new(exported.body))
        .expect("the export is an archive");
    let boards_in: BTreeSet<String> = archive
        .file_names()
        .filter_map(Result::ok)
        .filter_map(|name| name.strip_suffix("/container.jsonl").map(str::to_owned))
        .collect();
    let all = BTreeSet::from([
        board.container.clone(),
        wiki.container.clone(),
        course.container.clone(),
    ]);
    assert_eq!(boards_in, all, "the export is of boards, wikis and courses");

    for things in [&board, &wiki, &course] {
        let at = format!("{ROOT_WS}/containers/{}", things.container);
        client.delete(&at).await.check_done();
    }
    let trash = client.get(&format!("{ROOT_WS}/trash/containers")).await;
    trash.check_done();
    let mut listed: Vec<(String, String)> = trash
        .value()
        .as_array()
        .expect("a list")
        .iter()
        .map(|entry| (text(entry, "kind"), text(entry, "id")))
        .collect();
    listed.sort();
    assert_eq!(
        listed,
        [
            ("board".to_owned(), board.container.clone()),
            ("course".to_owned(), course.container.clone()),
            ("wiki".to_owned(), wiki.container.clone()),
        ]
    );
    // A card in the trash says no kind.
    let card = format!(
        "{ROOT_WS}/boards/{}/blocks/{}",
        board.container, board.block
    );
    let again = client
        .post(
            &format!("{ROOT_WS}/containers/{}/restore", board.container),
            &json!({}),
        )
        .await;
    again.check_done();
    client.delete(&card).await.check_done();
    let cards = client
        .get(&format!("{ROOT_WS}/boards/{}/trash/cards", board.container))
        .await;
    assert_eq!(ids(&cards), std::slice::from_ref(&board.block));
    assert!(cards.value()[0].get("kind").is_none(), "{}", cards.text());
    world.ts.teardown().await;
}

/// What names a board in a body or a query takes no wiki: the answer is the one for an
/// identifier of nothing, and nothing is made or changed. The board a guest is
/// invited to may be a wiki.
#[tokio::test]
async fn what_names_a_wiki_in_a_body_or_a_query_is_refused_as_what_names_nothing() {
    let world = World::start().await;
    let client = &world.ts.client;
    let (board, wiki, course) = (
        world.make(ContainerKind::Board).await,
        world.make(ContainerKind::Wiki).await,
        world.make(ContainerKind::Course).await,
    );

    assert_board_creation_refuses_other_kinds(client, &board.container).await;
    // What changes or deletes boards together, and the source of copied blocks.
    let same = async |method: Method, path: &str, body: &dyn Fn(&str) -> Value| {
        let send = async |id: &str| {
            Client::send(client.request(method.clone(), path).json(&body(id))).await
        };
        let nothing = send(UNKNOWN_ID).await;
        for other in [&wiki.container, &course.container] {
            let of_other = send(other).await;
            assert_eq!(of_other.problem(), nothing.problem(), "{method} {path}");
            assert!(
                !of_other.status.is_success(),
                "{method} {path}: {}",
                of_other.text()
            );
        }
    };
    let together = format!("{ROOT_WS}/containers-and-blocks");
    same(Method::PATCH, &together, &|id| {
        json!({
            "containerIds": [id],
            "containerPatches": [{"title": "changed"}],
            "blockIds": [board.block],
            "blockPatches": [{"title": "changed"}],
        })
    })
    .await;
    same(
        Method::DELETE,
        &together,
        &|id| json!({"containers": [id], "blocks": [board.block]}),
    )
    .await;
    let copied =
        json!([{"containerId": board.container, "type": "view", "fields": {"viewType": "board"}}]);
    let from = async |source: &str| {
        let path = format!(
            "{ROOT_WS}/boards/{}/blocks?sourceBoardId={source}",
            board.container
        );
        client.post(&path, &copied).await
    };
    let other = client
        .post(
            &format!("{ROOT_WS}/boards"),
            &json!({"visibility": "private"}),
        )
        .await;
    other.check_done();
    from(&text(&other.value(), "id")).await.check_done();
    let nothing = from(UNKNOWN_ID).await;
    for target in [&wiki.container, &course.container] {
        let of_target = from(target).await;
        assert_eq!(of_target.problem(), nothing.problem(), "sourceBoardId");
        assert!(!of_target.status.is_success(), "{}", of_target.text());
    }
    client
        .put(
            &format!("{ROOT_WS}/guest-invitations"),
            &json!({"enabled": true}),
        )
        .await
        .check_done();
    // A guest is invited to a wiki or a course as to a board.
    for target in [&wiki.container, &course.container] {
        client
            .post(
                &format!("{ROOT_WS}/invitations"),
                &json!({"role": "guest", "containerId": target}),
            )
            .await
            .check_done();
    }

    // A course unit accepts subscriptions; a chapter does not.
    assert_course_subscriptions(&world, &course.container, &course.block).await;

    // The wiki is as it was, and so is the board's card.
    let read = client
        .get(&format!("{ROOT_WS}/containers/{}", wiki.container))
        .await;
    read.check_done();
    assert_ne!(read.value()["title"], "changed");
    let card = client
        .get(&format!("{ROOT_WS}/cards/{}", board.block))
        .await;
    card.check_done();
    assert_eq!(card.value()["title"], "a card");
    world.ts.teardown().await;
}

async fn assert_course_subscriptions(world: &World, course_id: &str, unit_id: &str) {
    let chapter = world
        .ts
        .client
        .post(
            &format!("{ROOT_WS}/courses/{course_id}/units"),
            &json!({"kind": "chapter", "title": "a chapter"}),
        )
        .await;
    chapter.check_done();
    let chapter_id = text(&chapter.value(), "id");
    let subscribe = |block: &str, block_type: &str| {
        json!({
            "blockId": block, "blockType": block_type, "userId": world.me,
        })
    };
    world
        .ts
        .client
        .post(
            &format!("{ROOT_WS}/subscriptions"),
            &subscribe(unit_id, "unit"),
        )
        .await
        .check_done();
    let (to_chapter, to_nothing) = (
        world
            .ts
            .client
            .post(
                &format!("{ROOT_WS}/subscriptions"),
                &subscribe(&chapter_id, "unit"),
            )
            .await,
        world
            .ts
            .client
            .post(
                &format!("{ROOT_WS}/subscriptions"),
                &subscribe(UNKNOWN_ID, "unit"),
            )
            .await,
    );
    to_chapter.check_not_found();
    assert_eq!(
        to_chapter.problem(),
        to_nothing.problem(),
        "a course chapter is refused for subscriptions as a missing block"
    );
}

async fn assert_board_creation_refuses_other_kinds(client: &Client, board_id: &str) {
    let card = json!({"id": "card", "containerId": "made", "type": "card", "createdAt": 1, "updatedAt": 1});
    for non_board in ["wiki", "course"] {
        let as_other =
            json!({"id": "made", "visibility": "private", "kind": non_board, "title": "no board"});
        let one = client
            .post(
                &format!("{ROOT_WS}/boards"),
                &json!({"visibility": "private", "kind": non_board}),
            )
            .await;
        let many = client
            .post(
                &format!("{ROOT_WS}/containers-and-blocks"),
                &json!({"containers": [as_other], "blocks": [card]}),
            )
            .await;
        for (refused, at) in [(one, "/kind"), (many, "/containers/0/kind")] {
            refused.check_bad_request();
            assert_eq!(refused.refused_at(), at, "{non_board}: {}", refused.text());
        }
    }
    assert_eq!(
        ids(&client.get(&format!("{ROOT_WS}/wikis")).await).len(),
        1,
        "only the seeded wiki exists"
    );
    assert_eq!(
        ids(&client.get(&format!("{ROOT_WS}/courses")).await).len(),
        1,
        "only the seeded course exists"
    );
    for block_type in ["unit", "chapter"] {
        let refused = client
            .post(
                &format!("{ROOT_WS}/boards/{board_id}/blocks"),
                &json!([{"containerId": board_id, "type": block_type, "title": "no",
                         "createdAt": 1, "updatedAt": 1}]),
            )
            .await;
        refused.check_bad_request();
    }
}

/// The audit of the workspace names the wiki an administrator of the workspace put
/// themselves on, as it names a board, to who sees it.
#[tokio::test]
async fn the_audit_names_a_wiki_as_it_names_a_board() {
    let world = World::start().await;
    let (administrator, maker) = (&world.ts.client, &world.ts.client2);
    let mut made = Vec::new();
    for (path, title) in [
        ("boards", "Roadmap"),
        ("wikis", "Plans"),
        ("courses", "Curriculum"),
    ] {
        let container = maker
            .post(
                &format!("{ROOT_WS}/{path}"),
                &json!({"visibility": "private", "title": title}),
            )
            .await;
        container.check_done();
        let id = text(&container.value(), "id");
        administrator
            .post(
                &format!("{ROOT_WS}/containers/{id}/members"),
                &json!({"containerId": id, "userId": world.me, "role": "admin"}),
            )
            .await
            .check_done();
        made.push((id, title));
    }
    let audit = administrator.get(&format!("{ROOT_WS}/audit")).await;
    audit.check_done();
    let audit = audit.value();
    let records = audit.as_array().expect("a list");
    for (id, title) in made {
        let named: Vec<&Value> = records
            .iter()
            .filter(|record| record["targetId"] == json!(id))
            .map(|record| &record["targetName"])
            .collect();
        assert_eq!(named, [&json!(title)], "the records about {title}");
    }
    world.ts.teardown().await;
}

/// Nothing changes the kind of a board or a wiki: a patch that names a kind, its own
/// too, is refused at `/kind` and changes nothing; a member a patch does not know is
/// left out, as it was.
#[tokio::test]
async fn a_patch_that_names_a_kind_is_refused() {
    let world = World::start().await;
    let client = &world.ts.client;
    let (board, wiki, course) = (
        world.make(ContainerKind::Board).await,
        world.make(ContainerKind::Wiki).await,
        world.make(ContainerKind::Course).await,
    );
    for (things, own) in [(&board, "board"), (&wiki, "wiki"), (&course, "course")] {
        let at = format!("{ROOT_WS}/containers/{}", things.container);
        for kind in ["board", "wiki", "course", "page", ""] {
            let refused = client
                .patch(&at, &json!({"kind": kind, "title": "changed"}))
                .await;
            refused.check_bad_request();
            assert_eq!(refused.refused_at(), "/kind", "{own} as {kind:?}");
        }
        let read = client.get(&at).await.value();
        assert_eq!(
            (read["kind"].as_str(), read["title"].as_str()),
            (Some(own), Some(""))
        );
        client
            .patch(&at, &json!({"unknown": true, "title": "changed"}))
            .await
            .check_done();
    }
    // Who does not see the board is told it is not there, whatever the patch names.
    let unseen = client
        .post(
            &format!("{ROOT_WS}/boards"),
            &json!({"visibility": "private"}),
        )
        .await;
    unseen.check_done();
    let unseen = format!("{ROOT_WS}/containers/{}", text(&unseen.value(), "id"));
    let (with_kind, without) = (
        world
            .ts
            .client2
            .patch(&unseen, &json!({"kind": "wiki"}))
            .await,
        world
            .ts
            .client2
            .patch(&unseen, &json!({"title": "x"}))
            .await,
    );
    with_kind.check_not_found();
    assert_eq!(with_kind.problem(), without.problem());
    // Nor is a kind changed with blocks.
    let together = client
        .patch(
            &format!("{ROOT_WS}/containers-and-blocks"),
            &json!({
                "containerIds": [board.container],
                "containerPatches": [{"kind": "wiki"}],
                "blockIds": [],
                "blockPatches": [],
            }),
        )
        .await;
    together.check_bad_request();
    assert_eq!(together.refused_at(), "/containerPatches/0/kind");
    world.ts.teardown().await;
}
