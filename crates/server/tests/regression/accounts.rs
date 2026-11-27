//! Scenarios: system routes, registration and sessions, workspaces and users.
#![expect(
    clippy::too_many_lines,
    reason = "a scenario is one request sequence; cutting it up would hide the order it pins"
)]

use serde_json::{Value, json};

use crate::common::{ROOT_WORKSPACE_ID, UNKNOWN_ID};
use crate::harness::ROOT_API;
use crate::harness::{API, Session};

pub(crate) async fn system(s: &mut Session) {
    // Root `/hello` and `/ping` fall through to the web app, while retired `/api/v1/hello`
    // answers a JSON 404 under the API prefix.
    s.get("hello", "/hello").without_csrf_header().await;
    s.get("ping: at the root", "/ping")
        .without_csrf_header()
        .await;
    s.get("hello: under the prefix", &format!("{API}/hello"))
        .without_csrf_header()
        .await;
    let ping = s
        .get("ping", &format!("{API}/ping"))
        .without_csrf_header()
        .unrecorded()
        .await;
    let ping = ping.json();
    let mut members: Vec<&String> = ping
        .as_object()
        .expect("ping answers an object")
        .keys()
        .collect();
    members.sort();
    s.note("ping members", &json!(members));
    let db_type = ping["dbType"].as_str().unwrap_or_default();
    assert!(
        ["sqlite", "postgres"].contains(&db_type),
        "ping names the database kind, got {db_type:?}"
    );
    // The database kind differs between the two test runs, and the version between
    // releases: the snapshot keeps neither.
    assert_eq!(
        ping["version"],
        json!(env!("CARGO_PKG_VERSION")),
        "ping gives the server's version"
    );
    s.note("ping version", &json!(["<version>", "<db>"]));
    s.get("client config", &format!("{API}/client-config"))
        .await;
    s.get(
        "client config without csrf header",
        &format!("{API}/client-config"),
    )
    .without_csrf_header()
    .await;
    s.get("unknown api path", &format!("{API}/no-such-route"))
        .await;
    s.delete(
        "wrong method on a known path",
        &format!("{API}/client-config"),
    )
    .await;
    // Retired API paths return 404 (with or without a session) rather than serving the SPA shell.
    s.get("old path: client config", &format!("{API}/clientConfig"))
        .await;
    s.get("old prefix: client config", "/api/v2/clientConfig")
        .await;
    s.post("old prefix: login", "/api/v2/login")
        .json(json!({"username": "nobody", "password": "nothing"}))
        .await;
    s.get("old prefix: the prefix itself", "/api/v2").await;
    s.get("the api root", "/api").await;
    s.get("index for an app route", "/board/abc")
        .without_csrf_header()
        .keep_headers(&["Content-Type"])
        .await;
    s.get("static without slash", "/static")
        .without_csrf_header()
        .await;
    s.get("missing static file", "/static/nope.js")
        .without_csrf_header()
        .await;
}

