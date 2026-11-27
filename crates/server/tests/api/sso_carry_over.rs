//! Carrying an account over to the provider an organization moved to: a sign-in
//! through the new provider that vouches for somebody without an account is held for
//! the browser, and ends either in the account the former provider vouches for, with
//! the new identity tied to it, or in a new account. The former provider alone signs
//! nobody in, another browser's held sign-in is not used, and after the keeping ended
//! nothing is held or carried over.

use std::collections::BTreeMap;
use std::sync::Mutex;

use fvoci_model::{
    ContainerVisibility, LinkedIdentity, OidcProvider, OidcStart, OrgRole, WorkspaceRole,
};
use fvoci_store::Identity;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::boards::{member, new_board};
use crate::common::{Client, ROOT_WORKSPACE_ID, TEST_WORKSPACE_ID, TestServer};
use crate::fake_oidc::{FakeProvider, Person};
use crate::oidc::{org_of, set_provider};
use crate::sso_move::{records, server_for_two, settings_of};

/// A browser of somebody who is not signed in: it follows no redirect by itself, keeps
/// its cookies, and trusts both providers' certificates.
pub(crate) struct Visitor {
    http: reqwest::Client,
    base_url: String,
    root: String,
    cookies: Mutex<BTreeMap<String, String>>,
}

impl Visitor {
    pub(crate) fn new(ts: &TestServer, providers: &[&FakeProvider]) -> Self {
        let certificates = providers.iter().map(|fake| {
            reqwest::Certificate::from_der(&fake.certificate_der).expect("a certificate")
        });
        Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .tls_certs_merge(certificates)
                .build()
                .expect("a client"),
            base_url: ts.base_url.clone(),
            root: ts.app.config().server_url.trim_end_matches('/').to_owned(),
            cookies: Mutex::new(BTreeMap::new()),
        }
    }

    /// The cookie `name`, if the browser has it.
    pub(crate) fn cookie(&self, name: &str) -> Option<String> {
        self.cookies.lock().expect("no panic").get(name).cloned()
    }

    /// Sets a cookie as if the browser had it.
    pub(crate) fn set_cookie(&self, name: &str, value: &str) {
        let mut cookies = self.cookies.lock().expect("no panic");
        cookies.insert(name.to_owned(), value.to_owned());
    }

    /// Keeps the cookies an answer sets, and drops those it clears.
    fn keep(&self, response: &reqwest::Response) {
        let mut cookies = self.cookies.lock().expect("no panic");
        let set = response.headers().get_all(reqwest::header::SET_COOKIE);
        for cookie in set.iter().filter_map(|value| value.to_str().ok()) {
            let Some((name, value)) = cookie.split(';').next().and_then(|c| c.split_once('='))
            else {
                continue;
            };
            if value.is_empty() || cookie.contains("Max-Age=0") {
                cookies.remove(name);
            } else {
                cookies.insert(name.to_owned(), value.to_owned());
            }
        }
    }

    fn cookie_header(&self) -> String {
        let cookies = self.cookies.lock().expect("no panic");
        let pairs: Vec<String> = cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        pairs.join("; ")
    }

    /// Sends a request of the login page: `POST path`, with the browser's cookies.
    pub(crate) async fn post(&self, path: &str) -> reqwest::Response {
        let response = self
            .http
            .post(format!("{}/api/v1{path}", self.base_url))
            .header("X-Requested-With", "XMLHttpRequest")
            .header("Cookie", self.cookie_header())
            .send()
            .await
            .expect("the server answers");
        self.keep(&response);
        response
    }

    /// Drops a cookie, as a browser that lost it.
    pub(crate) fn forget(&self, name: &str) {
        self.cookies.lock().expect("no panic").remove(name);
    }

    /// Starts a sign-in with `POST path` and lets the provider it leads to answer: the
    /// path of the server the provider sends the browser back to; or the reason the
    /// start was refused with.
    pub(crate) async fn to_provider(&self, path: &str) -> Result<String, String> {
        let started = self.post(path).await;
        if !started.status().is_success() {
            let refused_with = started.status();
            assert_eq!(
                refused_with,
                StatusCode::CONFLICT,
                "a start is refused with 409"
            );
            let problem: Value = started.json().await.expect("a problem");
            return Err(problem["reason"].as_str().expect("a reason").to_owned());
        }
        let started: OidcStart = started.json().await.expect("where the browser goes");
        let authorized = self
            .http
            .get(&started.authorization_url)
            .send()
            .await
            .expect("the provider answers");
        let back = location_of(&authorized);
        Ok(back
            .strip_prefix(&self.root)
            .expect("under the server")
            .to_owned())
    }

    /// Comes back from a provider to `path` of the server, with the browser's cookies:
    /// where the server sends the browser then, under its own address.
    pub(crate) async fn come_back(&self, path: &str) -> String {
        let answer = self
            .http
            .get(format!("{}{path}", self.base_url))
            .header("Cookie", self.cookie_header())
            .send()
            .await
            .expect("the server answers");
        assert_eq!(answer.status(), StatusCode::SEE_OTHER, "a return sends on");
        self.keep(&answer);
        let location = location_of(&answer);
        location
            .strip_prefix(&self.root)
            .expect("under the server")
            .to_owned()
    }

    /// Starts a sign-in with `POST path`, goes to the provider and comes back: where
    /// the server sends the browser then; or the reason the start was refused with.
    pub(crate) async fn through_provider(&self, path: &str) -> Result<String, String> {
        let back = self.to_provider(path).await?;
        Ok(self.come_back(&back).await)
    }

    /// Signs in through the organization's provider, as the login page does.
    pub(crate) async fn sign_in(&self, org_id: &str) -> String {
        self.through_provider(&format!("/auth/oidc/start?org={org_id}"))
            .await
            .expect("the sign-in starts")
    }

    /// Carries the held sign-in over through the former provider.
    pub(crate) async fn carry_over(&self, org_id: &str) -> Result<String, String> {
        self.through_provider(&format!("/auth/oidc/held/carry-over?org={org_id}"))
            .await
    }

    /// The client of the session the browser has, if it has one.
    pub(crate) fn session(&self, ts: &TestServer) -> Option<Client> {
        self.cookie("fvoci_session")
            .map(|token| ts.client_with_token(&token))
    }
}

