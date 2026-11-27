//! An organization that takes no password, against the fake provider of the tests:
//! which sessions open what is the organization's, what keeps the organization from
//! locking itself out, and that nothing of it is reached around the wall.

use fvoci_model::{Block, Container, ContainerVisibility, OidcProvider, UserWorkspace, new_id};
use fvoci_server::auth::secret::token_digest;
use fvoci_store::Identity;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, PASSWORD, ROOT_WORKSPACE_ID, Setup, TestServer, USER1_USERNAME,
};
use crate::fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider, Person};
use crate::oidc::{Browser, org_of, server_for, set_provider, with_key_and_private_addresses};

/// The subject `user1` has at the fake providers of these tests.
pub(crate) const ADMIN_SUBJECT: &str = "subject-of-user1";

/// Ties what `fake` vouches for under `subject` to the account of `client`'s user, as
/// linking does.
pub(crate) async fn tie(ts: &TestServer, client: &Client, fake: &FakeProvider, subject: &str) {
    let me = client.me().await;
    ts.app
        .store()
        .add_identity(
            &me.id,
            Identity {
                issuer: &fake.issuer,
                subject,
            },
            &me.email,
        )
        .await
        .expect("the identity is tied to the account");
}

/// A session of the person `subject` made through the provider of the organization.
pub(crate) async fn through_provider(
    ts: &TestServer,
    fake: &FakeProvider,
    org_id: &str,
    subject: &str,
) -> Client {
    fake.sign_in_as(Person::new(subject, &format!("{subject}@example.com")));
    let token = Browser::new(ts, fake)
        .sign_in(org_id)
        .await
        .token()
        .to_owned();
    ts.client_with_token(&token)
}

/// `user1`, who administers the organization, in a session made through its provider.
pub(crate) async fn admin_through_provider(
    ts: &TestServer,
    fake: &FakeProvider,
    org_id: &str,
) -> Client {
    tie(ts, &ts.client, fake, ADMIN_SUBJECT).await;
    through_provider(ts, fake, org_id, ADMIN_SUBJECT).await
}

/// Whether an organization takes a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Passwords {
    Taken,
    Refused,
}

/// Sets `ssoRequired` of the organization's provider, the fake one, as `client`.
pub(crate) async fn require_sso(
    client: &Client,
    org_id: &str,
    fake: &FakeProvider,
    passwords: Passwords,
) -> ApiResponse {
    client
        .put(
            &format!("/org/{org_id}/oidc"),
            &json!({
                "issuer": fake.issuer,
                "clientId": CLIENT_ID,
                "ssoRequired": passwords == Passwords::Refused,
            }),
        )
        .await
}

/// Checks the refusal of a change to an organization's provider: 409 with the reason.
#[track_caller]
fn check_refused(response: &ApiResponse, reason: &str) {
    response.check_status(StatusCode::CONFLICT);
    assert_eq!(response.error_reason(), reason, "{}", response.text());
}

/// Checks the refusal of a session that does not open the organization: 401, the
/// problem `sso-required`, and the organization to sign in to.
#[track_caller]
fn check_sso_required(response: &ApiResponse, org_id: &str) {
    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.text()
    );
    let problem = response.check_problem();
    assert_eq!(problem["type"], "/problems/sso-required", "{problem}");
    assert_eq!(problem["reason"], "sso_required", "{problem}");
    assert_eq!(problem["orgId"], org_id, "{problem}");
}

fn closed(workspaces: &[UserWorkspace]) -> Vec<bool> {
    workspaces
        .iter()
        .map(|workspace| workspace.sso_required)
        .collect()
}

/// From the request after it is switched on, a session made with a password opens
/// nothing of the organization, under a workspace or under the organization, and goes
/// on as the account's own; the session made through the provider
/// opens everything. Switched off, the same password session opens it again: no session
/// was ended.
#[tokio::test]
async fn an_organization_that_takes_no_password_opens_for_its_providers_sessions_only() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let board = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    let boards = format!("/w/{ROOT_WORKSPACE_ID}/boards");
    let provider = format!("/org/{org_id}/oidc");
    let by_id = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
    ts.client.get(&boards).await.check_ok();
    let listed: Vec<UserWorkspace> = ts.client.get("/users/me/workspaces").await.ok_json();
    assert!(closed(&listed).iter().all(|closed| !closed), "{listed:?}");

    let stored: OidcProvider = require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .ok_json();
    assert!(stored.sso_required);

    // The sessions that were there, and a password login made now.
    let fresh = ts.login(USER1_USERNAME, PASSWORD).await;
    for client in [&ts.client, &ts.client2, &fresh] {
        for path in [&boards, &by_id] {
            check_sso_required(&client.get(path).await, &org_id);
        }
        client.get("/users/me").await.check_ok();
        client.get("/users/me/settings").await.check_ok();
        let orgs: Vec<Value> = client.get("/users/me/organizations").await.ok_json();
        assert!(
            orgs.iter().any(|org| org["id"] == org_id),
            "getMyOrganizations lists the walled organization: {orgs:?}"
        );
        // The user's own space, which no organization closes, is checked to ask
        // for no provider where it is listed.
        let (listed, _own) = crate::own_space::workspaces_and_own(client).await;
        assert!(!listed.is_empty(), "the workspaces are listed all the same");
        assert!(closed(&listed).iter().all(|closed| *closed), "{listed:?}");
        assert!(listed.iter().all(|workspace| workspace.org_id == org_id));
    }
    check_sso_required(&ts.client.get(&provider).await, &org_id);
    // Who does not administer the organization is told what they were told before.
    ts.client2.get(&provider).await.check_not_found();
    check_sso_required(
        &ts.client
            .patch(&by_id, &json!({ "title": "not through the wall" }))
            .await,
        &org_id,
    );

    for path in [&boards, &provider, &by_id] {
        admin.get(path).await.check_ok();
    }
    let listed: Vec<UserWorkspace> = admin.get("/users/me/workspaces").await.ok_json();
    assert!(closed(&listed).iter().all(|closed| !closed), "{listed:?}");
    let unchanged: Container = admin.get(&by_id).await.ok_json();
    assert_eq!(unchanged.title, board.title);

    let released: OidcProvider = require_sso(&admin, &org_id, &fake, Passwords::Taken)
        .await
        .ok_json();
    assert!(!released.sso_required);
    for client in [&ts.client, &ts.client2, &fresh] {
        client.get(&boards).await.check_ok();
        client.get(&by_id).await.check_ok();
    }
    ts.teardown().await;
    fake.stop();
}