/// The part of [`auth`] about who gets an account: nobody by registering once the
/// server has its organization, and whoever accepts a link with an account to make.
async fn registrations_and_acceptances(s: &mut Session, alice: &str) {
    let link = s.member_link("root").await;
    let register = format!("{API}/register");
    let accept = format!("{API}/invitations/accept");

    // The server has its organization: nobody registers after its first user, whatever they send.
    s.post("register: the server has its organization", &register)
        .json(json!({
            "username": "auth-x", "email": "auth-x@example.com", "password": "test-password",
            "organizationName": "Another Firm",
        }))
        .await;
    s.post(
        "register: the server has its organization, nothing sent",
        &register,
    )
    .json(json!({}))
    .await;
    s.post("accept: no such link", &accept)
        .json(json!({
            "token": "nope",
            "account": {
                "username": "auth-x", "email": "auth-x@example.com", "password": "test-password",
            },
        }))
        .await;
    s.post("accept: neither a session nor an account", &accept)
        .json(json!({"token": link}))
        .await;
    let refused: [(&str, Value); 8] = [
        (
            "no username",
            json!({"username": "", "email": "a@example.com", "password": "test-password"}),
        ),
        (
            "no email",
            json!({"username": "auth-y", "email": "", "password": "test-password"}),
        ),
        (
            "bad email",
            json!({"username": "auth-y", "email": "not-an-email", "password": "test-password"}),
        ),
        (
            "no password",
            json!({"username": "auth-y", "email": "auth-y@example.com", "password": ""}),
        ),
        (
            "short password",
            json!({"username": "auth-y", "email": "auth-y@example.com", "password": "1234567"}),
        ),
        (
            "long password",
            json!({"username": "auth-y", "email": "auth-y@example.com", "password": "x".repeat(1025)}),
        ),
        (
            "taken username",
            json!({"username": alice, "email": "other@example.com", "password": "test-password"}),
        ),
        (
            "taken email",
            json!({"username": "auth-z", "email": format!("{alice}@example.com"), "password": "test-password"}),
        ),
    ];
    for (name, account) in refused {
        s.post(&format!("accept: {name}"), &accept)
            .json(json!({"token": link, "account": account}))
            .await;
    }

    // An acceptance answers where the account was let in and signs nobody in.
    s.post("accept", &accept)
        .json(json!({
            "token": link,
            "account": {
                "username": "auth-new", "email": "auth-new@example.com",
                "password": "test-password",
            },
        }))
        .keep_headers(&["Set-Cookie"])
        .await;
    s.post("accept: a session and an account", &accept)
        .json(json!({
            "token": link,
            "account": {
                "username": "auth-other", "email": "auth-other@example.com",
                "password": "test-password",
            },
        }))
        .user(alice)
        .await;
    s.post("accept: a member already", &accept)
        .json(json!({"token": link}))
        .user(alice)
        .await;
}

