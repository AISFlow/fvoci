//! The contract of the API: the OpenAPI document of what the server takes and answers.
//!
//! The document is made from the routes themselves. Every handler carries a
//! `#[utoipa::path]` that names its path, its parameters, its bodies and its answers,
//! and the same attribute puts the handler on the router, so that a route cannot be
//! served without being in the document. [`document`] collects them and finishes what
//! the attributes cannot say: the head of the document, the answer every operation
//! gives when it refuses, and which members of an answer are always there.
//!
//! The document is committed as `contracts/openapi.json`; `cargo xtask openapi` fails
//! when the file is not what [`document`] makes.
//!
//! The doc comment of a type or a member the contract shows is its description there,
//! read by whoever calls the API. What only the code's readers need of such a type goes
//! in a plain `//` comment, which the contract leaves out.

pub(crate) mod bodies;
mod finish;
pub(crate) mod params;
mod tags;

use std::collections::BTreeSet;

use fvoci_model::{InvalidInput, Problem};
use serde_json::{Map, Value, json};
use utoipa::PartialSchema;
use utoipa_axum::router::OpenApiRouter;

use crate::app::App;
use crate::problems::Basis;

#[cfg(test)]
use finish::path_parameters;
use finish::{finish_operations, plain_descriptions, sorted};
use tags::TAGS;
pub(crate) use tags::tag;

/// The name of the security scheme of the session cookie.
const SESSION_COOKIE: &str = "sessionCookie";
/// The name of the security scheme of the token in the `Authorization` header.
const BEARER_TOKEN: &str = "bearerToken";

/// What the head of the document says about the API as a whole.
const DESCRIPTION: &str = "\
The API of an fvoci server.

## Signing in

`login` answers with a `Set-Cookie` header that carries the session: the cookie \
`__Host-fvoci_session` where the server is reached over HTTPS, `fvoci_session` over plain \
HTTP. It is `HttpOnly`, so a page's scripts never see it, and a browser sends it by itself. \
A client that is no browser may send the cookie's value as `Authorization: Bearer <value>` \
instead. The operations under `/shared/{containerId}` need no session: they read a shared board with its \
`readToken`, and look at no session that comes along.

## Personal API tokens

