//! Positive cases outside workspace paths, including the deliberately global admin role.

use serde_json::json;

use super::super::fake_oidc::FakeProvider;
use super::super::fake_smtp::FakeSmtp;
use super::{Case, Client, PASSWORD, TestServer, identity, shared};

/// The founder is told that somebody of their organization named them in a card, and
/// lists it, counts it, and marks it as read, by itself and with all.
async fn notifications(case: &mut Case, server: &TestServer) {
    let title = format!("{} told", case.label);
    let board = case
        .run(
            "createBoard",
            &[],
            json!({"title": title, "visibility": "private"}),
        )
        .await
        .value();
    let board = case.remember(&board, "id");
    let body = json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "mention", "attrs": {"userId": case.user}}]}]});
    let mut card = fvoci_model::Block {
        id: fvoci_model::new_id(),
        container_id: board,
        block_type: fvoci_model::BlockType::Card,
        title,
        body: fvoci_model::Body::parse(body).expect("a document"),
        ..fvoci_model::Block::default()
    };
    // Written by the other member, who never signs in: past the API, in the store.
    server
        .app
        .store()
        .for_org(&case.org)
        .expect("organization scope")
        .insert_block(&mut card, &case.peer)
        .await
        .expect("the card that names the founder is stored");
    case.identifiers.push(card.id.clone());
    fvoci_server::app::tend_notifications(&server.app).await;

    let told = case
        .present("getNotifications", &[], &card.id)
        .await
        .value();
    let told = told.as_array().expect("a list");
    assert_eq!(told.len(), 1, "the one mention: {told:?}");
    let notification = case.remember(&told[0], "id");
    let unread = case.run("getUnreadNotifications", &[], json!({})).await;
    assert_eq!(unread.value()["count"], 1, "{}", unread.text());
    case.run(
        "readNotification",
        &[("notificationId", &notification)],
        json!({}),
    )
    .await;
    case.run("readNotifications", &[], json!({})).await;
    // Sorted as an inbox: unread again, put away, back, and every one read put away.
    let one = &[("notificationId", notification.as_str())];
    case.run("unreadNotification", one, json!({})).await;
    case.run("archiveNotification", one, json!({})).await;
    case.run("unarchiveNotification", one, json!({})).await;
    case.run("archiveReadNotifications", &[], json!({})).await;
}

/// The founder starts, stops, edits, manually logs, lists, and deletes personal time
/// entries in their organization's workspace.
async fn time_entries(case: &mut Case) {
    case.run("getActiveTimeEntry", &[], json!({})).await;
    let started = case
        .run(
            "startTimeEntry",
            &[],
            json!({
                "workspaceId": case.workspace,
                "kind": "work",
                "note": format!("{} timer", case.label),
            }),
        )
        .await
        .value();
    let started_id = case.remember(&started["entry"], "id");
    case.run("stopTimeEntry", &[], json!({})).await;
    case.run(
        "patchTimeEntry",
        &[("entryId", &started_id)],
        json!({
            "startedAt": 1_000,
            "endedAt": 4_000,
            "note": format!("{} patched", case.label),
        }),
    )
    .await;
    let manual = case
        .run(
            "createTimeEntry",
            &[],
            json!({
                "workspaceId": case.workspace,
                "kind": "study",
                "startedAt": 5_000,
                "endedAt": 10_000,
                "note": format!("{} manual", case.label),
            }),
        )
        .await
        .value();
    let manual_id = case.remember(&manual, "id");
    case.present("listTimeEntries", &[], &manual_id).await;
    case.run("deleteTimeEntry", &[("entryId", &manual_id)], json!({}))
        .await;
}