pub(crate) async fn auth(s: &mut Session) {
    let [alice] = s.users("auth", ["alice"]).await;
    let login = format!("{API}/login");
    registrations_and_acceptances(s, &alice).await;

    s.post("login: the registered user", &login)
        .json(json!({"username": "auth-new", "password": "test-password"}))
        .await;

    s.post("login", &login)
        .json(json!({"username": alice, "password": "test-password"}))
        .await;
    s.post("login by email", &login)
        .json(json!({"email": format!("{alice}@example.com"), "password": "test-password"}))
        .await;
    s.post("login: wrong password", &login)
        .json(json!({"username": alice, "password": "nope"}))
        .await;
    s.post("login: unknown user", &login)
        .json(json!({"username": "nobody", "password": "x"}))
        .await;
    s.post("login: wrong type", &login)
        .json(json!({"type": "sso", "username": alice, "password": "test-password"}))
        .await;
    s.post("login: empty body", &login).json(json!({})).await;

    s.get("me", &format!("{API}/users/me")).user(&alice).await;
    s.get("me without session", &format!("{API}/users/me"))
        .await;
    s.get("me with a bad token", &format!("{API}/users/me"))
        .token("not-a-token")
        .await;
    s.get("my memberships", &format!("{API}/users/me/memberships"))
        .user(&alice)
        .await;
    let client_state = format!("{API}/users/me/client-state");
    s.get("my config", &client_state).user(&alice).await;
    let alice_id = s.user_id(&alice).await;
    s.get("user by id", &format!("{ROOT_API}/users/{alice_id}"))
        .user(&alice)
        .await;
    s.get("unknown user", &format!("{ROOT_API}/users/unknown-user"))
        .user(&alice)
        .await;
    s.post("users by id", &format!("{ROOT_API}/users"))
        .json(json!([alice_id]))
        .user(&alice)
        .await;
    s.post("users by id: empty list", &format!("{ROOT_API}/users"))
        .json(json!([]))
        .user(&alice)
        .await;
    s.patch("update my config", &client_state)
        .json(
            json!({"updatedFields": {"welcomePageViewed": "1", "theme": "dark"}, "deletedFields": []}),
        )
        .user(&alice)
        .await;
    s.patch("delete a config entry", &client_state)
        .json(json!({"updatedFields": {}, "deletedFields": ["theme"]}))
        .user(&alice)
        .await;
    s.get("my config afterwards", &client_state)
        .user(&alice)
        .await;
    // The bag of client state is reached through the session only. The paths that
    // named a user, one's own or another, are gone.
    let root_id = s.user_id("root").await;
    s.put(
        "update another user's config",
        &format!("{API}/users/{root_id}/config"),
    )
    .json(json!({"updatedFields": {"x": "1"}, "deletedFields": []}))
    .user(&alice)
    .await;
    s.put(
        "old path: update my config",
        &format!("{API}/users/{alice_id}/config"),
    )
    .json(json!({"updatedFields": {"x": "1"}, "deletedFields": []}))
    .user(&alice)
    .await;
    s.get("old path: my config", &format!("{API}/users/me/config"))
        .user(&alice)
        .await;
    s.get("my config after the old paths", &client_state)
        .user(&alice)
        .await;

    s.post(
        "old path: change password",
        &format!("{API}/users/{alice_id}/changepassword"),
    )
    .json(json!({"oldPassword": "test-password", "newPassword": "new-password"}))
    .user(&alice)
    .await;
    let change_password = format!("{API}/users/me/password");
    // A second session of the same user, as on another device, opened before the change.
    let other_device = s
        .post("login on another device", &login)
        .json(json!({"username": alice, "password": "test-password"}))
        .await
        .session_token();
    s.post("change password: wrong old password", &change_password)
        .json(json!({"oldPassword": "nope", "newPassword": "new-password"}))
        .user(&alice)
        .await;
    s.post("change password: short new password", &change_password)
        .json(json!({"oldPassword": "test-password", "newPassword": "short"}))
        .user(&alice)
        .await;
    let before_the_change = s.session_token(&alice);
    let new_token = s
        .post("change password", &change_password)
        .json(json!({"oldPassword": "test-password", "newPassword": "new-password"}))
        .user(&alice)
        .keep_headers(&["Set-Cookie"])
        .await
        .session_token();
    // The session the password was changed through goes on under a new token.
    s.set_session_token(&alice, new_token);
    s.get(
        "me with the token from before the change",
        &format!("{API}/users/me"),
    )
    .token(&before_the_change)
    .await;
    s.get(
        "me on the other device after the change",
        &format!("{API}/users/me"),
    )
    .token(&other_device)
    .await;
    s.get(
        "me with the session that changed the password",
        &format!("{API}/users/me"),
    )
    .user(&alice)
    .await;
    s.post("login with the new password", &login)
        .json(json!({"username": alice, "password": "new-password"}))
        .await;
    s.post("login with the old password", &login)
        .json(json!({"username": alice, "password": "test-password"}))
        .await;

    s.post("logout", &format!("{API}/logout"))
        .user(&alice)
        .await;
    s.get("me after logout", &format!("{API}/users/me"))
        .user(&alice)
        .await;
}

pub(crate) async fn workspaces(s: &mut Session) {
    let [alice] = s.users("workspaces", ["alice"]).await;
    s.get("workspaces", &format!("{API}/users/me/workspaces"))
        .user(&alice)
        .await;
    s.get("workspace 0", ROOT_API).user(&alice).await;
    s.get("some other workspace id", &format!("{API}/w/anything"))
        .user(&alice)
        .await;
    s.get("workspace without session", ROOT_API).await;
    let users = s
        .get("workspace users", &format!("{ROOT_API}/users"))
        .user(&alice)
        .unrecorded()
        .await;
    let includes_me = users
        .json()
        .as_array()
        .expect("the workspace users are a list")
        .iter()
        .any(|user| user["username"] == alice.as_str());
    s.note("workspace users include me", &json!(includes_me));
    s.get(
        "workspace users search",
        &format!("{ROOT_API}/users?search=workspaces-ali"),
    )
    .user(&alice)
    .await;
    s.get(
        "workspace users search without match",
        &format!("{ROOT_API}/users?search=zzzz-no-such"),
    )
    .user(&alice)
    .await;
    let alice_id = s.user_id(&alice).await;
    s.post("workspace users by id", &format!("{ROOT_API}/users"))
        .json(json!([alice_id]))
        .user(&alice)
        .await;
    s.post("workspace users by id: empty", &format!("{ROOT_API}/users"))
        .json(json!([]))
        .user(&alice)
        .await;
}