A user makes a personal API token (`createApiToken`) for an agent, which sends it as \
`Authorization: Bearer fvoci_pat_...`, never in a cookie. A token reaches the workspaces it \
names, of those its owner works in (no guest's), and the owner's own space if it was given \
that, but never a diary. It reads (`scope` `read`), or reads and writes the content of cards, \
pages, units, their comments and files (`write`): an operation says in its description \
whether a token is taken, and which. Every other operation answers a token with 403 and the \
reason `token_forbidden_endpoint`, the websocket too: what is of the account, of who is \
on what, of what is shared, and of an administrator is done by a person. A token that reads \
only is answered 403 with the reason `token_scope_insufficient` where an operation writes. A \
token sends at most 120 requests a minute, and the tokens that reach one organization 600 \
together; past that it is answered 429 with `Retry-After` and the reason `token_rate_limited`. \
It lasts as long as it was made for (90 days unless asked, at most 365, at most 90 where it \
reaches an organization that takes only sessions made through its provider), and ends with \
what ends its owner's sessions: a password changed, set or reset, the identity it was made \
through unlinked, the provider it was made through changed or removed, the account put out \
of use. An organization that takes only sessions made through its provider holds to its 90 \
days a token made before it did: older, the token is answered 401 there with the reason \
`token_expired_for_organization`.

An agent may instead speak the Model Context Protocol to `POST /api/v1/mcp` (`postMcp`), \
which takes a token and nothing else: its tools read and write as the operations above do, \
for the same token, with the same limits and refusals, and a token that reads only is told \
of no tool that writes.

## Requests that change something

Every request whose method is not `GET`, `HEAD` or `OPTIONS` needs the header \
`X-Requested-With: XMLHttpRequest`, unless it has a bearer token, and is refused with 403 \
and the reason `csrf` without it. If it has an `Origin` header, the origin must be the \
server's own (or one the server was configured to accept); otherwise it is refused with 403 \
and the reason `foreign_origin`.

A page should make these requests with `fetch` in its default mode, `cors`. The server sends \
its pages with `Referrer-Policy: no-referrer`, and under that policy a browser writes \
`Origin: null` on a changing request of any other mode (`same-origin`, `no-cors`, or a form \
the browser posts itself), which is refused as a foreign origin.

## Answers

An operation answers 200 with a body, 201 with what it made, or 204 with no body. Every \
other status comes with a `Problem`, the problem details of RFC 9457, as \
`application/problem+json`: `type` names the kind of the problem and is the same for every \
problem of the kind, `title` is its name, `status` repeats the status and `detail` says what \
went wrong this time. `type` is `/problems/<name>` under the path the server is served at, \
the address of a page of text about the kind; `/problems` lists them, and so does \
`x-problem-types` of the schema `Problem`, as they are on a server at the root of its host \
(a server under a path has that path before each). `reason` tells apart causes a client must tell apart; a \
problem with a `reason` has the `type` of that name, written with hyphens, and any other the \
`type` of its status. `errors` lists what of the request was refused, each by a JSON Pointer \
into the body (`pointer`), the name of a parameter of the query or the path (`parameter`), or \
the name of a header (`header`). `limit` is the limit that was exceeded: bytes for a body \
that was too large (413), a count for too many of something. `instance` is the request, `urn:uuid:` and its `requestId`. A session that \
is missing or over is 401. What the user may not see is 404, as if it were not there; what \
they may see but not do is 403. A path that is no operation is 404 too, and a method an \
operation's path does not have is 405.

A request without a session whose JSON body has more than 64 KiB is 413 before it is 401. \
A body larger than its route reads is 413 with the reason `request_too_large` and the `limit` \
in bytes.

Any request may be answered 503 with the reason `database_busy`: the database did not take \
one of its statements in time (another write held the file, or no connection to it was to be \
had). That statement did nothing and may be taken a moment later. A request of one step may \
be sent again as it is; of one of several (`completeUpload`, `importArchive`) what was done \
before stays done, and sent again it is answered as its operation says.

The identifiers of users, workspaces, boards, blocks and categories are UUIDs, written \
lower-case with hyphens; a client takes every identifier as an opaque string. A `fileId` is \
no UUID: it is an unguessable name that ends in the file's extension. Times are \
milliseconds since the Unix epoch.

## Lists

Every list comes in an order, and the description of each operation says which. Where it \
says nothing else, the order is that of creation, oldest first, and by identifier among \
what was created in the same millisecond.

## The user's own space

Every account has a space of its own from the moment it is made, however it was made: \
an organization of the `kind` `personal` with the account as its only member and one \
workspace, which `getMyWorkspaces` lists with that `kind`. Its workspace is a workspace \
like another to the operations under `/w/{workspaceId}`: boards, wikis, files, search, \
the trash, export and import, and sharing by a public link (`setPublicLink`) work in it as \
anywhere. It has no name (a client calls it by the reader's language) and takes none.

What it has not is other people, and anybody to administer it. Its identifier is 404 \
for every operation under `/org/{orgId}` (the organization and its settings, its \
members, its workspaces, joining one as an administrator, its audit log, its provider \
of sign-ins), for its owner as for anybody else, and no answer of those operations \
names it. Under its workspace these are 404, as for a workspace that is not there: \
`createInvitation`, `getInvitations`, `revokeInvitation`, `emailInvitation`, \
`setGuestInvitations`, `setWorkspaceRole`, `removeWorkspaceMember`, the operations on teams (`/w/{workspaceId}/teams` and what is \
under it) and on the teams of a board or a wiki \
(`/w/{workspaceId}/containers/{containerId}/teams`). It has no login page \
(`getLoginOptions`) and no sign-in through a provider starts for it. Nothing in it is \
moved to or from another organization but by export and import, and no reference \
reaches into it from outside or out of it. What changes in it is told to nobody else: \
the server's operator is posted no block of it (`block_change_webhooks`), and it is in \
no audit log anybody but its owner reads.

An account that was made by a sign-in through an organization's provider has no \
password (`hasPassword` of `getMe`), and until it has one its space is reached by any \
session of the account. It is given one by `setPassword`, so that its person reaches \
the space when they are no longer in that organization. Once the account has a \
password, the space takes only sessions that have shown it: one made with it (`login`), \
and one made through a provider that gave it since (`confirmPassword`). A provider is \
an organization's, and whoever holds it could make a session of the account. To a \
session made through a provider that has not given the password, the routes under the \
space's `/w/{workspaceId}` then answer 401 with the reason \
`password_sign_in_required`, `getMyWorkspaces` lists the space with `passwordRequired` \
true, `getMyMemberships` leaves its boards out, `getAccountDeletion` names it without \
what is in it, and the websocket follows nothing of it for such a session \
(`contracts/websocket.md`). The session stays and opens what it opened of the \
organizations.

There are two ways in from there. `confirmPassword` keeps the session: it gives the \
password once, and from then until it ends the session opens the space beside what it \
opened before, so that somebody whose organization takes only sessions made through \
its provider (`sso_required`) goes between that organization and the space without \
signing in again. Signing in with the password (`login`) opens the space too, in a new \
session, which such an organization then asks to sign in through its provider again. \
What a session confirmed opens the account's own space and nothing else: not what is \
another account's, and nothing of an organization that takes only sessions made \
through a provider other than the session's own. It goes with the session: a change \
of the password (`changePassword`, a reset, the operator's) ends the account's other \
sessions, as it always did, and what they confirmed with them. What is shared from \
the space by a public link is read without a session, and is not closed so.

It goes with its account: `getAccountDeletion` names it among `organizations` always, \
and while the account waits to be deleted it is closed with the account: what is \
shared from it by a public link answers 404 as a link that is not there, and answers \
again if the deletion is cancelled.

## Bodies edited together

While people edit a card's body together the server holds it as a Y document. What it \
refuses of an editor has the reason `update_refused`, `update_out_of_step`, \
`update_too_large`, `document_too_large` or `table_too_large` (`table_too_large` also \
comes from writing a body whole over HTTP when a table has more than 200 rows or more \
than 20 columns); `too_many_documents` and \
`document_unavailable` say that it cannot take the document for now. A write of a card's \
whole body here (a patch of the card or of blocks, a card stored again) first writes to the \
card what its editors typed: a `readAt` from before that is refused with 409, and if it \
cannot be written the write is refused with `document_unavailable`. The editors \
themselves speak to the server over the websocket, whose messages name the same reasons \
(`contracts/websocket.md`); `document_read_only`, `connection_too_slow`, \
`too_many_documents_joined`, `document_full`, `too_many_connections_joined` and \
`too_many_frames` come only there. The \
page of each kind says when it comes. `search_not_configured` comes from none of these \
operations: only from the reindex of the administration socket, on a server configured \
without a search server (a search there searches its database).

## Request bodies

A request schema lists the members a request is for. A member that is not required may be \
left out. `null` is a value only of a member whose schema allows it, and is then none; `null` \
for any other member is refused with 400, as a value of another type is. A member the schema \
marks as required is one the request cannot do without; \
where the server takes a missing one as empty instead of refusing the request, the result \
is that of the empty value (a login without a password is a failed login). A body with \
members the schema does not list is mostly not refused for them. The exceptions: \
`patchUserSettings`, `patchMe` and `createSubscription` refuse every member they do not know, `createBoard` and \
`createCategory` refuse an `id`, and `setPublicLink` refuses a `token`.";

/// What an operation's `default` response says.
const REFUSAL: &str = "The request was refused, or failed. The status says the kind: 400 for a \
request that is malformed or asks for what cannot be, 401 without a session, 403 for what the \
user may not do, 404 for what is not there for them, 405 for a method the path does not have, \
409 for what is in the way, 412 for a condition of the request that does not hold, 413 for a \
body that is too large, 416 for a range that is not there, 429 for too many requests, 500 for \
a failure of the server, 503 for a server that cannot serve for now.";

/// Which members of a schema are always there.
#[derive(Debug, Clone, Copy)]
enum Members {
    /// The schema says it itself: it was written for the contract, or has no members.
    AsWritten,
    /// Every member.
    All,
    /// Every member but these, which are left out when they are empty.
    AllBut(&'static [&'static str]),
    /// These members and no others.
    Only(&'static [&'static str]),
}