/// Taking no password is switched on from a session made through the very provider,
/// and while it is on the provider is not switched off, removed, or set to another
/// issuer or client.
#[tokio::test]
async fn an_organization_does_not_lock_itself_out() {
    let fake = FakeProvider::start().await;
    let other = FakeProvider::start().await;
    let ts = Setup::native()
        .trusting_oidc_provider(&fake.certificate_der)
        .trusting_oidc_provider(&other.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    let path = format!("/org/{org_id}/oidc");
    let settings = |provider: &FakeProvider, more: Value| {
        let mut settings = json!({
            "issuer": provider.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET,
        });
        for (name, value) in more.as_object().expect("an object") {
            settings[name] = value.clone();
        }
        settings
    };

    // No provider yet, and a password: nobody has signed in through it.
    check_refused(
        &ts.client
            .put(&path, &settings(&fake, json!({ "ssoRequired": true })))
            .await,
        "sso_unproven",
    );
    ts.client.get(&path).await.check_not_found();
    set_provider(&ts, &org_id, &fake).await;
    check_refused(
        &require_sso(&ts.client, &org_id, &fake, Passwords::Refused).await,
        "sso_unproven",
    );

    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    // Another issuer is another provider, which the session was not made through; and
    // one that is switched off lets nobody in.
    for unproven in [
        settings(&other, json!({ "ssoRequired": true })),
        settings(&fake, json!({ "ssoRequired": true, "enabled": false })),
    ] {
        check_refused(&admin.put(&path, &unproven).await, "sso_unproven");
    }
    let read: OidcProvider = admin.get(&path).await.ok_json();
    assert!(!read.sso_required && read.enabled && read.issuer == fake.issuer);

    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    // Each refusal names what of the request would let nobody in.
    for (held, at) in [
        (settings(&fake, json!({ "enabled": false })), "/enabled"),
        (
            settings(&other, json!({ "ssoRequired": true, "enabled": false })),
            "/enabled",
        ),
    ] {
        let response = admin.put(&path, &held).await;
        check_refused(&response, "sso_enforced");
        assert_eq!(response.refused_at(), at, "{held}");
    }
    // Another issuer or client is set aside instead (sso_pending.rs).
    check_refused(&admin.delete(&path).await, "sso_enforced");
    let read: OidcProvider = admin.get(&path).await.ok_json();
    assert!(read.sso_required && read.enabled && read.issuer == fake.issuer);
    assert_eq!(read.client_id, CLIENT_ID);

    // What lets people in as before is changed as ever, and the session still opens.
    let changed: OidcProvider = admin
        .put(&path, &settings(&fake, json!({ "autoJoin": false })))
        .await
        .ok_json();
    assert!(changed.sso_required && !changed.auto_join);
    admin.get(&path).await.check_ok();

    // Switched off also while the provider does not answer: the issuer that stays is
    // not read again, so who is in need not wait for it to get everybody back in.
    fake.redirect_discovery();
    let released: OidcProvider = admin
        .put(&path, &settings(&fake, json!({ "ssoRequired": false })))
        .await
        .ok_json();
    assert!(!released.sso_required);
    ts.client.get(&path).await.check_ok();
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();

    // Switched off with the change, the provider is another's to become.
    let replaced: OidcProvider = admin
        .put(&path, &settings(&other, json!({ "ssoRequired": false })))
        .await
        .ok_json();
    assert!(!replaced.sso_required && replaced.issuer == other.issuer);
    // The session made through the issuer before ends with it, also the one that
    // replaced it.
    admin.get(&path).await.check_unauthorized();
    ts.client.delete(&path).await.check_no_content();
    ts.teardown().await;
    fake.stop();
    other.stop();
}

/// When the provider is replaced by one of another issuer, the sessions made through
/// the one before end: they open nothing, the organization's notice included, and
/// prove nothing of the new provider. The new provider's session goes on.
#[tokio::test]
async fn a_change_of_issuer_ends_the_sessions_of_the_provider_before() {
    let before = FakeProvider::start().await;
    let now = FakeProvider::start().await;
    let ts = Setup::native()
        .trusting_oidc_provider(&before.certificate_der)
        .trusting_oidc_provider(&now.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    let boards = format!("/w/{ROOT_WORKSPACE_ID}/boards");
    set_provider(&ts, &org_id, &before).await;
    let old = admin_through_provider(&ts, &before, &org_id).await;
    require_sso(&old, &org_id, &before, Passwords::Refused)
        .await
        .check_ok();
    old.get(&boards).await.check_ok();
    require_sso(&old, &org_id, &before, Passwords::Taken)
        .await
        .check_ok();
    let session_of = |token: String| {
        let store = ts.app.store().clone();
        async move { store.get_session(&token_digest(&token), 0, 0).await }
    };
    let old_origin = session_of(old.token.clone())
        .await
        .expect("stored")
        .login
        .expect("made through the provider");
    assert_eq!(old_origin.issuer, before.issuer);
    assert_eq!(old_origin.org_id, org_id);

    set_provider(&ts, &org_id, &now).await;
    assert!(
        session_of(old.token.clone()).await.is_err(),
        "the session of the issuer before is ended"
    );
    old.get(&boards).await.check_unauthorized();
    let ended = require_sso(&old, &org_id, &now, Passwords::Refused).await;
    ended.check_unauthorized();
    let new = admin_through_provider(&ts, &now, &org_id).await;
    require_sso(&new, &org_id, &now, Passwords::Refused)
        .await
        .check_ok();
    new.get(&boards).await.check_ok();
    ts.teardown().await;
    before.stop();
    now.stop();
}

/// Who runs the server releases an organization on the administration socket, and
/// nowhere else: the way back in when nobody gets in through the provider.
#[cfg(unix)]
#[tokio::test]
async fn the_administration_socket_releases_an_organization() {
    use crate::admin::socket::{socket_path, socket_request};

    let fake = FakeProvider::start().await;
    let socket = socket_path();
    let ts = Setup::native()
        .admin_socket(&socket)
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();
    let boards = format!("/w/{ROOT_WORKSPACE_ID}/boards");
    check_sso_required(&ts.client.get(&boards).await, &org_id);
    let release = format!("/api/v1/admin/orgs/{org_id}/sso/release");

    // Over TCP there is no such route, for nobody: with a session, with the session of
    // the provider, without one.
    for client in [&ts.client, &admin, &ts.anonymous()] {
        let response = client
            .post_empty(&format!("/admin/orgs/{org_id}/sso/release"))
            .await;
        response.check_not_found();
        assert_eq!(response.error_message(), "route not found");
    }
    check_sso_required(&ts.client.get(&boards).await, &org_id);

    socket_request(&socket, "POST", &release, "")
        .await
        .check_no_content();
    ts.client.get(&boards).await.check_ok();
    let provider: OidcProvider = ts
        .client
        .get(&format!("/org/{org_id}/oidc"))
        .await
        .ok_json();
    assert!(!provider.sso_required, "released");
    assert!(
        provider.enabled && provider.issuer == fake.issuer,
        "and otherwise as it was"
    );
    // Released already, it stays so; and what names no organization is not there.
    socket_request(&socket, "POST", &release, "")
        .await
        .check_no_content();
    let records: Vec<Value> = ts
        .client
        .get(&format!("/org/{org_id}/audit"))
        .await
        .ok_json();
    let operators: Vec<_> = records
        .iter()
        .filter(|record| record["actor"]["kind"] == "server_operator")
        .collect();
    assert_eq!(
        operators.len(),
        1,
        "only the effective socket release is recorded"
    );
    assert_eq!(
        operators[0]["actor"],
        serde_json::json!({ "kind": "server_operator" }),
        "the operator, with nothing of a person"
    );
    assert_eq!(operators[0]["action"], "sso_released");
    for missing in [new_id(), "no-identifier".to_owned()] {
        socket_request(
            &socket,
            "POST",
            &format!("/api/v1/admin/orgs/{missing}/sso/release"),
            "",
        )
        .await
        .check_error(404, "not-found", "organization not found");
    }
    ts.teardown().await;
    fake.stop();
}

/// The operations of the contract that are the account's own, come before there is a
/// session, or read a board or wiki by its public link, which asks for none: what a session
/// that does not open the organization still does.
const OPEN_TO_ANY_SESSION: &[&str] = &[
    // Deleting the account is the account's own, and so is leaving an organization by
    // it. An organization that would go with the account is asked for
    // (`account_deletion::an_organization_behind_the_wall_does_not_go_by_a_password`).
    "cancelAccountDeletion",
    // The account's address is the account's own: asked to change under its password
    // (`requestEmailChange`), and changed by whoever holds the link mailed to the new
    // one, signed in or not (`email_change`).
    "cancelEmailChange",
    "changePassword",
    // Confirming a registration's address asks for no session: there is no account yet.
    "completeEmailVerification",
    // Giving the account's password in a session is the account's own: it opens the
    // account's own space to the session, and nothing of an organization the session
    // does not open (`confirmed_password`).
    "confirmEmailChange",
    "confirmPassword",
    // Recovering a password asks for no session; what it changes is the account's own.
    "completePasswordReset",
    // The account's personal API tokens are the account's own: a workspace of an
    // organization the session does not open is given to none
    // (`api_tokens::an_organization_that_takes_no_password_gives_its_workspaces_to_tokens_while_it_lets`).
    "createApiToken",
    // Starting an organization is the account's own: the new one is no part of the
    // organization the session does not open, and nothing of that one is read for it.
    "createOrganization",
    // Personal time entries live across organizations, leaving out any organization the
    // session does not open (`time_entries_obey_the_sso_wall`, `time_entries_isolation`):
    // `startTimeEntry` and `createTimeEntry` stay in the sweep below targeting the walled
    // organization's card.
    "deleteTimeEntry",
    // The two ends of a sign-in held for a browser come before there is a session, as
    // the sign-in itself does: they look at the browser's cookie of it, and at none.
    "finishHeldOidcLogin",
    "finishOidcLogin",
    "getClientConfig",
    "getAccountDeletion",
    // Aggregates across organizations opened by the session, excluding walled organizations
    // and counting their locked workspaces (`lockedWorkspaceCount`).
    "getAccountToday",
    "getActiveTimeEntry",
    "getApiTokens",
    "getClientState",
    "getLoginOptions",
    "getMe",
    "getMyIdentities",
    // Lists across organizations, leaving out the boards of one the session does not
    // open: asked below.
    "getMyMemberships",
    // Listing the account's organizations is the account's own, so settings can show
    // and leave an organization even when it takes only sessions through its provider.
    "getMyOrganizations",
    "getMyWorkspaces",
    // What the account was told, in every organization its session opens: those of
    // one it does not open are left out of the list and of the count, and are not
    // marked as read
    // (`notifications_isolation::a_session_its_organization_does_not_take_reaches_nothing_it_was_told_there`).
    "getNotifications",
    "getUnreadNotifications",
    "getSharedBlocks",
    "getSharedContainer",
    "getSharedCards",
    "getSharedFile",
    "getSharedFileRecord",
    "getSharedPage",
    "getSharedPageTree",
    "getSharedUnit",
    "getSharedUnitTree",
    "getUserSettings",
    "hello",
    // Leaving an organization is the account's own, as deleting the account takes it
    // out of one: the session that leaves need not open it.
    "leaveOrganization",
    "listTimeEntries",
    "login",
    "logout",
    "patchClientState",
    "patchMe",
    "patchTimeEntry",
    "patchUserSettings",
    "ping",
    // The MCP server takes a personal API token and no session: a session is answered
    // 401 before anything is read. What a token reaches behind the wall is asked as a
    // tool enters its workspace
    // (`api_tokens::an_organization_that_takes_no_password_gives_its_workspaces_to_tokens_while_it_lets`).
    "postMcp",
    // What a link leads into is shown to whoever holds it, signed in or not.
    "previewInvitation",
    // Marking as read what the account was told, as listing it (`getNotifications`):
    // nothing of an organization the session does not open is marked.
    "readNotification",
    "readNotifications",
    "ready",
    "register",
    "requestAccountDeletion",
    "requestEmailChange",
    "requestEmailVerification",
    "requestPasswordReset",
    "resendEmailChange",
    // Giving an account the address it had back asks for no session: whoever holds the
    // link mailed to that address uses it (`email_revert`).
    "revertEmailChange",
    "revokeApiToken",
    // The first password of an account that has none is the account's own, as
    // changing one is.
    "setPassword",
    "startOidcCarryOver",
    "startOidcLogin",
    "stopTimeEntry",
    "unlinkIdentity",
];

/// What the sweep names in the paths, and a file of the board.
struct Named {
    org_id: String,
    board: Container,
    card_id: String,
    course_id: String,
    unit_id: String,
    unit_comment_id: String,
    meeting_id: String,
    exam_id: String,
    review_id: String,
    note_id: String,
    file_id: String,
    upload_id: String,
    user_id: String,
    category_id: String,
    /// The token of a link for members into the organization's root workspace.
    invitation: String,
}

impl Named {
    fn path(&self, template: &str) -> String {
        template
            .trim_start_matches("/api/v1")
            .replace("{workspaceId}", ROOT_WORKSPACE_ID)
            .replace("{orgId}", &self.org_id)
            .replace("{containerId}", &self.board.id)
            .replace("{boardId}", &self.board.id)
            .replace("{cardId}", &self.card_id)
            .replace("{blockId}", &self.card_id)
            .replace("{courseId}", &self.course_id)
            .replace("{unitId}", &self.unit_id)
            .replace("{meetingId}", &self.meeting_id)
            .replace("{examId}", &self.exam_id)
            .replace("{commentId}", &self.unit_comment_id)
            .replace("{reviewId}", &self.review_id)
            .replace("{noteId}", &self.note_id)
            .replace("{version}", "1")
            .replace("{fileId}", &self.file_id)
            .replace("{uploadId}", &self.upload_id)
            .replace("{userId}", &self.user_id)
            .replace("{categoryId}", &self.category_id)
            .replace("{partNumber}", "1")
    }

    /// A body the operation reads before it asks whose the request is, where it does.
    fn body(&self, operation: &str) -> Value {
        match operation {
            "getWorkspaceUsersById" => json!([self.user_id]),
            "createBlocks" | "reorderCardProperties" => json!([]),
            "importArchive" => json!({ "uploadId": self.upload_id }),
            "acceptInvitation" | "previewInvitation" => json!({ "token": self.invitation }),
            "startTimeEntry" => json!({
                "workspaceId": ROOT_WORKSPACE_ID,
                "blockId": self.card_id,
                "kind": "work",
            }),
            "createTimeEntry" => json!({
                "workspaceId": ROOT_WORKSPACE_ID,
                "blockId": self.card_id,
                "kind": "work",
                "startedAt": 1_000,
                "endedAt": 2_000,
            }),
            _ => json!({}),
        }
    }
}

/// Makes what the sweep names, as `user1` before the organization takes no password.
async fn named(ts: &TestServer, org_id: &str) -> Named {
    let board = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let card: Value = ts
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards/{}/cards", board.id),
            &json!({ "title": "A card of the organization" }),
        )
        .await
        .value();
    let card_id = card["id"].as_str().expect("a card").to_owned();
    let course = course_fixtures(&ts.client).await;
    let file = reqwest::multipart::Part::bytes(b"of the organization".to_vec()).file_name("a.txt");
    let uploaded = Client::send(
        ts.client
            .request(
                Method::POST,
                &format!("/w/{ROOT_WORKSPACE_ID}/containers/{}/files", board.id),
            )
            .multipart(reqwest::multipart::Form::new().part("file", file)),
    )
    .await
    .value();
    let file_id = uploaded["fileId"].as_str().expect("a file").to_owned();
    let upload = ts
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/containers/{}/uploads", board.id),
            &json!({ "name": "later.bin", "size": 1 }),
        )
        .await
        .value();
    let upload_id = upload["uploadId"].as_str().expect("an upload").to_owned();
    // The category every user has, made when the sidebar is first read.
    let categories = ts
        .client
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/categories"))
        .await
        .value();
    Named {
        org_id: org_id.to_owned(),
        card_id,
        course_id: course.course,
        unit_id: course.unit,
        unit_comment_id: course.comment,
        meeting_id: course.meeting,
        exam_id: course.exam,
        review_id: course.review,
        note_id: course.note,
        board,
        file_id,
        upload_id,
        user_id: ts.client2.me().await.id,
        category_id: categories[0]["id"].as_str().expect("a category").to_owned(),
        invitation: ts.member_link().await,
    }
}