async fn exam_publication(case: &mut Case, server: &TestServer) {
    let course = case
        .run(
            "createCourse",
            &[],
            json!({"title": format!("{} pub course", case.label), "visibility": "private"}),
        )
        .await
        .value();
    let course_id = case.remember(&course, "id");
    let at = &[("courseId", course_id.as_str())];
    let exam = case
        .run(
            "createCourseExam",
            at,
            json!({"title": format!("{} pub exam", case.label), "maxScore": 100.0}),
        )
        .await
        .value();
    let exam_id = case.remember(&exam, "id");
    let of_exam = &[
        ("courseId", course_id.as_str()),
        ("examId", exam_id.as_str()),
    ];
    case.run("putMyExamScore", of_exam, json!({"score": 80.0}))
        .await;

    let store = server
        .app
        .store()
        .for_org(&case.org)
        .expect("organization scope");
    let score_input = fvoci_model::MyExamScoreUpdate {
        score: 90.0,
        note: String::new(),
    }
    .check_request()
    .expect("valid score");
    for idx in 0..4 {
        let mut learner = fvoci_model::User {
            id: fvoci_model::new_id(),
            username: format!("{}-learner-{idx}", case.label),
            email: format!("{}-learner-{idx}@example.test", case.label),
            ..fvoci_model::User::default()
        };
        store.create_user(&mut learner).await.expect("learner user");
        store
            .add_member(&fvoci_model::ContainerMember {
                container_id: course_id.clone(),
                user_id: learner.id.clone(),
                role: fvoci_model::ContainerRole::Viewer,
                synthetic: false,
                inherited_role: None,
            })
            .await
            .expect("learner course member");
        store
            .upsert_my_exam_score(&course_id, &exam_id, &learner.id, &score_input)
            .await
            .expect("learner score");
        case.identifiers.push(learner.id);
    }

    case.run("publishCourseExamResults", of_exam, json!({}))
        .await;
    case.run("unpublishCourseExamResults", of_exam, json!({}))
        .await;
    case.run("deleteCourseExam", of_exam, json!({})).await;
    case.run(
        "deleteContainer",
        &[("boardId", course_id.as_str())],
        json!({}),
    )
    .await;
}

pub(super) async fn exercise(
    case: &mut Case,
    server: &TestServer,
    fake: &FakeProvider,
    smtp: &FakeSmtp,
) {
    for operation in ["ping", "ready", "getClientConfig"] {
        case.run(operation, &[], json!({})).await;
    }
    account(case).await;
    organization(case).await;
    tokens(case, server).await;
    notifications(case, server).await;
    exam_publication(case, server).await;
    time_entries(case).await;
    shared::exercise(case).await;
    shared::exercise_wiki(case).await;
    shared::exercise_course(case).await;
    invitation(case, server, smtp).await;
    recovery(case, server, smtp).await;
    identity::exercise(case, server, fake).await;
    // A founder made one organization, and makes another as any account does.
    let another = case
        .run("createOrganization", &[], json!({"name": "Another"}))
        .await
        .value();
    assert_eq!(
        another["role"], "admin",
        "who starts an organization: {another}"
    );
    for field in ["id", "orgId"] {
        case.remember(&another, field);
    }
    // And registered with a password: there is no first one to set.
    let request = case
        .request("setPassword", &[])
        .json(&json!({"password": "a first parity password"}));
    let response = Client::send(request).await;
    assert_eq!(response.status.as_u16(), 409, "{}", response.text());
    case.record("setPassword", &response);
    let request = case
        .request("setUserAdmin", &[("userId", &case.user)])
        .json(&json!({"isAdmin": true}));
    let response = Client::send(request).await;
    assert_eq!(
        response.status.as_u16(),
        if case.instance_admin { 200 } else { 403 },
        "organization ownership does not grant instance authority"
    );
    if case.instance_admin {
        assert_eq!(
            response.value()["id"],
            case.user,
            "admin response names the requested account"
        );
        assert_eq!(
            response.value()["isAdmin"],
            true,
            "instance administrator remains an administrator"
        );
    }
    assert_eq!(
        case.client.me().await.is_admin,
        case.instance_admin,
        "instance authority stays as assigned"
    );
    case.record("setUserAdmin", &response);
    let back = address(case, server, smtp).await;
    // A session made with the password gives it once more, and goes on under a new
    // cookie as the session it was.
    let confirmed = case
        .run("confirmPassword", &[], json!({"password": PASSWORD}))
        .await;
    case.client.token = confirmed.session_cookie().expect("rotated session cookie");
    let changed = case
        .run(
            "changePassword",
            &[],
            json!({"oldPassword": PASSWORD, "newPassword": "changed parity password"}),
        )
        .await;
    case.client.token = changed.session_cookie().expect("rotated session cookie");
    deletion(case, server).await;
    leaving(case, server).await;
    // The session of an account that left an organization stays.
    case.run("logout", &[], json!({})).await;
    // The link mailed to the address before stood through all of that, and gives it
    // back: last, as it ends every session of the account and locks its password.
    let reverted = server
        .anonymous()
        .post("/email-change/revert", &json!({"token": back.token}))
        .await;
    assert_eq!(
        reverted.value()["email"],
        back.email,
        "the address before is the account's again: {}",
        reverted.text()
    );
    case.record("revertEmailChange", &reverted);
}