/// Every schema of the document, with the members that are always there.
///
/// The types the server answers with are decoded leniently where they are read, so
/// their derived schemas call every member optional. In an answer every member is
/// there, and that is what the contract says. A schema that is not listed here fails
/// [`document`], so that a new type is looked at once.
const SCHEMAS: &[(&str, Members)] = &[
    // What the server answers with.
    (
        "AcceptedInvitation",
        Members::AllBut(&["containerId", "containerKind"]),
    ),
    (
        "AccountDeletion",
        Members::AllBut(&["requestedAt", "deleteAfter"]),
    ),
    ("AccountDeletionAsked", Members::All),
    (
        "LeavingOrganization",
        Members::AllBut(&["boards", "wikis", "diaries", "courses", "files"]),
    ),
    ("OrganizationName", Members::All),
    (
        "EmailVerified",
        Members::AllBut(&["containerId", "containerKind"]),
    ),
    ("RevertedEmailChange", Members::AllBut(&["email"])),
    (
        "Block",
        Members::AllBut(&["deletedAt", "editedAt", "number"]),
    ),
    ("Body", Members::AsWritten),
    (
        "Container",
        Members::AllBut(&["deletedAt", "dueDatePropertyId", "doneWhen"]),
    ),
    ("DoneWhen", Members::Only(&["propertyId"])),
    ("ContainerPermissions", Members::All),
    ("BoardViewWrite", Members::All),
    ("CardMovesResult", Members::All),
    ("RestoredCards", Members::All),
    ("CopiedContainerResult", Members::All),
    ("AccountToday", Members::All),
    ("WorkspaceToday", Members::All),
    ("TodayWorkspaceMeta", Members::All),
    (
        "TodayCardItem",
        Members::AllBut(&["cardNumber", "dueAtMillis"]),
    ),
    ("TodayUnitEntry", Members::All),
    ("TodayReviewEntry", Members::All),
    ("TodayMeetingItem", Members::AllBut(&["endsAt"])),
    (
        "ContainerMember",
        Members::AllBut(&["synthetic", "inheritedRole"]),
    ),
    ("ContainerTeam", Members::All),
    ("ContainerTeamRole", Members::AsWritten),
    ("ContainersAndBlocks", Members::All),
    ("BodyVersion", Members::AllBut(&["agentTokenName"])),
    ("BodyVersionCause", Members::AsWritten),
    ("BodyVersionContent", Members::AllBut(&["agentTokenName"])),
    ("KeptBodyVersion", Members::All),
    ("KeptOutcome", Members::AsWritten),
    (
        "Card",
        Members::AllBut(&[
            "deletedAt",
            "preview",
            "bodyMarkdown",
            "hasComplexBlocks",
            "complexBlocks",
            "recurrence",
            "recurredTo",
        ]),
    ),
    ("Recurrence", Members::AllBut(&["from"])),
    ("RecurrenceAnchor", Members::AsWritten),
    ("RecurrenceUnit", Members::AsWritten),
    ("CardEvent", Members::All),
    ("CardEventKind", Members::AsWritten),
    ("CardPreview", Members::All),
    ("CardWithReactions", Members::AllBut(&["reactions"])),
    ("CardsOfPage", Members::AsWritten),
    ("CompactCard", Members::All),
    ("ComplexBlock", Members::All),
    ("ComplexBlockReason", Members::AsWritten),
    ("CardPropertyDefinition", Members::All),
    ("PropertyOption", Members::All),
    ("Category", Members::AllBut(&["deletedAt"])),
    ("CategoryContainerMetadata", Members::All),
    ("CategoryContainers", Members::All),
    ("ClientConfig", Members::All),
    ("CommentVersion", Members::All),
    ("CommentWithReactions", Members::AllBut(&["reactions"])),
    ("Reaction", Members::All),
    ("DeletedItem", Members::AllBut(&["kind"])),
    ("Favorite", Members::AllBut(&["number"])),
    ("FavoriteKind", Members::AsWritten),
    ("FileRecord", Members::All),
    ("ListedFile", Members::All),
    ("FileLease", Members::All),
    ("FileLeaseRequest", Members::AsWritten),
    ("FileVersion", Members::All),
    ("FileVersionBase", Members::AsWritten),
    ("HeldFileLease", Members::All),
    ("ImportCounts", Members::All),
    ("ImportFormat", Members::AsWritten),
    ("ImportSummary", Members::AllBut(&["warningsOmitted"])),
    ("ImportWarning", Members::All),
    ("ImportWarningCode", Members::AsWritten),
    ("InvalidInput", Members::Only(&["detail", "reason"])),
    ("LinkedIdentity", Members::All),
    (
        "Invitation",
        Members::AllBut(&[
            "containerId",
            "containerRole",
            "maxUses",
            "parentId",
            "recipient",
            "token",
        ]),
    ),
    (
        "InvitationPreview",
        Members::AllBut(&["containerTitle", "containerKind"]),
    ),
    ("LoginOptions", Members::All),
    ("OidcStart", Members::All),
    ("LoginOrganization", Members::All),
    ("OidcProvider", Members::All),
    ("OidcFormerProvider", Members::All),
    ("OidcFormerProviderEnd", Members::All),
    ("OidcPendingProvider", Members::All),
    ("Notification", Members::All),
    ("NotificationKind", Members::AsWritten),
    ("NotificationSubject", Members::AllBut(&["agentTokenName"])),
    ("TimeEntry", Members::All),
    ("TimeEntryKind", Members::AsWritten),
    ("TimeTargetKind", Members::AsWritten),
    ("ActiveTimeEntry", Members::All),
    ("StartedTimeEntry", Members::AllBut(&["stopped"])),
    ("TimeEntryKindTotals", Members::All),
    ("TimeEntrySummary", Members::All),
    ("TimeEntryList", Members::All),
    (
        "Problem",
        Members::Only(&["type", "title", "status", "detail"]),
    ),
    ("Readiness", Members::All),
    ("ReadinessCheck", Members::AsWritten),
    ("SearchHit", Members::AllBut(&["number"])),
    ("SearchKind", Members::AsWritten),
    ("ServerMetadata", Members::AsWritten),
    ("PublicLink", Members::AllBut(&["token", "updatedAt"])),
    ("SnippetPart", Members::All),
    ("Subscription", Members::AllBut(&["deletedAt"])),
    ("WelcomeBoard", Members::All),
    ("Workspace", Members::All),
    ("WorkspaceOverview", Members::All),
    ("OverviewWorkspace", Members::All),
    ("OverviewPerson", Members::All),
    (
        "OverviewContainer",
        Members::AllBut(&["myRole", "cardProperties"]),
    ),
    ("Team", Members::All),
    ("TeamMember", Members::All),
    ("TeamName", Members::AsWritten),
    ("AuditAction", Members::AsWritten),
    ("AuditPerson", Members::All),
    ("AuditRecord", Members::All),
    ("AuditRecordActor", Members::AsWritten),
    ("Organization", Members::All),
    (
        "ApiToken",
        Members::AllBut(&["lastUsedAt", "lastUsedAddress"]),
    ),
    ("MemberApiToken", Members::AllBut(&["lastUsedAt"])),
    ("ApiTokenScope", Members::AsWritten),
    ("ApiTokenWorkspace", Members::All),
    ("IssuedApiToken", Members::All),
    ("OrgMember", Members::All),
    ("OrgWorkspace", Members::AllBut(&["deletedAt"])),
    ("UploadOpened", Members::All),
    ("UploadState", Members::All),
    ("ReceivedPart", Members::All),
    ("UploadedFile", Members::AsWritten),
    ("User", Members::AllBut(&["deletedAt"])),
    ("Me", Members::AllBut(&["pendingEmail"])),
    ("PersonName", Members::All),
    ("UserSettings", Members::AsWritten),
    ("UnreadNotifications", Members::All),
    ("UserOrganization", Members::All),
    ("UserWorkspace", Members::All),
    ("WorkspaceMember", Members::All),
    ("WorkspaceRole", Members::AsWritten),
    ("OrgRole", Members::AsWritten),
    ("OrgKind", Members::AsWritten),
    ("Page", Members::All),
    ("PageWithReactions", Members::AllBut(&["reactions"])),
    ("PageNode", Members::All),
    (
        "LinkedBlock",
        Members::AllBut(&["number", "containerTitle", "viewType"]),
    ),
    ("WorkspaceRoleChange", Members::All),
    // What a request carries.
    ("AcceptInvitation", Members::Only(&["token"])),
    ("AccountChange", Members::AsWritten),
    ("AdminChange", Members::AsWritten),
    ("ContainerPatch", Members::Only(&[])),
    ("CardPatch", Members::Only(&[])),
    ("CategoryUpdate", Members::AsWritten),
    (
        "ChangePasswordRequest",
        Members::Only(&["oldPassword", "newPassword"]),
    ),
    ("FileForm", Members::AsWritten),
    ("FinishPasswordReset", Members::All),
    ("CompleteEmailVerification", Members::Only(&["token"])),
    ("ConfirmEmailChange", Members::All),
    ("RevertEmailChange", Members::All),
    ("RequestEmailChange", Members::Only(&["email", "password"])),
    ("ResendEmailChange", Members::Only(&[])),
    ("RequestEmailVerification", Members::Only(&[])),
    ("RequestPasswordReset", Members::Only(&["email"])),
    ("GuestInvitationsChange", Members::All),
    ("ImportRequest", Members::Only(&["uploadId"])),
    ("InvitationMail", Members::Only(&["token", "recipients"])),
    ("InvitationToken", Members::All),
    ("LoginRequest", Members::Only(&["password"])),
    ("ContainerMemberPatch", Members::All),
    (
        "NewAccount",
        Members::Only(&["username", "email", "password"]),
    ),
    ("NewBoard", Members::AsWritten),
    ("NewWiki", Members::AsWritten),
    ("NewPage", Members::Only(&[])),
    ("PagePatch", Members::Only(&[])),
    ("NewPageComment", Members::Only(&["body"])),
    ("PageCommentPatch", Members::Only(&["body", "readAt"])),
    ("PageMove", Members::Only(&[])),
    ("PageDuplicate", Members::Only(&[])),
    ("DeletedPage", Members::All),
    ("DiaryEntry", Members::All),
    ("DiaryDay", Members::AllBut(&["mood"])),
    ("DiaryRun", Members::All),
    ("DeletedDiaryEntry", Members::AllBut(&["mood"])),
    ("Mood", Members::AsWritten),
    ("NewDiary", Members::Only(&[])),
    ("NewDiaryEntry", Members::Only(&["date"])),
    ("DiaryEntryPatch", Members::Only(&[])),
    ("NewCourse", Members::AsWritten),
    ("UnitKind", Members::AsWritten),
    ("UnitNode", Members::All),
    ("Unit", Members::All),
    ("UnitWithReactions", Members::AllBut(&["reactions"])),
    ("DeletedUnit", Members::All),
    ("NewUnit", Members::Only(&[])),
    ("UnitMove", Members::Only(&[])),
    ("UnitPatch", Members::Only(&[])),
    (
        "UnitProgress",
        Members::AllBut(&["suggestedNextIntervalDays"]),
    ),
    ("UnitProgressUpdate", Members::Only(&[])),
    ("MemberCourseProgress", Members::All),
    ("CourseProgressSummary", Members::All),
    ("AttendanceStatus", Members::AsWritten),
    ("MeetingAttendance", Members::All),
    ("CourseMeeting", Members::All),
    ("NewCourseMeeting", Members::Only(&["title", "startsAt"])),
    ("CourseMeetingPatch", Members::Only(&[])),
    ("MeetingAttendanceUpdate", Members::All),
    ("MyExamScore", Members::All),
    ("CourseExam", Members::All),
    ("NewCourseExam", Members::Only(&["title", "maxScore"])),
    ("CourseExamPatch", Members::Only(&[])),
    ("MyExamScoreUpdate", Members::Only(&["score"])),
    ("UnitReview", Members::All),
    ("ScheduleUnitReview", Members::Only(&[])),
    ("CompleteUnitReview", Members::Only(&[])),
    ("CompletedUnitReview", Members::All),
    ("TodayReviewItem", Members::All),
    ("UnitNoteKind", Members::AsWritten),
    ("UnitNote", Members::All),
    ("CreateUnitNote", Members::Only(&["kind", "content"])),
    ("PatchUnitNote", Members::Only(&[])),
    ("AmountUnit", Members::AsWritten),
    ("CourseSchedule", Members::All),
    (
        "CourseScheduleUpdate",
        Members::Only(&["startDate", "targetDate", "studyDays"]),
    ),
    ("CourseScheduleView", Members::All),
    ("ScheduledUnit", Members::All),
    ("RedistributeScheduleRequest", Members::Only(&[])),
    ("UnitScheduleUpdate", Members::All),
    ("LearningTodayItem", Members::All),
    ("WorkspaceLearningToday", Members::All),
    ("NewUnitComment", Members::Only(&["body"])),
    ("UnitCommentPatch", Members::Only(&["body", "readAt"])),
    ("NewCardComment", Members::Only(&["body"])),
    ("CardCommentPatch", Members::Only(&["body", "readAt"])),
    ("CardMovesRequest", Members::Only(&["cardIds"])),
    ("CardBatchIds", Members::All),
    ("CardBatchPatch", Members::Only(&["cardIds"])),
    ("NewCardAttachment", Members::All),
    ("CategoryPatch", Members::Only(&[])),
    ("CategoryContainerUpdate", Members::Only(&[])),
    (
        "CopyContainerToWorkspaceRequest",
        Members::Only(&["targetWorkspaceId"]),
    ),
    ("NewBoardView", Members::Only(&["title", "viewType"])),
    ("BoardViewMove", Members::Only(&[])),
    ("BoardViewPatch", Members::Only(&[])),
    ("BoardViewDuplicate", Members::Only(&[])),
    ("ViewType", Members::AsWritten),
    ("NewCard", Members::AsWritten),
    ("NewCategory", Members::AsWritten),
    ("NewFileVersion", Members::AsWritten),
    ("NewInvitation", Members::Only(&["role"])),
    ("NewMember", Members::AsWritten),
    ("NewSubscription", Members::AsWritten),
    ("NewUpload", Members::Only(&["name", "size"])),
    ("OrganizationPatch", Members::Only(&[])),
    ("NewApiToken", Members::Only(&["name", "scope"])),
    ("MarkdownAppend", Members::Only(&["markdown"])),
    ("OrgRoleChange", Members::AsWritten),
    ("WorkspaceName", Members::AsWritten),
    (
        "OidcProviderSettings",
        Members::Only(&["issuer", "clientId"]),
    ),
    ("OidcStartRequest", Members::Only(&[])),
    ("DeleteAccountRequest", Members::Only(&["username"])),
    (
        "RegisterRequest",
        Members::Only(&["username", "email", "password"]),
    ),
    ("ConfirmPasswordRequest", Members::All),
    ("NewOrganization", Members::All),
    ("SetPasswordRequest", Members::All),
    ("SettingsChange", Members::AsWritten),
    ("PublicLinkRequest", Members::AsWritten),
    (
        "UnlinkRequest",
        Members::Only(&["issuer", "subject", "password"]),
    ),
    ("StartTimeEntryRequest", Members::Only(&["kind"])),
    (
        "CreateTimeEntryRequest",
        Members::Only(&["kind", "startedAt", "endedAt"]),
    ),
    ("TimeEntryPatch", Members::Only(&[])),
    ("ReferenceIds", Members::AsWritten),
    ("UserIds", Members::AsWritten),
    ("ClientStatePatch", Members::Only(&[])),
    // Closed sets of words.
    ("BlockType", Members::AsWritten),
    ("CardOpen", Members::AsWritten),
    ("DeletionProof", Members::AsWritten),
    ("CategoryType", Members::AsWritten),
    ("ContainerKind", Members::AsWritten),
    ("ContainerRole", Members::AsWritten),
    ("ContainerVisibility", Members::AsWritten),
    ("Credential", Members::AsWritten),
    ("Density", Members::AsWritten),
    ("HourCycle", Members::AsWritten),
    ("LinkedKind", Members::AsWritten),
    ("MinimumRole", Members::AsWritten),
    ("NameShown", Members::AsWritten),
    ("PropertyColor", Members::AsWritten),
    ("PropertyType", Members::AsWritten),
    ("Theme", Members::AsWritten),
    ("Title", Members::AsWritten),
    ("WeekStart", Members::AsWritten),
];