struct CourseFixtures {
    course: String,
    unit: String,
    comment: String,
    meeting: String,
    exam: String,
    review: String,
    note: String,
}

/// Makes an open course with a unit (holding one body version), a comment, a study meeting,
/// an exam, a review, and a note.
async fn course_fixtures(client: &Client) -> CourseFixtures {
    let course: Value = client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/courses"),
            &json!({ "title": "A course of the organization", "visibility": "open" }),
        )
        .await
        .value();
    let course_id = course["id"].as_str().expect("a course").to_owned();
    let units = format!("/w/{ROOT_WORKSPACE_ID}/courses/{course_id}/units");
    let unit: Value = client
        .post(&units, &json!({ "title": "A unit of the organization" }))
        .await
        .value();
    let unit_id = unit["id"].as_str().expect("a unit").to_owned();
    let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": "unit content"}]}]});
    client
        .patch(
            &format!("{units}/{unit_id}"),
            &json!({"readAt": unit["updatedAt"], "body": body}),
        )
        .await
        .check_ok();
    let comment: Value = client
        .post(
            &format!("{units}/{unit_id}/comments"),
            &json!({"body": body}),
        )
        .await
        .value();
    let comment_id = comment["id"].as_str().expect("a comment").to_owned();
    let meeting: Value = client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/courses/{course_id}/meetings"),
            &json!({ "title": "A meeting", "startsAt": 1_800_000_000_000_i64 }),
        )
        .await
        .value();
    let meeting_id = meeting["id"].as_str().expect("a meeting").to_owned();
    let exam: Value = client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/courses/{course_id}/exams"),
            &json!({ "title": "An exam", "maxScore": 100.0 }),
        )
        .await
        .value();
    let exam_id = exam["id"].as_str().expect("an exam").to_owned();
    let review: Value = client
        .post(
            &format!("{units}/{unit_id}/reviews"),
            &json!({"intervalDays": 3}),
        )
        .await
        .value();
    let review_id = review["id"].as_str().expect("a review").to_owned();
    let note: Value = client
        .post(
            &format!("{units}/{unit_id}/notes"),
            &json!({"kind": "question", "content": "sso test note"}),
        )
        .await
        .value();
    let note_id = note["id"].as_str().expect("a note").to_owned();
    CourseFixtures {
        course: course_id,
        unit: unit_id,
        comment: comment_id,
        meeting: meeting_id,
        exam: exam_id,
        review: review_id,
        note: note_id,
    }
}