/// Administrators of the instance. The harness's first user, `root`, is the one the
/// instance starts with.
pub(crate) async fn admins(s: &mut Session) {
    let [alice, bob] = s.users("admins", ["alice", "bob"]).await;
    let alice_id = s.user_id(&alice).await;
    let bob_id = s.user_id(&bob).await;
    let root_id = s.user_id("root").await;
    let workspace = format!("{API}/w/{ROOT_WORKSPACE_ID}");
    let invitations = format!("{workspace}/invitations");
    let admin_of = |user_id: &str| format!("{API}/users/{user_id}/admin");

    s.get("me: the first user", &format!("{API}/users/me"))
        .user("root")
        .await;
    s.get("me: a later user", &format!("{API}/users/me"))
        .user(&alice)
        .await;
    s.get(
        "the first user, seen by another",
        &format!("{ROOT_API}/users/{root_id}"),
    )
    .user(&alice)
    .await;

    // -- a workspace's administrators invite its members -----------------------------
    s.get("workspace: administrator", &workspace)
        .user("root")
        .await;
    s.get("workspace: anybody else", &workspace)
        .user(&alice)
        .await;
    s.get(
        "workspaces: administrator",
        &format!("{API}/users/me/workspaces"),
    )
    .user("root")
    .await;
    s.get(
        "workspaces: anybody else",
        &format!("{API}/users/me/workspaces"),
    )
    .user(&alice)
    .await;
    s.post("invite: not an administrator", &invitations)
        .json(json!({"role": "member"}))
        .user(&alice)
        .await;
    s.post("invite: no session", &invitations)
        .json(json!({"role": "member"}))
        .await;
    s.post(
        "invite: unknown workspace",
        &format!("{API}/w/{UNKNOWN_ID}/invitations"),
    )
    .json(json!({"role": "member"}))
    .user("root")
    .await;
    s.post(
        "old path: renew the token",
        &format!("{workspace}/signup-token"),
    )
    .user("root")
    .await;
    s.post("invite", &invitations)
        .json(json!({"role": "member"}))
        .user("root")
        .await;
    s.post("invite: administrators", &invitations)
        .json(json!({"role": "admin"}))
        .user("root")
        .await;
    s.post("invite: members to a board", &invitations)
        .json(json!({"role": "member", "containerId": UNKNOWN_ID}))
        .user("root")
        .await;
    s.post("register without an invitation", &format!("{API}/register"))
        .json(json!({
            "username": "admins-late", "email": "admins-late@example.com",
            "password": "test-password", "organizationName": "Late",
        }))
        .await;

    // -- making and unmaking administrators ------------------------------------------
    s.put("make: not an administrator", &admin_of(&bob_id))
        .json(json!({"isAdmin": true}))
        .user(&alice)
        .await;
    s.put("make: oneself, not an administrator", &admin_of(&alice_id))
        .json(json!({"isAdmin": true}))
        .user(&alice)
        .await;
    s.put("make: no session", &admin_of(&bob_id))
        .json(json!({"isAdmin": true}))
        .await;
    s.put("make: no body member", &admin_of(&bob_id))
        .json(json!({}))
        .user("root")
        .await;
    s.put("make: not a boolean", &admin_of(&bob_id))
        .json(json!({"isAdmin": "yes"}))
        .user("root")
        .await;
    s.put("make: unknown user", &admin_of(UNKNOWN_ID))
        .json(json!({"isAdmin": true}))
        .user("root")
        .await;
    s.put("make: no identifier", &admin_of("nobody"))
        .json(json!({"isAdmin": true}))
        .user("root")
        .await;
    s.put("step down: the only administrator", &admin_of(&root_id))
        .json(json!({"isAdmin": false}))
        .user("root")
        .await;
    s.put("make", &admin_of(&bob_id))
        .json(json!({"isAdmin": true}))
        .user("root")
        .await;
    s.put("make: again", &admin_of(&bob_id))
        .json(json!({"isAdmin": true}))
        .user("root")
        .await;
    s.get("workspace: the new administrator", &workspace)
        .user(&bob)
        .await;
    s.put("step down", &admin_of(&root_id))
        .json(json!({"isAdmin": false}))
        .user("root")
        .await;
    s.put("make: after stepping down", &admin_of(&root_id))
        .json(json!({"isAdmin": true}))
        .user("root")
        .await;
    s.get("workspace: after stepping down", &workspace)
        .user("root")
        .await;
    s.put("unmake: the new only administrator", &admin_of(&bob_id))
        .json(json!({"isAdmin": false}))
        .user(&bob)
        .await;
    // An administrator of the instance administers no workspace by that: the members
    // of a workspace are its administrators' to invite.
    s.post(
        "invite: an administrator of the instance alone",
        &invitations,
    )
    .json(json!({"role": "member"}))
    .user(&bob)
    .await;
}