/// Paths whose parameter names the router cannot have: a position of the router has one
/// name, and these two routes read the first position after `/subscriptions` as
/// different things. The document names each as what it is.
const RENAMED_PATHS: [(&str, &str); 2] = [
    (
        "/api/v1/w/{workspaceId}/subscriptions/{blockOrUserId}/{userId}",
        "/api/v1/w/{workspaceId}/subscriptions/{blockId}/{userId}",
    ),
    (
        "/api/v1/w/{workspaceId}/subscriptions/{blockOrUserId}",
        "/api/v1/w/{workspaceId}/subscriptions/{userId}",
    ),
];

/// The methods an OpenAPI path item has operations under.
const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// The document could not be made: the routes or the schemas say something the contract
/// cannot be written from. The message names what.
#[derive(Debug, thiserror::Error)]
#[error("the API contract cannot be made: {0}")]
pub struct ContractError(String);

/// The OpenAPI document of the API, as the text of `contracts/openapi.json`: JSON with
/// two spaces of indentation, the members of every object in the order of their names,
/// and a line break at the end.
///
/// # Errors
///
/// Returns a [`ContractError`] if a route or a schema is not fit for the contract: an
/// operation without a name or a summary, a path parameter nothing describes, a schema
/// the table of schemas in this module does not list.
pub fn document() -> Result<String, ContractError> {
    let routes = super::routes();
    let api = OpenApiRouter::<App>::new()
        .merge(routes.json)
        .merge(routes.unauthenticated)
        .merge(routes.uploads)
        .merge(routes.shared);
    let (_, openapi) = OpenApiRouter::<App>::new()
        .nest(super::PREFIX, api)
        .split_for_parts();

    let mut document = serde_json::to_value(&openapi)
        .map_err(|error| ContractError(format!("the document is no JSON: {error}")))?;
    write_head(&mut document);
    finish_schemas(&mut document)?;
    finish_inline_schemas(object_at(&mut document, "/paths")?);
    plain_descriptions(&mut document);
    rename_paths(&mut document)?;
    finish_operations(&mut document)?;

    let mut text = serde_json::to_string_pretty(&sorted(document))
        .map_err(|error| ContractError(format!("the document is no JSON: {error}")))?;
    text.push('\n');
    Ok(text)
}