fn location_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("a Location header")
        .to_owned()
}

/// An organization that moved from `old` to `new`, taking passwords, with `ada`, a
/// member whose account the old provider made: the server, the organization, and
/// ada's identifier.
pub(crate) struct Moved {
    pub(crate) ts: TestServer,
    pub(crate) org_id: String,
    pub(crate) ada: String,
}

pub(crate) const ADA: &str = "ada-at-the-old-provider";
const ADA_NEW: &str = "ada-at-the-new-provider";

pub(crate) async fn moved(old: &FakeProvider, new: &FakeProvider) -> Moved {
    let ts = server_for_two(old, new, |_| {}).await;
    ts.app
        .store()
        .system()
        .execute_unchecked("UPDATE users SET email_verified_at = created_at WHERE email <> ''")
        .await
        .expect("fixture accounts have verified addresses");
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, old).await;
    old.sign_in_as(Person::new(ADA, "ada@old.example.com"));
    let before = Visitor::new(&ts, &[old, new]);
    assert_eq!(before.sign_in(&org_id).await, "/", "ada's first sign-in");
    let ada = before.session(&ts).expect("a session").me().await.id;
    ts.client
        .put(&format!("/org/{org_id}/oidc"), &settings_of(new))
        .await
        .check_ok();
    new.sign_in_as(Person::new(ADA_NEW, "ada@new.example.com"));
    Moved { ts, org_id, ada }
}

async fn count(ts: &TestServer, table: &str) -> i64 {
    let rows = ts.app.store().system().count_rows(table).await;
    rows.expect("counted")
}

pub(crate) fn held_page(org_id: &str) -> String {
    format!("/login?org={org_id}&oidc_held=1")
}