/// The link that gives an account an address back, and that address.
struct Back {
    token: String,
    email: String,
}

/// The founder leaves the organization once somebody else administers it, and the
/// instance: a founder is the only administrator of theirs, who does not leave.
async fn leaving(case: &mut Case, server: &TestServer) {
    let store = server
        .app
        .store()
        .for_org(&case.org)
        .expect("organization scope");
    let peer = case.peer.clone();
    for statement in [
        format!("UPDATE organization_members SET role = 'admin' WHERE user_id = '{peer}'"),
        format!("UPDATE users SET is_admin = true WHERE id = '{peer}'"),
    ] {
        store
            .execute_unchecked(&statement)
            .await
            .expect("the peer stands in");
    }
    case.run("leaveOrganization", &[], json!({})).await;
}

/// The founder asks to delete the account, signs in to it while it waits, and takes
/// the asking back. Meanwhile somebody else administers the organization, and the
/// instance: a founder is the only administrator of theirs, who does not go.
async fn deletion(case: &mut Case, server: &TestServer) {
    const CHANGED: &str = "changed parity password";
    let store = server
        .app
        .store()
        .for_org(&case.org)
        .expect("organization scope");
    let peer = case.peer.clone();
    let stand_in = async |role: &str, administers: bool| {
        for statement in [
            format!("UPDATE organization_members SET role = '{role}' WHERE user_id = '{peer}'"),
            format!("UPDATE users SET is_admin = {administers} WHERE id = '{peer}'"),
        ] {
            store
                .execute_unchecked(&statement)
                .await
                .expect("the peer stands in");
        }
    };
    stand_in("admin", true).await;
    let standing = case.run("getAccountDeletion", &[], json!({})).await.value();
    assert_eq!(
        standing["soleAdminOf"],
        json!([]),
        "somebody else administers the organization: {standing}"
    );
    let name = case.label.clone();
    case.run(
        "requestAccountDeletion",
        &[],
        json!({"username": name, "password": CHANGED}),
    )
    .await;
    let back = server
        .anonymous()
        .post("/login", &json!({"username": name, "password": CHANGED}))
        .await;
    assert_eq!(back.status.as_u16(), 204, "an account that waits signs in");
    case.client.token = back.session_cookie().expect("login cookie");
    case.run("cancelAccountDeletion", &[], json!({})).await;
    stand_in("member", false).await;
}

async fn account(case: &mut Case) {
    case.run(
        "patchMe",
        &[],
        json!({"customName": format!("{} display", case.label)}),
    )
    .await;
    let user = case.user.clone();
    case.present("getMe", &[], &user).await;
    let workspace = case.workspace.clone();
    case.present("getMyWorkspaces", &[], &workspace).await;
    let org = case.org.clone();
    case.present("getMyOrganizations", &[], &org).await;
    let memberships = case.run("getMyMemberships", &[], json!({})).await.value();
    assert!(
        !memberships.as_array().expect("memberships").is_empty(),
        "founder has board memberships"
    );
    case.run(
        "patchClientState",
        &[],
        json!({"updatedFields": {"parity": case.label}}),
    )
    .await;
    let label = case.label.clone();
    case.present("getClientState", &[], &label).await;
    case.run("patchUserSettings", &[], json!({"language": "ko"}))
        .await;
    let settings = case.run("getUserSettings", &[], json!({})).await.value();
    assert_eq!(settings["language"], "ko", "account setting round trip");
    let options = super::with_query(case.request("getLoginOptions", &[]), "org", &case.org);
    let options = case.send("getLoginOptions", options).await.value();
    assert_eq!(
        options["organization"]["id"], case.org,
        "login options name the requested organization"
    );
}