/// Writes what the document says about the API as a whole: what it is, how a request
/// shows who sends it, and the groups of its operations.
fn write_head(document: &mut Value) {
    document["info"] = json!({
        "title": "fvoci",
        "version": env!("CARGO_PKG_VERSION"),
        "description": DESCRIPTION,
        "license": { "name": env!("CARGO_PKG_LICENSE"), "identifier": env!("CARGO_PKG_LICENSE") },
    });
    // The paths are those of the server the document comes with, wherever that is.
    document["servers"] = json!([{ "url": "/", "description": "The server itself." }]);
    document["tags"] = TAGS
        .iter()
        .map(|(name, description)| json!({ "name": name, "description": description }))
        .collect();
    document["components"]["securitySchemes"] = json!({
        SESSION_COOKIE: {
            "type": "apiKey",
            "in": "cookie",
            "name": "__Host-fvoci_session",
            "description": "The session cookie `login` sets. Over plain HTTP its name is `fvoci_session`.",
        },
        BEARER_TOKEN: {
            "type": "http",
            "scheme": "bearer",
            "description": "The value of the session cookie, for clients that are no browsers.",
        },
    });
    // What an operation needs unless it says otherwise: one of the two.
    document["security"] = json!([{ SESSION_COOKIE: [] }, { BEARER_TOKEN: [] }]);
}