fn refused_page(org_id: &str) -> String {
    format!("/login?org={org_id}&oidc_error=oidc-carry-over-refused")
}

/// After the move, ada signs in through the new provider, which knows her under
/// another subject and address: no account is made, the sign-in is held, and by
/// signing in through the former provider once she is in her own account, with the
/// new identity tied to it, in a session made through the new provider. The next
/// sign-in through the new provider goes straight in.
#[tokio::test]
async fn a_member_carries_their_account_over() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let Moved { ts, org_id, ada } = moved(&old, &new).await;
    let (users, sessions) = (count(&ts, "users").await, count(&ts, "sessions").await);
    let browser = Visitor::new(&ts, &[&old, &new]);

    let held = browser.sign_in(&org_id).await;
    let after_the_hold = (
        count(&ts, "users").await,
        count(&ts, "sessions").await,
        browser.cookie("fvoci_session"),
        browser.cookie("fvoci_oidc_held").is_some(),
    );
    let carried = browser.carry_over(&org_id).await;
    let session = browser.session(&ts).expect("a session is set");
    let identities: Vec<LinkedIdentity> = session.get("/users/me/identities").await.ok_json();
    let again = Visitor::new(&ts, &[&old, &new]);
    let next = again.sign_in(&org_id).await;

    assert_eq!(held, held_page(&org_id));
    assert_eq!(
        after_the_hold,
        (users, sessions, None, true),
        "held: no account, no session, the cookie of what is held"
    );
    assert_eq!(carried, Ok("/".to_owned()), "to the web app");
    assert_eq!(session.me().await.id, ada, "her own account");
    assert_eq!(count(&ts, "users").await, users, "no account made");
    assert_eq!(browser.cookie("fvoci_oidc_held"), None, "taken");
    let of_the_new: Vec<_> = identities
        .iter()
        .filter(|identity| identity.issuer == new.issuer)
        .collect();
    assert_eq!(of_the_new.len(), 1, "{identities:?}");
    assert_eq!(of_the_new[0].subject, ADA_NEW);
    assert_eq!(of_the_new[0].email, "ada@new.example.com");
    assert!(
        of_the_new[0].made_this_session,
        "the session is made through the new provider, for the new identity"
    );
    session
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
        .await
        .check_ok();
    assert_eq!(
        records(&ts.client, &org_id).await[0],
        (
            "oidc_identity_carried_over".to_owned(),
            "person".to_owned(),
            new.issuer.clone()
        )
    );
    assert_eq!(next, "/", "the new provider knows her now");
    assert_eq!(again.session(&ts).expect("a session").me().await.id, ada);
    ts.teardown().await;
    old.stop();
    new.stop();
}

/// The former provider signs nobody in by itself: without a sign-in held for the
/// browser nothing starts through it, a cookie of the browser's own making holds
/// nothing, and the held sign-in of one browser is not another's to carry over.
#[tokio::test]
async fn nothing_is_carried_over_without_the_browsers_own_held_sign_in() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let Moved { ts, org_id, ada } = moved(&old, &new).await;
    let holder = Visitor::new(&ts, &[&old, &new]);
    assert_eq!(holder.sign_in(&org_id).await, held_page(&org_id));
    let (identities, sessions) = (
        count(&ts, "user_identities").await,
        count(&ts, "sessions").await,
    );
    old.sign_in_as(Person::new(ADA, "ada@old.example.com"));

    let without = Visitor::new(&ts, &[&old, &new]);
    let nothing_held = without.carry_over(&org_id).await;
    let forged = Visitor::new(&ts, &[&old, &new]);
    forged.set_cookie("fvoci_oidc_held", "a secret of the browser's own making");
    let forged_held = forged.carry_over(&org_id).await;
    let no_organization = holder.post("/auth/oidc/held/carry-over").await;
    // An organization that is not there is told as nothing held is, to the holder too.
    let elsewhere = fvoci_model::new_id();
    let unknown_organization = holder
        .to_provider(&format!("/auth/oidc/held/carry-over?org={elsewhere}"))
        .await;

    let refused = Err("oidc_carry_over_refused".to_owned());
    assert_eq!(nothing_held, refused);
    assert_eq!(forged_held, refused);
    assert_eq!(unknown_organization, refused);
    assert_eq!(no_organization.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        (
            count(&ts, "user_identities").await,
            count(&ts, "sessions").await
        ),
        (identities, sessions),
        "nothing tied, nobody signed in"
    );
    assert_eq!(without.cookie("fvoci_session"), None);
    // The holder's own sign-in is still theirs to carry over.
    assert_eq!(holder.carry_over(&org_id).await, Ok("/".to_owned()));
    assert_eq!(holder.session(&ts).expect("a session").me().await.id, ada);
    ts.teardown().await;
    old.stop();
    new.stop();
}