/// What a user has set about how things are shown to them, and the bag of client state
/// next to it.
pub(crate) async fn settings(s: &mut Session) {
    let [alice, bob] = s.users("settings", ["alice", "bob"]).await;
    let path = format!("{API}/users/me/settings");
    let bag = format!("{API}/users/me/client-state");

    s.get("get: defaults", &path).user(&alice).await;
    s.get("get: no session", &path).await;
    s.patch("patch: every setting", &path)
        .json(json!({
            "timeZone": "Asia/Seoul", "weekStart": "monday", "hourCycle": "h23",
            "theme": "dark", "cardOpen": "side", "density": "compact", "haptics": false,
            "language": "ko",
            "notifyMentions": false, "notifyAssignments": false, "notifyComments": false,
        }))
        .user(&alice)
        .await;
    s.get("get: as set", &path).user(&alice).await;
    s.get("get: another user", &path).user(&bob).await;
    s.patch("patch: one setting, two back to their defaults", &path)
        .json(json!({"cardOpen": "full", "theme": null, "haptics": null}))
        .user(&alice)
        .await;
    s.patch("patch: nothing", &path)
        .json(json!({}))
        .user(&alice)
        .await;
    s.patch("patch: an old name of a time zone", &path)
        .json(json!({"timeZone": "Asia/Calcutta"}))
        .user(&alice)
        .await;

    let refused: [(&str, Value); 13] = [
        (
            "no such time zone",
            json!({"timeZone": "Mars/Olympus_Mons"}),
        ),
        (
            "a time zone in other letters",
            json!({"timeZone": "asia/seoul"}),
        ),
        ("no such week start", json!({"weekStart": "tuesday"})),
        ("no such hour cycle", json!({"hourCycle": "h24"})),
        ("no such theme", json!({"theme": "sepia"})),
        ("no such way to open a card", json!({"cardOpen": "auto"})),
        ("no such density", json!({"density": "cozy"})),
        ("haptics as a word", json!({"haptics": "true"})),
        (
            "no shape of a language tag",
            json!({"language": "en-US-US"}),
        ),
        ("a number", json!({"theme": 3})),
        ("no such setting", json!({"locale": "ko"})),
        (
            "one valid, one not",
            json!({"density": "comfortable", "theme": "sepia"}),
        ),
        ("not an object", json!(["theme"])),
    ];
    for (name, body) in refused {
        s.patch(&format!("patch: {name}"), &path)
            .json(body)
            .user(&alice)
            .await;
    }
    s.patch("patch: no session", &path)
        .json(json!({"theme": "dark"}))
        .await;
    s.get("get: after the refusals", &path).user(&alice).await;

    // The bag of client state: names and values the server does not know, within bounds.
    s.patch("client state: set", &bag)
        .json(json!({"updatedFields": {"theme": "of-the-client", "tour.step": "2"}}))
        .user(&alice)
        .await;
    s.get("client state: get", &bag).user(&alice).await;
    s.get("get: untouched by the client state", &path)
        .user(&alice)
        .await;
    s.patch("client state: a name with a space", &bag)
        .json(json!({"updatedFields": {"with space": "1"}}))
        .user(&alice)
        .await;
    s.patch("client state: a name that is too long", &bag)
        .json(json!({"updatedFields": {"n".repeat(65): "1"}}))
        .user(&alice)
        .await;
    s.patch("client state: a value that is too long", &bag)
        .json(json!({"updatedFields": {"big": "v".repeat(4097)}}))
        .user(&alice)
        .await;
    let many: serde_json::Map<String, Value> = (0..99)
        .map(|index| (format!("entry-{index:02}"), json!("v")))
        .collect();
    s.patch("client state: more entries than the bag holds", &bag)
        .json(json!({"updatedFields": many}))
        .user(&alice)
        .await;
    s.patch("client state: remove", &bag)
        .json(json!({"deletedFields": ["theme"]}))
        .user(&alice)
        .await;
}