/// The members of a JSON object that is expected at `pointer`.
fn object_at<'v>(
    document: &'v mut Value,
    pointer: &str,
) -> Result<&'v mut Map<String, Value>, ContractError> {
    document
        .pointer_mut(pointer)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ContractError(format!("the document has no object at {pointer}")))
}

/// Brings the schemas to what the contract says: `Problem` is among them, none
/// carries the defaults of the decoding, and each has the members [`SCHEMAS`] says are
/// always there.
fn finish_schemas(document: &mut Value) -> Result<(), ContractError> {
    let schemas = object_at(document, "/components/schemas")?;
    for (name, schema) in [
        ("Problem", Problem::schema()),
        ("InvalidInput", InvalidInput::schema()),
    ] {
        let schema = serde_json::to_value(schema)
            .map_err(|error| ContractError(format!("{name} has no schema: {error}")))?;
        schemas.insert(name.to_owned(), schema);
    }
    schemas["Problem"]["x-problem-types"] = problem_types();

    let listed: BTreeSet<&str> = SCHEMAS.iter().map(|(name, _)| *name).collect();
    if let Some(unlisted) = schemas.keys().find(|name| !listed.contains(name.as_str())) {
        return Err(ContractError(format!(
            "the schema {unlisted} is not listed in api::contract::SCHEMAS"
        )));
    }
    for (name, members) in SCHEMAS {
        let schema = schemas.get_mut(*name).ok_or_else(|| {
            ContractError(format!(
                "api::contract::SCHEMAS lists {name}, which no operation uses"
            ))
        })?;
        strip_defaults(schema);
        open_free_objects(schema);
        require_members(name, schema, *members)?;
    }
    Ok(())
}