/// Who the former provider vouches for must have an account in use with a place in
/// the organization: an identity nobody has, and one of an account that is no member
/// and in none of its workspaces, carry nothing over, and each try takes the held sign-in.
#[tokio::test]
async fn an_identity_of_nobody_with_a_place_carries_nothing_over() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let Moved { ts, org_id, .. } = moved(&old, &new).await;
    // Somebody with an account and an identity at the old provider, but no place here.
    let outsider = ts.client2.me().await.id;
    let at_the_old = Identity {
        issuer: &old.issuer,
        subject: "an outsider",
    };
    let store = ts.app.store().system();
    store
        .add_identity(&outsider, at_the_old, "")
        .await
        .expect("tied");
    for table in ["organization_members", "workspace_members"] {
        store
            .execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{outsider}'"))
            .await
            .expect("no member any more");
    }
    let (identities, sessions) = (
        count(&ts, "user_identities").await,
        count(&ts, "sessions").await,
    );

    let mut outcomes = Vec::new();
    for subject in ["known to nobody", "an outsider"] {
        let browser = Visitor::new(&ts, &[&old, &new]);
        new.sign_in_as(Person::new(ADA_NEW, "ada@new.example.com"));
        assert_eq!(browser.sign_in(&org_id).await, held_page(&org_id));
        old.sign_in_as(Person::new(subject, "somebody@old.example.com"));
        let first = browser.carry_over(&org_id).await;
        let second = browser.carry_over(&org_id).await;
        outcomes.push((first, second, browser.cookie("fvoci_session")));
    }

    for outcome in outcomes {
        assert_eq!(
            outcome,
            (
                Ok(refused_page(&org_id)),
                Err("oidc_carry_over_refused".to_owned()),
                None
            ),
            "refused, and the held sign-in is taken by the try"
        );
    }
    assert_eq!(
        (
            count(&ts, "user_identities").await,
            count(&ts, "sessions").await
        ),
        (identities, sessions),
        "nothing tied, nobody signed in"
    );
    ts.teardown().await;
    old.stop();
    new.stop();
}

const GWEN: &str = "gwen-at-the-old-provider";
const GWEN_NEW: &str = "gwen-at-the-new-provider";

/// The second user made a guest of the test workspace alone, no member of the
/// organization, whose account the old provider vouches for: their identifier.
async fn guest_known_to_the_old(ts: &TestServer, old: &FakeProvider) -> String {
    let guest = ts.client2.me().await.id;
    let store = ts.app.store().system();
    for statement in [
        format!("DELETE FROM organization_members WHERE user_id = '{guest}'"),
        format!(
            "DELETE FROM workspace_members WHERE user_id = '{guest}' \
             AND workspace_id <> '{TEST_WORKSPACE_ID}'"
        ),
        format!(
            "UPDATE workspace_members SET role = '{}' WHERE user_id = '{guest}'",
            WorkspaceRole::Guest.as_str()
        ),
    ] {
        store
            .execute_unchecked(&statement)
            .await
            .expect("a guest of the test workspace alone");
    }
    let at_the_old = Identity {
        issuer: &old.issuer,
        subject: GWEN,
    };
    store
        .add_identity(&guest, at_the_old, "")
        .await
        .expect("tied");
    guest
}