/// How a session reaches the server and what keeps another site's page from using it:
/// the cookie of a login, the request-forgery header, the origin of a request.
pub(crate) async fn cookies(s: &mut Session) {
    let [alice] = s.users("cookies", ["alice"]).await;
    let login = format!("{API}/login");
    let me = format!("{API}/users/me");
    let boards = format!("{ROOT_API}/boards");
    let credentials = json!({"username": alice, "password": "test-password"});
    let board = json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": "made"});
    // The address the test servers take for their own: the default `server_url`.
    let own_origin = "http://localhost:8000";
    let foreign_origin = "https://evil.example";

    // -- signing in -----------------------------------------------------------------
    let token = s
        .post("login: the cookie and an empty body", &login)
        .json(credentials.clone())
        .keep_headers(&["Set-Cookie"])
        .await
        .session_token();
    s.post("login: wrong password sets no cookie", &login)
        .json(json!({"username": alice, "password": "nope"}))
        .keep_headers(&["Set-Cookie"])
        .await;
    s.post("login: without the header", &login)
        .json(credentials.clone())
        .without_csrf_header()
        .await;
    s.post("login: from another origin", &login)
        .json(credentials.clone())
        .header("Origin", foreign_origin)
        .await;
    s.post("login: from the own origin", &login)
        .json(credentials.clone())
        .header("Origin", own_origin)
        .unrecorded()
        .await;

    // -- reading --------------------------------------------------------------------
    s.get("read: cookie, no header", &me)
        .token(&token)
        .in_cookie()
        .without_csrf_header()
        .await;
    s.get("read: bearer, no header", &me)
        .token(&token)
        .without_csrf_header()
        .await;
    s.get("read: cookie, another origin named", &me)
        .token(&token)
        .in_cookie()
        .header("Origin", foreign_origin)
        .await;
    s.get(
        "read: the token in the address",
        &format!("{me}?access_token={token}"),
    )
    .await;
    s.get("read: a cookie that is no session's", &me)
        .token("not-a-session")
        .in_cookie()
        .await;
    s.get("read: the cookie of another name", &me)
        .header("Cookie", &format!("FVOCIAUTHTOKEN={token}"))
        .await;

    // -- changing -------------------------------------------------------------------
    s.post("change: cookie, no header", &boards)
        .json(board.clone())
        .token(&token)
        .in_cookie()
        .without_csrf_header()
        .await;
    s.post("change: cookie and header", &boards)
        .json(board.clone())
        .token(&token)
        .in_cookie()
        .await;
    s.post("change: bearer, no header", &boards)
        .json(board.clone())
        .token(&token)
        .without_csrf_header()
        .await;
    s.post("change: cookie and header, the own origin", &boards)
        .json(board.clone())
        .token(&token)
        .in_cookie()
        .header("Origin", own_origin)
        .await;
    s.post("change: cookie and header, another origin", &boards)
        .json(board.clone())
        .token(&token)
        .in_cookie()
        .header("Origin", foreign_origin)
        .await;
    s.post("change: bearer, another origin", &boards)
        .json(board.clone())
        .token(&token)
        .header("Origin", foreign_origin)
        .await;
    s.post("change: cookie and header, no origin to speak of", &boards)
        .json(board.clone())
        .token(&token)
        .in_cookie()
        .header("Origin", "null")
        .await;
    s.post(
        "change: bearer and cookie, the bearer is no session's",
        &boards,
    )
    .json(board)
    .token("not-a-session")
    .header("Cookie", &format!("fvoci_session={token}"))
    .await;

    // -- signing out ----------------------------------------------------------------
    s.post("logout: cookie, no header", &format!("{API}/logout"))
        .token(&token)
        .in_cookie()
        .without_csrf_header()
        .await;
    s.post("logout: clears the cookie", &format!("{API}/logout"))
        .token(&token)
        .in_cookie()
        .keep_headers(&["Set-Cookie"])
        .await;
    s.get("read: after the logout", &me)
        .token(&token)
        .in_cookie()
        .await;
}