/// Every kind of problem, as the contract lists them: the `type` on a server at the
/// root of its host (a server under a base path has the base path before it), the
/// `title`, and the `status` or the `reason` the kind is of.
fn problem_types() -> Value {
    crate::problems::all()
        .into_iter()
        .map(|problem| {
            let mut listed = json!({
                "type": problem.type_reference(),
                "title": problem.title,
            });
            match problem.basis {
                Basis::Status(status) => listed["status"] = json!(status),
                Basis::Reason(reason) => listed["reason"] = json!(reason),
            }
            listed
        })
        .collect()
}

/// Does for the schemas that are written where they are used, in the parameters, the
/// request bodies and the answers of the operations, what [`finish_schemas`] does for
/// the named ones: none carries a default of the decoding, and an object without
/// members may have any.
fn finish_inline_schemas(members: &mut Map<String, Value>) {
    for (name, member) in members {
        if name == "schema" {
            strip_defaults(member);
            open_free_objects(member);
        } else {
            match member {
                Value::Object(inner) => finish_inline_schemas(inner),
                Value::Array(items) => {
                    for item in items.iter_mut().filter_map(Value::as_object_mut) {
                        finish_inline_schemas(item);
                    }
                }
                _ => {}
            }
        }
    }
}

/// Removes the `default` of a schema and of every schema inside it. The defaults are
/// those of the decoding: what a member is taken as when a request leaves it out. In an
/// answer nothing is left out, and for requests the descriptions say it.
fn strip_defaults(schema: &mut Value) {
    let Some(members) = schema.as_object_mut() else {
        return;
    };
    members.remove("default");
    if let Some(properties) = members.get_mut("properties").and_then(Value::as_object_mut) {
        properties.values_mut().for_each(strip_defaults);
    }
    for name in ["items", "additionalProperties"] {
        if let Some(inner) = members.get_mut(name) {
            strip_defaults(inner);
        }
    }
    for name in ["allOf", "oneOf", "anyOf"] {
        if let Some(parts) = members.get_mut(name).and_then(Value::as_array_mut) {
            parts.iter_mut().for_each(strip_defaults);
        }
    }
}