/// What the account is in the organization and its workspaces: its role in the
/// organization, and each workspace it is in with its role there.
async fn place_of(
    ts: &TestServer,
    org_id: &str,
    user_id: &str,
) -> (Option<OrgRole>, Vec<(String, WorkspaceRole)>) {
    let store = ts.app.store().system();
    let org_role = store
        .org_role_of_user(user_id, org_id)
        .await
        .expect("asked");
    let workspaces = store
        .workspaces_of_user(user_id, fvoci_store::Making::with_password())
        .await
        .expect("listed")
        .into_iter()
        .map(|workspace| (workspace.id, workspace.role))
        .collect();
    (org_role, workspaces)
}

/// A guest of a workspace, who is no member of the organization, has a place in it as
/// a sign-in counts one, and carries their account over as a member does: the boards
/// they were invited to open as before, no other does, and they stay a guest, as the
/// next sign-in through the new provider leaves them.
#[tokio::test]
async fn a_guest_carries_their_account_over_and_keeps_their_boards() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let Moved { ts, org_id, .. } = moved(&old, &new).await;
    let guest = guest_known_to_the_old(&ts, &old).await;
    let board = async |title: &str| {
        let board: fvoci_model::Container = ts
            .client
            .create_board(&new_board(
                TEST_WORKSPACE_ID,
                ContainerVisibility::Private,
                title,
            ))
            .await
            .created_json();
        board.id
    };
    let invited = board("Invited").await;
    let other = board("Not invited").await;
    ts.client
        .add_member(&member(
            &invited,
            &guest,
            fvoci_model::ContainerRole::Viewer,
        ))
        .await
        .check_ok();
    let before = place_of(&ts, &org_id, &guest).await;
    let users = count(&ts, "users").await;

    new.sign_in_as(Person::new(GWEN_NEW, "gwen@new.example.com"));
    let browser = Visitor::new(&ts, &[&old, &new]);
    let held = browser.sign_in(&org_id).await;
    old.sign_in_as(Person::new(GWEN, "gwen@old.example.com"));
    let carried = browser.carry_over(&org_id).await;
    let session = browser.session(&ts).expect("a session is set");
    let again = Visitor::new(&ts, &[&old, &new]);
    let next = again.sign_in(&org_id).await;

    assert_eq!(
        before,
        (
            None,
            vec![(TEST_WORKSPACE_ID.to_owned(), WorkspaceRole::Guest)]
        )
    );
    assert_eq!(held, held_page(&org_id));
    assert_eq!(carried, Ok("/".to_owned()));
    assert_eq!(session.me().await.id, guest, "the guest's own account");
    assert_eq!(count(&ts, "users").await, users, "no account made");
    session
        .get(&format!("/w/{TEST_WORKSPACE_ID}/containers/{invited}"))
        .await
        .check_ok();
    session
        .get(&format!("/w/{TEST_WORKSPACE_ID}/containers/{other}"))
        .await
        .check_not_found();
    let listed: Vec<fvoci_model::Container> = session
        .get(&format!("/w/{TEST_WORKSPACE_ID}/boards"))
        .await
        .ok_json();
    assert_eq!(
        listed
            .iter()
            .map(|board| board.id.as_str())
            .collect::<Vec<_>>(),
        [invited.as_str()],
        "the boards of their invitations, and no other"
    );
    assert_eq!(
        place_of(&ts, &org_id, &guest).await,
        before,
        "a guest still, of the one workspace"
    );
    assert_eq!(
        records(&ts.client, &org_id).await[0],
        (
            "oidc_identity_carried_over".to_owned(),
            "person".to_owned(),
            new.issuer.clone()
        )
    );
    assert_eq!(next, "/", "the new provider knows them now");
    assert_eq!(again.session(&ts).expect("a session").me().await.id, guest);
    assert_eq!(
        place_of(&ts, &org_id, &guest).await,
        before,
        "the next sign-in joins nothing"
    );
    ts.teardown().await;
    old.stop();
    new.stop();
}