/// Every operation of the contract, by its method, its path as written there, and its
/// identifier.
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
    operations
}

/// What one operation of the sweep was answered.
struct Answer {
    status: StatusCode,
    body: Value,
    /// The operation and its answer, to be told.
    told: String,
}

impl Answer {
    fn says_sso_required_of(&self, org_id: &str) -> bool {
        self.status == StatusCode::UNAUTHORIZED
            && self.body["reason"] == "sso_required"
            && self.body["orgId"] == org_id
    }
}

/// Asks every operation of the contract that is not the account's own, as `client`.
async fn sweep(client: &Client, named: &Named) -> Vec<Answer> {
    let mut answers = Vec::new();
    for (method, template, operation) in operations() {
        if OPEN_TO_ANY_SESSION.contains(&operation.as_str()) {
            continue;
        }
        let response = Client::send(
            client
                .request(method.parse().expect("a method"), &named.path(&template))
                .json(&named.body(&operation)),
        )
        .await;
        let body: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
        answers.push(Answer {
            status: response.status,
            told: format!(
                "{operation} ({method} {template}): {} {body}",
                response.status
            ),
            body,
        });
    }
    answers
}

/// A server whose organization takes no password, with what the sweep names, made
/// before, and the session of its administrator made through the provider.
async fn walled(fake: &FakeProvider) -> (TestServer, Named, Client) {
    let ts = Setup::local_mode()
        .trusting_oidc_provider(&fake.certificate_der)
        .configure(with_key_and_private_addresses)
        .start()
        .await
        .init_basic()
        .await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, fake).await;
    let named = named(&ts, &org_id).await;
    let admin = admin_through_provider(&ts, fake, &org_id).await;
    require_sso(&admin, &org_id, fake, Passwords::Refused)
        .await
        .check_ok();
    (ts, named, admin)
}