/// Says of every object the schema gives no members for that it may have any: the
/// free-form objects, such as the fields of a block. An object schema that says nothing
/// reads the same to a validator, but a generator of types takes it for an object that
/// is always empty.
fn open_free_objects(schema: &mut Value) {
    let Some(members) = schema.as_object_mut() else {
        return;
    };
    let is_object = match members.get("type") {
        Some(Value::String(name)) => name == "object",
        Some(Value::Array(names)) => names.iter().any(|name| name == "object"),
        _ => false,
    };
    let says_its_members = [
        "properties",
        "additionalProperties",
        "allOf",
        "oneOf",
        "anyOf",
    ]
    .iter()
    .any(|name| members.contains_key(*name));
    if is_object && !says_its_members {
        members.insert("additionalProperties".to_owned(), Value::Bool(true));
    }
    if let Some(properties) = members.get_mut("properties").and_then(Value::as_object_mut) {
        properties.values_mut().for_each(open_free_objects);
    }
    // `additionalProperties: true` is no schema and is passed over as no object.
    for name in ["items", "additionalProperties"] {
        if let Some(inner) = members.get_mut(name) {
            open_free_objects(inner);
        }
    }
    for name in ["allOf", "oneOf", "anyOf"] {
        if let Some(parts) = members.get_mut(name).and_then(Value::as_array_mut) {
            parts.iter_mut().for_each(open_free_objects);
        }
    }
}

/// Sets which members of the schema `name` are always there.
fn require_members(name: &str, schema: &mut Value, members: Members) -> Result<(), ContractError> {
    if matches!(members, Members::AsWritten) {
        return Ok(());
    }
    // A type that has another's members inline is the two side by side; its own
    // members are in the part that is no reference.
    let own = match schema.get_mut("allOf").and_then(Value::as_array_mut) {
        Some(parts) => parts
            .iter_mut()
            .find(|part| part.get("properties").is_some()),
        None => Some(schema),
    };
    let own = own
        .and_then(Value::as_object_mut)
        .ok_or_else(|| ContractError(format!("the schema {name} has no members of its own")))?;
    let names: Vec<String> = own
        .get("properties")
        .and_then(Value::as_object)
        .map_or_default(|properties| properties.keys().cloned().collect());
    let unknown = |listed: &[&str]| {
        listed
            .iter()
            .find(|member| !names.iter().any(|name| name == *member))
            .map(|member| ContractError(format!("the schema {name} has no member {member}")))
    };
    let required: Vec<String> = match members {
        Members::AsWritten => return Ok(()),
        Members::All => names,
        Members::AllBut(optional) => {
            if let Some(error) = unknown(optional) {
                return Err(error);
            }
            names
                .into_iter()
                .filter(|name| !optional.contains(&name.as_str()))
                .collect()
        }
        Members::Only(required) => {
            if let Some(error) = unknown(required) {
                return Err(error);
            }
            required.iter().map(|&member| member.to_owned()).collect()
        }
    };
    if required.is_empty() {
        own.remove("required");
    } else {
        own.insert("required".to_owned(), Value::from(required));
    }
    Ok(())
}

/// Gives the paths of [`RENAMED_PATHS`] the names the contract has for them.
fn rename_paths(document: &mut Value) -> Result<(), ContractError> {
    let paths = object_at(document, "/paths")?;
    for (routed, named) in RENAMED_PATHS {
        let item = paths
            .remove(routed)
            .ok_or_else(|| ContractError(format!("there is no path {routed} to rename")))?;
        if paths.insert(named.to_owned(), item).is_some() {
            return Err(ContractError(format!("there are two paths {named}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