/// The held identity tied to another account while the person was at the former
/// provider is not tied a second time, and signs nobody in.
#[tokio::test]
async fn a_held_identity_tied_to_another_account_meanwhile_is_refused() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let Moved { ts, org_id, ada } = moved(&old, &new).await;
    let browser = Visitor::new(&ts, &[&old, &new]);
    assert_eq!(browser.sign_in(&org_id).await, held_page(&org_id));
    let another = ts.client2.me().await.id;
    let held = Identity {
        issuer: &new.issuer,
        subject: ADA_NEW,
    };
    let store = ts.app.store().system();
    store.add_identity(&another, held, "").await.expect("tied");
    old.sign_in_as(Person::new(ADA, "ada@old.example.com"));

    let carried = browser.carry_over(&org_id).await;

    assert_eq!(carried, Ok(refused_page(&org_id)));
    assert_eq!(browser.cookie("fvoci_session"), None);
    let of_ada = store.identities_of_user(&ada).await.expect("read");
    assert_eq!(of_ada.len(), 1, "only the old one: {of_ada:?}");
    let owner = store.user_of_identity(held).await.expect("read");
    assert_eq!(owner.map(|user| user.id), Some(another));
    ts.teardown().await;
    old.stop();
    new.stop();
}

/// Once the keeping of the former provider ended, early or by its time, a sign-in
/// held before carries nothing over, also one whose return from the former provider
/// was under way; and a new sign-in is held no more: it ends as a first sign-in.
#[tokio::test]
async fn after_the_keeping_ended_nothing_is_held_or_carried_over() {
    for ending in ["early", "lapsed"] {
        let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
        let Moved { ts, org_id, ada } = moved(&old, &new).await;
        let path = format!("/org/{org_id}/oidc");
        let waiting = Visitor::new(&ts, &[&old, &new]);
        assert_eq!(waiting.sign_in(&org_id).await, held_page(&org_id));
        old.sign_in_as(Person::new(ADA, "ada@old.example.com"));
        if ending == "early" {
            let read: OidcProvider = ts.client.get(&path).await.ok_json();
            let kept_since = read.former.expect("kept").created_at;
            let ended = ts
                .client
                .request(reqwest::Method::DELETE, &format!("{path}/former"))
                .json(&json!({ "createdAt": kept_since }));
            Client::send(ended).await.check_no_content();
        } else {
            let lapse = "UPDATE oidc_former_providers SET expires_at = 1";
            let store = ts.app.store().system();
            store.execute_unchecked(lapse).await.expect("lapsed");
        }
        let identities = count(&ts, "user_identities").await;

        let carried = waiting.carry_over(&org_id).await;
        let newcomer = Visitor::new(&ts, &[&old, &new]);
        new.sign_in_as(Person::new("a newcomer", "newcomer@new.example.com"));
        let signed_in = newcomer.sign_in(&org_id).await;

        assert_eq!(
            carried,
            Err("oidc_carry_over_refused".to_owned()),
            "{ending}: nothing starts through the former provider"
        );
        assert_eq!(waiting.cookie("fvoci_session"), None, "{ending}");
        assert_eq!(signed_in, "/", "{ending}: no longer held");
        let made = newcomer.session(&ts).expect("a session").me().await.id;
        assert_ne!(made, ada, "{ending}: an account of their own");
        assert_eq!(
            count(&ts, "user_identities").await,
            identities + 1,
            "{ending}: the newcomer's identity, and no other"
        );
        ts.teardown().await;
        old.stop();
        new.stop();
    }
}