/// The founder gives an agent a token of the workspace, finds it as its owner and as the
/// organization's administrator, lets the agent speak MCP with it, and takes one back
/// each way.
async fn tokens(case: &mut Case, server: &TestServer) {
    let made = json!({"name": "agent", "scope": "read", "workspaceIds": [case.workspace]});
    let first = case.run("createApiToken", &[], made.clone()).await.value();
    let first = case.remember(&first["apiToken"], "id");
    case.present("getApiTokens", &[], &first).await;
    case.present("getOrgApiTokens", &[], &first).await;
    case.run("revokeOrgApiToken", &[("tokenId", &first)], json!({}))
        .await;
    let issued = case.run("createApiToken", &[], made).await.value();
    let second = case.remember(&issued["apiToken"], "id");
    let agent = server.client_with_token(issued["token"].as_str().expect("the token"));
    mcp(case, &agent).await;
    case.run("revokeApiToken", &[("tokenId", &second)], json!({}))
        .await;
}

/// The agent of a founder's token is told of the tools it may call, and is given the
/// overview of the founder's workspace and nothing of the other organization's.
async fn mcp(case: &mut Case, agent: &Client) {
    let message = |id: u32, method: &str, params: serde_json::Value| json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    let listed =
        agent
            .request(reqwest::Method::POST, "/mcp")
            .json(&message(1, "tools/list", json!({})));
    let listed = case.send("postMcp", listed).await.value();
    assert!(
        listed["result"]["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty()),
        "the tools are listed: {listed}"
    );
    let overview = agent.request(reqwest::Method::POST, "/mcp").json(&message(
        2,
        "tools/call",
        json!({"name": "get_workspace_overview",
               "arguments": {"workspaceId": case.workspace}}),
    ));
    let overview = Client::send(overview).await;
    case.assert_is_ours("postMcp", &overview.body);
    let overview = overview.value();
    assert_eq!(
        overview["result"]["isError"], false,
        "the overview is given: {overview}"
    );
    assert!(
        overview["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains(&case.workspace)),
        "the overview is of the founder's workspace: {overview}"
    );
}

async fn organization(case: &mut Case) {
    let org = case.org.clone();
    case.present("getOrganization", &[], &org).await;
    case.run(
        "patchOrganization",
        &[],
        json!({"name": format!("{} renamed", case.label)}),
    )
    .await;
    let peer = case.peer.clone();
    case.present("getOrgMembers", &[], &peer).await;
    for role in ["admin", "member"] {
        case.run("putOrgRole", &[("userId", &peer)], json!({"role": role}))
            .await;
    }
    let made = case
        .run("createOrgWorkspace", &[], json!({"name": "Additional"}))
        .await
        .value();
    let workspace = case.remember(&made, "id");
    let at = &[("workspaceId", workspace.as_str())];
    let joined = case.run("joinOrgWorkspace", at, json!({})).await.value();
    assert_eq!(
        joined["joinedAsOrgAdmin"], true,
        "organization administrator joins the new workspace"
    );
    let left = case.run("leaveOrgWorkspace", at, json!({})).await.value();
    assert_eq!(
        left["joinedAsOrgAdmin"], false,
        "leaving ends administrative participation"
    );
    case.run(
        "renameOrgWorkspace",
        at,
        json!({"name": "Additional renamed"}),
    )
    .await;
    case.run("deleteOrgWorkspace", at, json!({})).await;
    let restored = case.run("restoreOrgWorkspace", at, json!({})).await.value();
    assert_eq!(restored.get("deletedAt"), None, "workspace is restored");
    case.present("getOrgWorkspaces", &[], &workspace).await;
    case.present("getOrgAudit", &[], &workspace).await;
}

async fn invitation(case: &mut Case, server: &TestServer, smtp: &FakeSmtp) {
    let invitation = case
        .run("createInvitation", &[], json!({"role": "member"}))
        .await
        .value();
    let token = invitation["token"].as_str().expect("invitation token");
    let mailed = format!("{}-mailed@example.test", case.label);
    case.run(
        "emailInvitation",
        &[],
        json!({"token": token, "recipients": [mailed]}),
    )
    .await
    .check_no_content();
    let letter = mail_to(case, smtp, &mailed, "/invite/").await;
    assert!(
        letter.contains(token),
        "the mail carries this organization's invitation"
    );
    let preview = case
        .run("previewInvitation", &[], json!({"token": token}))
        .await
        .value();
    assert!(
        preview.to_string().contains(&case.label),
        "preview identifies this invitation"
    );
    let accepted = server
        .anonymous()
        .post(
            "/invitations/accept",
            &json!({
                "token": token, "account": {"username": format!("{}-invited", case.label),
                "email": format!("{}-invited@example.test", case.label), "password": PASSWORD}
            }),
        )
        .await;
    // A link for members proves no address: the new account waits for its own.
    assert_eq!(accepted.status.as_u16(), 202, "{}", accepted.text());
    case.record("acceptInvitation", &accepted);
    let invited = format!("{}-invited@example.test", case.label);
    let resent = server
        .anonymous()
        .post("/email-verification/request", &json!({"email": invited}))
        .await;
    resent.check_no_content();
    case.record("requestEmailVerification", &resent);
    let confirmed = confirm(server, smtp, &invited, &case.forbidden, 2).await;
    assert_eq!(
        confirmed.value()["workspaceId"],
        case.workspace,
        "invitation joins its own workspace"
    );
    case.record("completeEmailVerification", &confirmed);
    let invited_username = format!("{}-invited", case.label);
    let members = case
        .client
        .get(&format!("/org/{}/members", case.org))
        .await
        .value();
    let invited_id = members
        .as_array()
        .expect("org members")
        .iter()
        .find(|m| m["username"] == invited_username)
        .and_then(|m| m["userId"].as_str())
        .expect("invited member in org")
        .to_owned();
    case.run("removeOrgMember", &[("userId", &invited_id)], json!({}))
        .await;
}

/// Confirms the registration of `recipient` by the newest of the `count` confirmation
/// mails it is to have, which must name nothing of `forbidden`.
pub(super) async fn confirm(
    server: &TestServer,
    smtp: &FakeSmtp,
    recipient: &str,
    forbidden: &[String],
    count: usize,
) -> super::ApiResponse {
    let letter = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let mut letters: Vec<String> = smtp
                .messages()
                .await
                .into_iter()
                .filter(|message| {
                    message.recipient == recipient && message.decoded.contains("/verify-email#")
                })
                .map(|message| message.decoded)
                .collect();
            if letters.len() >= count {
                return letters.pop().expect("the newest");
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the confirmation mail arrives");
    for value in forbidden {
        assert!(
            !letter.contains(value.as_str()),
            "mail to {recipient} leaked first-organization data {value}"
        );
    }
    let link = letter
        .lines()
        .find(|line| line.starts_with("http") && line.contains("/verify-email#"))
        .expect("the confirmation mail has its link");
    let token = link.split_once('#').expect("a fragment").1.trim();
    // Not the browser that registered: the link alone confirms nothing, in either
    // organization, and a wrong password is a failed login.
    let asked = server
        .anonymous()
        .post("/email-verification/complete", &json!({"token": token}))
        .await;
    assert_eq!(asked.status.as_u16(), 403, "{}", asked.text());
    assert_eq!(
        asked.error_reason(),
        "password_required",
        "the link alone confirms nothing"
    );
    let wrong = server
        .anonymous()
        .post(
            "/email-verification/complete",
            &json!({"token": token, "password": "not the password"}),
        )
        .await;
    assert_eq!(wrong.status.as_u16(), 401, "{}", wrong.text());
    let confirmed = server
        .anonymous()
        .post(
            "/email-verification/complete",
            &json!({"token": token, "password": PASSWORD}),
        )
        .await;
    assert_eq!(confirmed.status.as_u16(), 200, "{}", confirmed.text());
    confirmed
}

/// The invited account recovers its password from the mail its own organization sent.
async fn recovery(case: &mut Case, server: &TestServer, smtp: &FakeSmtp) {
    let invited = format!("{}-invited", case.label);
    let email = format!("{invited}@example.test");
    let requested = server
        .anonymous()
        .post("/password-reset/request", &json!({"email": email}))
        .await;
    requested.check_no_content();
    case.record("requestPasswordReset", &requested);
    let letter = mail_to(case, smtp, &email, "/reset-password#").await;
    let link = letter
        .lines()
        .find(|line| line.starts_with("http") && line.contains("/reset-password#"))
        .expect("the reset mail has its link");
    let token = link.split_once('#').expect("a fragment").1.trim();
    let completed = server
        .anonymous()
        .post(
            "/password-reset/complete",
            &json!({"token": token, "password": "recovered parity password"}),
        )
        .await;
    completed.check_no_content();
    case.record("completePasswordReset", &completed);
    let again = server
        .anonymous()
        .post(
            "/login",
            &json!({"username": invited, "password": "recovered parity password"}),
        )
        .await;
    assert_eq!(again.status.as_u16(), 204, "the new password signs in");
}

/// The founder moves the account to another address: asks under the password, has the
/// link mailed again and takes the asking back; asks for another, and confirms it by
/// the link mailed there, in a browser that is not signed in. Returns the link mailed
/// to the address before, which gives it back.
async fn address(case: &mut Case, server: &TestServer, smtp: &FakeSmtp) -> Back {
    let me: fvoci_model::Me = case.client.get("/users/me").await.ok_json();
    let before = me.email;
    let ask = |email: String| json!({"email": email, "password": PASSWORD});
    case.run(
        "requestEmailChange",
        &[],
        ask(format!("{}-first@example.test", case.label)),
    )
    .await;
    case.run("resendEmailChange", &[], json!({})).await;
    case.run("cancelEmailChange", &[], json!({})).await;
    let moved = format!("{}-moved@example.test", case.label);
    case.run("requestEmailChange", &[], ask(moved.clone()))
        .await;
    let letter = mail_to(case, smtp, &moved, "/confirm-email#").await;
    let link = letter
        .lines()
        .find(|line| line.starts_with("http") && line.contains("/confirm-email#"))
        .expect("the mail has its link");
    let token = link.split_once('#').expect("a fragment").1.trim();
    let confirmed = server
        .anonymous()
        .post("/email-change/confirm", &json!({"token": token}))
        .await;
    confirmed.check_no_content();
    case.record("confirmEmailChange", &confirmed);
    let me = case.run("getMe", &[], json!({})).await.value();
    assert_eq!(me["email"], moved, "the address is the confirmed one: {me}");
    let notice = mail_to(case, smtp, &before, "/revert-email#").await;
    let link = notice
        .lines()
        .find(|line| line.starts_with("http") && line.contains("/revert-email#"))
        .expect("the notice has the link back");
    Back {
        token: link
            .split_once('#')
            .expect("a fragment")
            .1
            .trim()
            .to_owned(),
        email: before,
    }
}

/// The decoded mail to `recipient` that holds `containing`, which must name nothing of
/// the other organization.
async fn mail_to(case: &Case, smtp: &FakeSmtp, recipient: &str, containing: &str) -> String {
    let letter = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            if let Some(found) = smtp.messages().await.into_iter().find(|message| {
                message.recipient == recipient && message.decoded.contains(containing)
            }) {
                return found.decoded;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the mail arrives");
    for value in &case.forbidden {
        assert!(
            !letter.contains(value.as_str()),
            "mail to {recipient} leaked first-organization data {value}"
        );
    }
    letter
}