/// One attempt too many: an address that keeps failing at an account is told to wait,
/// with the same words whether the account exists.
pub(crate) async fn login_limits(s: &mut Session) {
    let [alice, bob] = s.users("limits", ["alice", "bob"]).await;
    let login = format!("{API}/login");
    let attempt =
        |username: &str, password: &str| json!({"username": username, "password": password});

    for account in [alice.as_str(), "limits-nobody"] {
        // Five failures are free; the sixth is refused like them and starts the wait.
        for number in 1..=6 {
            s.post(&format!("{account}: wrong password {number}"), &login)
                .json(attempt(account, "not-the-password"))
                .await;
        }
        s.post(&format!("{account}: one too many"), &login)
            .json(attempt(account, "not-the-password"))
            .keep_headers(&["Retry-After"])
            .await;
    }
    s.post("the right password waits too", &login)
        .json(attempt(&alice, "test-password"))
        .keep_headers(&["Retry-After"])
        .await;
    s.post("the account in other letters is the same account", &login)
        .json(attempt(&alice.to_uppercase(), "test-password"))
        .await;
    s.post("another account is not held up", &login)
        .json(attempt(&bob, "test-password"))
        .await;
    s.get("a session from before goes on", &format!("{API}/users/me"))
        .user(&alice)
        .await;

    // The names are counted as the request gives them, before any is looked up: a
    // spelling that finds no account waits with the name it is a spelling of, whether
    // that name is an account or not.
    for account in [alice.as_str(), "limits-nobody"] {
        s.post(&format!("{account}: with a space after it"), &login)
            .json(attempt(&format!("{account} "), "not-the-password"))
            .await;
    }
}

/// A session is not the password: the old password is guessed at through a change no
/// more often than through a login, and the two are one count.
pub(crate) async fn password_limits(s: &mut Session) {
    let [carol] = s.users("password-limits", ["carol"]).await;
    let login = format!("{API}/login");
    let change = format!("{API}/users/me/password");
    let guess = json!({"oldPassword": "not-the-password", "newPassword": "another-password"});
    for number in 1..=6 {
        s.post(&format!("wrong old password {number}"), &change)
            .json(guess.clone())
            .user(&carol)
            .await;
    }
    s.post("one guess too many", &change)
        .json(guess)
        .user(&carol)
        .keep_headers(&["Retry-After"])
        .await;
    s.post("the right old password waits too", &change)
        .json(json!({"oldPassword": "test-password", "newPassword": "another-password"}))
        .user(&carol)
        .await;
    s.post("a login to the account waits with it", &login)
        .json(json!({"username": carol, "password": "test-password"}))
        .await;
    s.get("the session goes on", &format!("{API}/users/me"))
        .user(&carol)
        .await;
}