/// The keeping ended while the person was at the former provider: the return carries
/// nothing over.
#[tokio::test]
async fn a_return_after_the_keeping_ended_carries_nothing_over() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let Moved { ts, org_id, ada } = moved(&old, &new).await;
    let browser = Visitor::new(&ts, &[&old, &new]);
    assert_eq!(browser.sign_in(&org_id).await, held_page(&org_id));
    old.sign_in_as(Person::new(ADA, "ada@old.example.com"));
    let carry_over = format!("/auth/oidc/held/carry-over?org={org_id}");
    let back = browser.to_provider(&carry_over).await.expect("started");
    let lapse = "UPDATE oidc_former_providers SET expires_at = 1";
    let store = ts.app.store().system();
    store.execute_unchecked(lapse).await.expect("lapsed");

    let returned = browser.come_back(&back).await;

    assert_eq!(returned, refused_page(&org_id));
    assert_eq!(
        count(&ts, "oidc_held_sign_ins").await,
        0,
        "taken by the try"
    );
    assert_eq!(browser.cookie("fvoci_session"), None);
    let of_ada = store.identities_of_user(&ada).await.expect("read");
    assert_eq!(of_ada.len(), 1, "only the old one: {of_ada:?}");
    ts.teardown().await;
    old.stop();
    new.stop();
}

/// Somebody who had no account ends the held sign-in in a new one, once, by the rules
/// of a first sign-in: with the organization closed to newcomers none is made.
#[tokio::test]
async fn a_held_sign_in_ends_in_a_new_account_as_a_first_sign_in_does() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let Moved { ts, org_id, ada } = moved(&old, &new).await;
    let users = count(&ts, "users").await;
    let anew = format!("/auth/oidc/held/sign-in?org={org_id}");
    let newcomer = Visitor::new(&ts, &[&old, &new]);
    new.sign_in_as(Person::new("a newcomer", "newcomer@new.example.com"));
    assert_eq!(newcomer.sign_in(&org_id).await, held_page(&org_id));

    let nothing_held = Visitor::new(&ts, &[&old, &new]).post(&anew).await;
    let made = newcomer.post(&anew).await;
    let session = newcomer.session(&ts).expect("a session is set");
    let again = newcomer.post(&anew).await;
    // Closed to newcomers, the next one is held all the same, and makes no account.
    let closed =
        json!({ "issuer": new.issuer, "clientId": crate::fake_oidc::CLIENT_ID, "autoJoin": false });
    let path = format!("/org/{org_id}/oidc");
    ts.client.put(&path, &closed).await.check_ok();
    let shut_out = Visitor::new(&ts, &[&old, &new]);
    new.sign_in_as(Person::new("another newcomer", "another@new.example.com"));
    let held = shut_out.sign_in(&org_id).await;
    let refused = shut_out.post(&anew).await;

    assert_eq!(nothing_held.status(), StatusCode::CONFLICT);
    let problem: Value = nothing_held.json().await.expect("a problem");
    assert_eq!(problem["reason"], "oidc_carry_over_refused");
    assert_eq!(made.status(), StatusCode::NO_CONTENT);
    let me = session.me().await;
    assert_ne!(me.id, ada);
    assert_eq!(count(&ts, "users").await, users + 1, "one account made");
    session
        .get(&format!("/w/{ROOT_WORKSPACE_ID}/boards"))
        .await
        .check_ok();
    assert_eq!(newcomer.cookie("fvoci_oidc_held"), None, "taken");
    assert_eq!(again.status(), StatusCode::CONFLICT, "taken once");
    assert_eq!(held, held_page(&org_id));
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let problem: Value = refused.json().await.expect("a problem");
    assert_eq!(problem["reason"], "oidc_join_closed");
    assert_eq!(shut_out.cookie("fvoci_session"), None);
    assert_eq!(count(&ts, "users").await, users + 1, "no other account");
    ts.teardown().await;
    old.stop();
    new.stop();
}