/// Nothing of an organization that takes no password is reached around the wall: with
/// a password session of its administrator, every operation of the contract that is
/// not the account's own answers that the session does not open the organization,
/// whether it names a workspace, the organization, or neither. An operation that is
/// added is asked too.
#[tokio::test]
async fn nothing_of_the_organization_is_reached_around_the_wall() {
    let fake = FakeProvider::start().await;
    let (ts, named, admin) = walled(&fake).await;

    let answers = sweep(&ts.client, &named).await;
    assert!(
        answers.len() > 60,
        "the contract was read: {} operations",
        answers.len()
    );
    let violations: Vec<&str> = answers
        .iter()
        .filter(|answer| !answer.says_sso_required_of(&named.org_id))
        .map(|answer| answer.told.as_str())
        .collect();
    assert!(
        violations.is_empty(),
        "reached with a session that does not open the organization:\n{}",
        violations.join("\n")
    );
    // Nothing was changed on the way.
    let board: Container = admin
        .get(&format!(
            "/w/{ROOT_WORKSPACE_ID}/containers/{}",
            named.board.id
        ))
        .await
        .ok_json();
    assert_eq!(board.title, named.board.title);
    let cards: Vec<Block> = admin
        .get(&format!(
            "/w/{ROOT_WORKSPACE_ID}/boards/{}/blocks",
            named.board.id
        ))
        .await
        .ok_json();
    assert_eq!(cards.len(), 1, "{cards:?}");

    // The boards a user lists across organizations leave out the walled one's.
    let boards_of = async |client: &Client| -> Vec<String> {
        let memberships: Vec<Value> = client.get("/users/me/memberships").await.ok_json();
        memberships
            .iter()
            .map(|member| {
                member["containerId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    };
    assert!(
        !boards_of(&ts.client).await.contains(&named.board.id),
        "a password session lists no board of the organization"
    );
    assert!(
        boards_of(&admin).await.contains(&named.board.id),
        "the provider's session lists its boards"
    );

    // It was the wall that answered, not what the requests lack: the session the
    // organization takes is told of none of them that it is not signed in.
    let stopped: Vec<String> = sweep(&admin, &named)
        .await
        .into_iter()
        .filter(|answer| answer.status == StatusCode::UNAUTHORIZED)
        .map(|answer| answer.told)
        .collect();
    assert!(
        stopped.is_empty(),
        "refused the session made through the provider:\n{}",
        stopped.join("\n")
    );
    ts.teardown().await;
    fake.stop();
}

/// An import looks at no upload for a session that does not open the workspace's
/// organization: it is told so before the body is read, whatever upload it names, and
/// the upload stays. A workspace that is not there is not there, as for anybody.
#[tokio::test]
async fn an_import_looks_at_no_upload_for_a_session_that_opens_nothing() {
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let upload = ts
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/archive/uploads"),
            &json!({ "name": "archive.zip", "size": 1 }),
        )
        .await;
    upload.check_created();
    let upload_id = upload.value()["uploadId"]
        .as_str()
        .expect("an upload")
        .to_owned();
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    require_sso(&admin, &org_id, &fake, Passwords::Refused)
        .await
        .check_ok();

    let import = async |workspace: &str, upload: &str| {
        ts.client
            .post(
                &format!("/w/{workspace}/archive/import"),
                &json!({ "uploadId": upload }),
            )
            .await
    };
    for upload in [upload_id.clone(), new_id()] {
        check_sso_required(&import(ROOT_WORKSPACE_ID, &upload).await, &org_id);
    }
    import(&new_id(), &upload_id).await.check_not_found();
    // The upload is there as it was: its owner finds it open.
    admin
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/uploads/{upload_id}"))
        .await
        .check_ok();
    ts.teardown().await;
    fake.stop();
}

/// The public link of a board stays open when its organization takes no password: it
/// asks for no session, and reads the same for a session that does not open the
/// organization, for the one that does, and for nobody. The same board by its
/// workspace is closed to the session that does not open the organization.
#[tokio::test]
async fn a_public_link_stays_open_and_reads_the_same_for_a_session_that_opens_nothing() {
    let fake = FakeProvider::start().await;
    let (ts, named, admin) = walled(&fake).await;
    let read_token = ts.share(&named.board.id).await;
    let anonymous = ts.anonymous();
    let board = &named.board.id;
    for (under, rest) in [
        ("containers", String::new()),
        ("boards", "/blocks".to_owned()),
        ("boards", "/cards".to_owned()),
        ("containers", format!("/files/{}/info", named.file_id)),
    ] {
        let path = format!("/{under}/{board}{rest}");
        let shared = format!("/shared/{board}{rest}?readToken={read_token}");
        let as_anybody = anonymous.get(&shared).await;
        as_anybody.check_ok();
        for signed_in in [&ts.client, &admin] {
            let response = signed_in.get(&shared).await;
            response.check_ok();
            assert_eq!(response.value(), as_anybody.value(), "{path}");
        }
        let wrong = format!("/shared/{board}{rest}?readToken=not-the-token");
        assert_eq!(
            ts.client.get(&wrong).await.status,
            anonymous.get(&wrong).await.status,
            "{path}: a wrong token is told what anybody's is"
        );
        let by_workspace = format!("/w/{ROOT_WORKSPACE_ID}{path}");
        check_sso_required(&ts.client.get(&by_workspace).await, &named.org_id);
        admin.get(&by_workspace).await.check_ok();
    }
    // A shared course link also stays open and reads identically behind the SSO wall,
    // while its workspace routes remain blocked for a password session.
    let course = &named.course_id;
    let course_token = ts.share(course).await;
    for (under, rest) in [
        ("containers", String::new()),
        ("courses", "/units".to_owned()),
        ("courses", format!("/units/{}", named.unit_id)),
    ] {
        let path = format!("/{under}/{course}{rest}");
        let shared = format!("/shared/{course}{rest}?readToken={course_token}");
        let as_anybody = anonymous.get(&shared).await;
        as_anybody.check_ok();
        for signed_in in [&ts.client, &admin] {
            let response = signed_in.get(&shared).await;
            response.check_ok();
            assert_eq!(response.value(), as_anybody.value(), "{path}");
        }
        let by_workspace = format!("/w/{ROOT_WORKSPACE_ID}{path}");
        check_sso_required(&ts.client.get(&by_workspace).await, &named.org_id);
        admin.get(&by_workspace).await.check_ok();
    }
    ts.teardown().await;
    fake.stop();
}

/// The two organizations of [`two_organizations`], and `user1` in both.
pub(crate) struct Two {
    pub(crate) ts: TestServer,
    pub(crate) ours: String,
    pub(crate) theirs: String,
    pub(crate) their_workspace: String,
}

/// A server with a second organization, which `user1` administers too, both with the
/// fake provider as theirs.
pub(crate) async fn two_organizations(fake: &FakeProvider) -> Two {
    use fvoci_model::{OrgRole, Workspace, WorkspaceRole};
    use fvoci_server::app::secret_context;
    use fvoci_store::StoredProvider;

    let ts = server_for(fake).await;
    let ours = org_of(&ts).await;
    set_provider(&ts, &ours, fake).await;
    let their_workspace = new_id();
    let theirs = ts
        .app
        .store()
        .create_organization(
            "theirs",
            &Workspace {
                id: their_workspace.clone(),
                ..Workspace::default()
            },
        )
        .await
        .expect("a second organization");
    let their_store = ts.app.store().for_org(&theirs).expect("an identifier");
    let user1 = ts.client.me().await.id;
    their_store
        .add_organization_member(&user1, OrgRole::Admin)
        .await
        .expect("user1 administers theirs");
    their_store
        .add_workspace_member(&their_workspace, &user1, WorkspaceRole::Admin)
        .await
        .expect("user1 is in their workspace");
    their_store
        .put_oidc_provider(&StoredProvider {
            id: String::new(),
            org_id: theirs.clone(),
            issuer: fake.issuer.clone(),
            client_id: CLIENT_ID.to_owned(),
            sealed_client_secret: crate::oidc::secret_key().seal(
                CLIENT_SECRET,
                &secret_context(&theirs, &fake.issuer, CLIENT_ID),
            ),
            auto_join: true,
            enabled: true,
            created_at: 0,
            updated_at: 0,
            sso_required: false,
            revision: 0,
        })
        .await
        .expect("their provider");
    Two {
        ts,
        ours,
        theirs,
        their_workspace,
    }
}

/// A password opens the organization that takes one while another takes none; the
/// session of one organization's provider does not open another that takes none, also
/// where the two providers are one issuer; and who has no place in an organization is
/// told nothing of its taking no password.
#[tokio::test]
async fn one_organizations_wall_is_not_anothers() {
    use fvoci_store::DbType;
    use fvoci_store::testing::db_type_of_the_tests;
    if db_type_of_the_tests() != DbType::Postgres {
        // An SQLite database holds one organization.
        return;
    }
    let fake = FakeProvider::start().await;
    let Two {
        ts,
        ours,
        theirs,
        their_workspace,
    } = two_organizations(&fake).await;
    let board = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let our_boards = format!("/w/{ROOT_WORKSPACE_ID}/boards");
    let their_boards = format!("/w/{their_workspace}/boards");
    let by_id = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
    tie(&ts, &ts.client, &fake, ADMIN_SUBJECT).await;
    let through_ours = through_provider(&ts, &fake, &ours, ADMIN_SUBJECT).await;
    let through_theirs = through_provider(&ts, &fake, &theirs, ADMIN_SUBJECT).await;
    assert_eq!(through_ours.me().await.id, through_theirs.me().await.id);
    require_sso(&through_ours, &ours, &fake, Passwords::Refused)
        .await
        .check_ok();

    // Ours takes no password; theirs does.
    check_sso_required(&ts.client.get(&our_boards).await, &ours);
    check_sso_required(&ts.client.get(&by_id).await, &ours);
    ts.client.get(&their_boards).await.check_ok();
    ts.client
        .get(&format!("/org/{theirs}/oidc"))
        .await
        .check_ok();
    let listed: Vec<UserWorkspace> = ts.client.get("/users/me/workspaces").await.ok_json();
    for workspace in &listed {
        assert_eq!(
            workspace.sso_required,
            workspace.org_id == ours,
            "{workspace:?}"
        );
    }
    assert!(listed.iter().any(|workspace| workspace.org_id == theirs));

    // The session of their provider is no session of ours.
    check_sso_required(&through_theirs.get(&our_boards).await, &ours);
    check_sso_required(&through_theirs.get(&by_id).await, &ours);
    check_sso_required(
        &through_theirs.get(&format!("/org/{ours}/oidc")).await,
        &ours,
    );
    through_theirs.get(&their_boards).await.check_ok();
    // The session of ours opens ours, and theirs, which takes any.
    through_ours.get(&our_boards).await.check_ok();
    through_ours.get(&by_id).await.check_ok();
    through_ours.get(&their_boards).await.check_ok();

    // Theirs takes none either, from a session of its own provider.
    let unproven = require_sso(&through_ours, &theirs, &fake, Passwords::Refused).await;
    unproven.check_status(StatusCode::CONFLICT);
    assert_eq!(unproven.error_reason(), "sso_unproven");
    require_sso(&through_theirs, &theirs, &fake, Passwords::Refused)
        .await
        .check_ok();
    check_sso_required(&through_ours.get(&their_boards).await, &theirs);
    check_sso_required(&ts.client.get(&their_boards).await, &theirs);
    through_theirs.get(&their_boards).await.check_ok();
    through_ours.get(&our_boards).await.check_ok();
    ts.teardown().await;
    fake.stop();
}

/// Who has no place in an organization that takes no password is told what anybody is
/// told of what is not theirs: that it is not there.
#[tokio::test]
async fn who_has_no_place_in_the_organization_learns_nothing_of_its_wall() {
    use fvoci_store::DbType;
    use fvoci_store::testing::db_type_of_the_tests;
    if db_type_of_the_tests() != DbType::Postgres {
        // An SQLite database holds one organization.
        return;
    }
    let fake = FakeProvider::start().await;
    let Two {
        ts,
        ours,
        theirs,
        their_workspace,
    } = two_organizations(&fake).await;
    let board = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    // Somebody of theirs alone: their provider let them in, and into nothing of ours.
    let outsider = through_provider(&ts, &fake, &theirs, "subject-of-an-outsider").await;
    let admin = admin_through_provider(&ts, &fake, &ours).await;
    let before = [
        outsider
            .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
            .await,
        outsider.get(&format!("/org/{ours}/oidc")).await,
        outsider
            .get(&format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id))
            .await,
    ]
    .map(|response| (response.status, response.error_message()));

    require_sso(&admin, &ours, &fake, Passwords::Refused)
        .await
        .check_ok();

    let after = [
        outsider
            .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
            .await,
        outsider.get(&format!("/org/{ours}/oidc")).await,
        outsider
            .get(&format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id))
            .await,
    ]
    .map(|response| (response.status, response.error_message()));
    assert_eq!(after, before, "the same answers as before");
    assert!(
        after
            .iter()
            .all(|(status, _)| *status == StatusCode::NOT_FOUND),
        "{after:?}"
    );
    let (listed, _own) = crate::own_space::workspaces_and_own(&outsider).await;
    assert_eq!(
        listed
            .iter()
            .map(|workspace| (workspace.id.as_str(), workspace.sso_required))
            .collect::<Vec<_>>(),
        vec![(their_workspace.as_str(), false)]
    );
    ts.teardown().await;
    fake.stop();
}

/// Who holds the link of an invitation into an organization that takes no password and
/// is signed in with a password, though no member of it, is told so by the
/// organization the link names, and let in nowhere: the link opens it to a session of
/// its provider.
#[tokio::test]
async fn a_link_into_an_organization_that_takes_no_password_lets_no_password_in() {
    use fvoci_model::User;
    use fvoci_store::DbType;
    use fvoci_store::testing::db_type_of_the_tests;
    if db_type_of_the_tests() != DbType::Postgres {
        // An SQLite database holds one organization.
        return;
    }
    let fake = FakeProvider::start().await;
    let Two {
        ts, ours, theirs, ..
    } = two_organizations(&fake).await;
    // Somebody of theirs alone, who signs in with a password.
    let mut outsider = User {
        id: new_id(),
        username: "theirs-only".to_owned(),
        email: "theirs-only@sample.com".to_owned(),
        password_hash: fvoci_server::auth::password::hash_password(PASSWORD).expect("a hash"),
        ..User::default()
    };
    ts.app
        .store()
        .for_org(&theirs)
        .expect("an identifier")
        .create_user(&mut outsider)
        .await
        .expect("an account of theirs");
    let outsider_client = ts.login("theirs-only", PASSWORD).await;
    let link = ts.member_link().await;
    tie(&ts, &ts.client, &fake, ADMIN_SUBJECT).await;
    let through_ours = through_provider(&ts, &fake, &ours, ADMIN_SUBJECT).await;
    require_sso(&through_ours, &ours, &fake, Passwords::Refused)
        .await
        .check_ok();

    let used_before = used(&ts, &ours).await;
    let refused = outsider_client
        .post("/invitations/accept", &json!({ "token": link }))
        .await;
    check_sso_required(&refused, &ours);
    let theirs_only: Vec<UserWorkspace> =
        outsider_client.get("/users/me/workspaces").await.ok_json();
    assert!(
        theirs_only.iter().all(|workspace| workspace.org_id != ours),
        "let in nowhere: {theirs_only:?}"
    );
    assert_eq!(
        used(&ts, &ours).await,
        used_before,
        "the refusal used nothing of the link"
    );
    ts.teardown().await;
    fake.stop();
}

/// How many links of the organization were used.
async fn used(ts: &TestServer, org_id: &str) -> i64 {
    ts.app
        .store()
        .for_org(org_id)
        .expect("an identifier")
        .count_rows("invitations WHERE uses > 0")
        .await
        .expect("the links are counted")
}

/// A guest of a board of an organization that takes no password gets in through its
/// provider as the guest they are, whether the organization lets newcomers in through
/// it or not: their board opens, and they join nothing else.
#[tokio::test]
async fn a_guest_signs_in_through_the_provider_as_a_guest() {
    for auto_join in [false, true] {
        let fake = FakeProvider::start().await;
        let ts = server_for(&fake).await;
        let org_id = org_of(&ts).await;
        set_provider(&ts, &org_id, &fake).await;
        let board: Container = ts
            .client
            .post(
                &format!("/w/{ROOT_WORKSPACE_ID}/boards"),
                &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": "For a guest"}),
            )
            .await
            .created_json();
        ts.client
            .put(
                &format!("/w/{ROOT_WORKSPACE_ID}/guest-invitations"),
                &json!({"enabled": true}),
            )
            .await
            .check_ok();
        let link: Value = ts
            .client
            .post(
                &format!("/w/{ROOT_WORKSPACE_ID}/invitations"),
                &json!({"role": "guest", "containerId": board.id}),
            )
            .await
            .value();
        let account = json!({
            "username": "guest",
            "email": "guest@example.com",
            "password": PASSWORD,
        });
        ts.anonymous()
            .post(
                "/invitations/accept",
                &json!({"token": link["token"], "account": account}),
            )
            .await
            .check_ok();
        let guest = ts.login("guest", PASSWORD).await;
        let guest_id = guest.me().await.id;
        tie(&ts, &guest, &fake, "guest").await;
        let admin = admin_through_provider(&ts, &fake, &org_id).await;
        admin
            .put(
                &format!("/org/{org_id}/oidc"),
                &json!({
                    "issuer": fake.issuer,
                    "clientId": CLIENT_ID,
                    "autoJoin": auto_join,
                    "ssoRequired": true,
                }),
            )
            .await
            .check_ok();
        let on_the_board = format!("/w/{ROOT_WORKSPACE_ID}/containers/{}", board.id);
        check_sso_required(&guest.get(&on_the_board).await, &org_id);

        let through = through_provider(&ts, &fake, &org_id, "guest").await;
        assert_eq!(through.me().await.id, guest_id, "autoJoin {auto_join}");
        through.get(&on_the_board).await.check_ok();
        let (workspaces, _own) = crate::own_space::workspaces_and_own(&through).await;
        assert_eq!(
            workspaces
                .iter()
                .map(|workspace| (workspace.id.as_str(), workspace.role))
                .collect::<Vec<_>>(),
            [(ROOT_WORKSPACE_ID, fvoci_model::WorkspaceRole::Guest)],
            "autoJoin {auto_join}: a guest still, of that workspace alone"
        );
        assert!(
            !ts.app
                .store()
                .is_organization_member(&guest_id)
                .await
                .expect("the membership is read"),
            "autoJoin {auto_join}: no member of the organization"
        );
        ts.teardown().await;
        fake.stop();
    }
}

/// Time entries obey the SSO wall: a password session is refused with 401 `sso_required`
/// when starting or logging time on a card or course unit of an SSO-required organization
/// (whether `workspaceId` is sent or omitted), and sees nothing of entries recorded there
/// across `active`, `entries`, `stop`, `patch`, and `delete`.
#[tokio::test]
async fn time_entries_obey_the_sso_wall() {
    let fake = FakeProvider::start().await;
    let (ts, named, admin) = walled(&fake).await;

    // Password session cannot start or log time on either a card or a unit behind the wall,
    // whether workspaceId is supplied or resolved from blockId.
    for body in [
        json!({"workspaceId": ROOT_WORKSPACE_ID, "blockId": named.card_id, "kind": "work"}),
        json!({"blockId": named.card_id, "kind": "work"}),
        json!({"workspaceId": ROOT_WORKSPACE_ID, "blockId": named.unit_id, "kind": "study"}),
        json!({"blockId": named.unit_id, "kind": "study"}),
    ] {
        check_sso_required(
            &ts.client.post("/users/me/time/start", &body).await,
            &named.org_id,
        );
    }
    for body in [
        json!({
            "workspaceId": ROOT_WORKSPACE_ID,
            "blockId": named.card_id,
            "kind": "work",
            "startedAt": 1_000,
            "endedAt": 2_000,
        }),
        json!({
            "blockId": named.unit_id,
            "kind": "lecture",
            "startedAt": 3_000,
            "endedAt": 4_000,
        }),
    ] {
        check_sso_required(
            &ts.client.post("/users/me/time/entries", &body).await,
            &named.org_id,
        );
    }

    // Provider session succeeds for both manual log and active timer.
    let manual: Value = admin
        .post(
            "/users/me/time/entries",
            &json!({
                "blockId": named.unit_id,
                "kind": "lecture",
                "startedAt": 1_000,
                "endedAt": 6_000,
                "note": "sso unit",
            }),
        )
        .await
        .created_json();
    let active_started: Value = admin
        .post(
            "/users/me/time/start",
            &json!({"blockId": named.card_id, "kind": "work", "note": "sso active"}),
        )
        .await
        .created_json();
    let manual_id = manual["id"].as_str().expect("manual id");
    let active_id = active_started["entry"]["id"].as_str().expect("active id");

    // Password session sees nothing of the walled organization's entries and cannot
    // stop, edit, or delete them.
    assert_sso_time_entries_hidden(&ts.client, manual_id, active_id).await;

    // Provider session sees both its entries intact (active one now stopped) and none of
    // the password-protected personal space entry.
    let list_sso: Value = admin.get("/users/me/time/entries").await.ok_json();
    assert_eq!(
        list_sso["summary"]["count"], 2,
        "provider session sees both org entries"
    );
    assert_eq!(
        list_sso["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .filter(|entry| entry["id"] == manual_id && entry["note"] == "sso unit")
            .count(),
        1,
        "provider session sees intact manual entry"
    );

    ts.teardown().await;
    fake.stop();
}

async fn assert_sso_time_entries_hidden(client: &Client, manual_id: &str, active_id: &str) {
    let active_pw: Value = client.get("/users/me/time/active").await.ok_json();
    assert_eq!(active_pw["entry"], Value::Null, "hidden active timer");
    let list_pw: Value = client.get("/users/me/time/entries").await.ok_json();
    assert_eq!(list_pw["entries"], json!([]), "hidden list");
    assert_eq!(
        list_pw["summary"]["totalMs"], 0,
        "hidden summary totalMs for password session"
    );
    assert_eq!(
        list_pw["summary"]["count"], 0,
        "hidden summary count for password session"
    );
    client
        .post_empty("/users/me/time/stop")
        .await
        .check_not_found();
    client
        .patch(
            &format!("/users/me/time/entries/{manual_id}"),
            &json!({"note": "tampered"}),
        )
        .await
        .check_not_found();
    client
        .delete(&format!("/users/me/time/entries/{manual_id}"))
        .await
        .check_not_found();
    client
        .delete(&format!("/users/me/time/entries/{active_id}"))
        .await
        .check_not_found();

    // Starting a timer in personal space with the password session stops the walled active
    // timer in the DB (keeping the 1-active invariant) without leaking it in `stopped`.
    let own_started: Value = client
        .post("/users/me/time/start", &json!({"kind": "study"}))
        .await
        .created_json();
    assert!(
        own_started.get("stopped").is_none(),
        "must not leak closed org timer in stopped: {own_started}"
    );
}