/// The return from the former provider carries over only for the browser that still
/// has the cookie of the very sign-in it was started for: without it, or with the
/// cookie of another held sign-in, nothing is tied, nobody is signed in, and the held
/// sign-in is taken by the try.
#[tokio::test]
async fn a_return_without_the_held_sign_ins_own_cookie_carries_nothing_over() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let Moved { ts, org_id, ada } = moved(&old, &new).await;
    let carry_over = format!("/auth/oidc/held/carry-over?org={org_id}");
    let store = ts.app.store().system();

    let mut returned = Vec::new();
    for lost in [true, false] {
        let browser = Visitor::new(&ts, &[&old, &new]);
        new.sign_in_as(Person::new(ADA_NEW, "ada@new.example.com"));
        assert_eq!(browser.sign_in(&org_id).await, held_page(&org_id));
        old.sign_in_as(Person::new(ADA, "ada@old.example.com"));
        let back = browser.to_provider(&carry_over).await.expect("started");
        if lost {
            browser.forget("fvoci_oidc_held");
        } else {
            // Another sign-in of the browser is held meanwhile, under another cookie.
            let other = Visitor::new(&ts, &[&old, &new]);
            new.sign_in_as(Person::new("another", "another@new.example.com"));
            assert_eq!(other.sign_in(&org_id).await, held_page(&org_id));
            let theirs = other.cookie("fvoci_oidc_held").expect("held");
            browser.set_cookie("fvoci_oidc_held", &theirs);
        }
        returned.push((
            browser.come_back(&back).await,
            browser.cookie("fvoci_session"),
        ));
    }

    for outcome in returned {
        assert_eq!(outcome, (refused_page(&org_id), None));
    }
    let of_ada = store.identities_of_user(&ada).await.expect("read");
    assert_eq!(of_ada.len(), 1, "only the old one: {of_ada:?}");
    assert_eq!(
        count(&ts, "oidc_held_sign_ins").await,
        1,
        "each held sign-in that was tried is taken; the other browser's is left"
    );
    ts.teardown().await;
    old.stop();
    new.stop();
}

/// Starting anew keeps every rule of a first sign-in: an address the provider says it
/// has not checked, none at all, or one that is another account's makes no account
/// and no session, and the held sign-in is taken with its cookie.
#[tokio::test]
async fn starting_anew_keeps_the_rules_of_a_first_sign_in() {
    let (old, new) = (FakeProvider::start().await, FakeProvider::start().await);
    let Moved { ts, org_id, .. } = moved(&old, &new).await;
    let anew = format!("/auth/oidc/held/sign-in?org={org_id}");
    // The address `user2` registered with.
    let taken = "user2@sample.com";
    let users = count(&ts, "users").await;
    let unverified = Person {
        email_verified: Some(false),
        ..Person::new("unverified", "unverified@new.example.com")
    };
    let without = Person {
        email: None,
        ..Person::new("without an address", "unused@new.example.com")
    };
    let of_another = Person::new("with another account's address", taken);

    let mut refusals = Vec::new();
    for person in [unverified, without, of_another] {
        let browser = Visitor::new(&ts, &[&old, &new]);
        new.sign_in_as(person);
        assert_eq!(browser.sign_in(&org_id).await, held_page(&org_id));
        let refused = browser.post(&anew).await;
        let status = refused.status();
        let problem: Value = refused.json().await.expect("a problem");
        let reason = problem["reason"].as_str().unwrap_or_default().to_owned();
        let left = (
            browser.cookie("fvoci_session"),
            browser.cookie("fvoci_oidc_held"),
        );
        refusals.push((status, reason, left));
    }

    let reasons: Vec<&str> = refusals
        .iter()
        .map(|(_, reason, _)| reason.as_str())
        .collect();
    assert_eq!(
        reasons,
        [
            "oidc_email_unverified",
            "oidc_email_missing",
            "oidc_email_taken"
        ]
    );
    for (status, reason, left) in &refusals {
        assert_eq!(*status, StatusCode::CONFLICT, "{reason}");
        assert_eq!(
            *left,
            (None, None),
            "{reason}: no session, and nothing held"
        );
    }
    assert_eq!(count(&ts, "users").await, users, "no account made");
    assert_eq!(count(&ts, "oidc_held_sign_ins").await, 0, "each is taken");
    ts.teardown().await;
    old.stop();
    new.stop();
}
