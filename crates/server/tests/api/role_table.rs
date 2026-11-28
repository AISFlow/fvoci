//! The table of roles: what every operation on one board or wiki, its people, its
//! sharing, its files, its place in the sidebar and the trash, and a page's comments and
//! versions, answers to every kind of person, a board's and a wiki's each pinned in a
//! file of `role_table/`.
//!
//! The people are the roles on a container for the invited only (admin, editor,
//! commenter, viewer), a member of the workspace without a role on it and on an open
//! one, a guest invited to it and one on an open one, the workspace's administrator
//! who only manages it, the organization's administrator who is not in the workspace,
//! and a member of another workspace on an open one, who asks under the workspace they
//! work in. The two who are not in the workspace are answered exactly as for nothing:
//! their cells say so (`=`), compared with the same question about identifiers of
//! nothing. Another organization is asked with real identifiers in the comparison of
//! two organizations (`organization_boundary_round2`, PostgreSQL only). Every answer is
//! asked of a container as it was made: one that an answer changed is made anew.
//!
//! A board and a wiki answer alike but where an open wiki is read without joining it
//! ([`the_wiki_differs_from_the_board_only_where_an_open_wiki_is_read`]).
//!
//! With `FVOCI_REGRESSION_UPDATE=1` the files are written anew instead of compared.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;

use fvoci_model::WorkspaceRole;
use reqwest::Method;
use reqwest::multipart::{Form, Part};
use serde_json::{Value, json};

use crate::common::{
    ApiResponse, Client, ROOT_WS, Setup, TEST_WORKSPACE_ID, TEST_WS, TestServer, UNKNOWN_ID,
};

/// A container kind exercised by the role tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Board,
    Wiki,
    Course,
    Template,
}

/// Who a container is for besides its members.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    /// For the invited only.
    Invited,
    /// Open to the workspace.
    Open,
}

impl Scope {
    const fn wire(self) -> &'static str {
        match self {
            Self::Invited => "private",
            Self::Open => "open",
        }
    }
}

/// The people of the table: who asks, and of which container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    Admin,
    Editor,
    Commenter,
    Viewer,
    Member,
    Guest,
    WorkspaceAdmin,
    OrgAdmin,
    OtherWorkspace,
}

/// The columns of the table, in their order: the header, who asks, of which container.
const COLUMNS: [(&str, Who, Scope); 11] = [
    ("admin", Who::Admin, Scope::Invited),
    ("editor", Who::Editor, Scope::Invited),
    ("commenter", Who::Commenter, Scope::Invited),
    ("viewer", Who::Viewer, Scope::Invited),
    ("member", Who::Member, Scope::Invited),
    ("member@open", Who::Member, Scope::Open),
    ("guest", Who::Guest, Scope::Invited),
    ("guest@open", Who::Guest, Scope::Open),
    ("ws-admin", Who::WorkspaceAdmin, Scope::Invited),
    ("org-admin", Who::OrgAdmin, Scope::Invited),
    ("other-ws", Who::OtherWorkspace, Scope::Open),
];

/// The column whose answers may differ between a board and a wiki.
const READS_UNJOINED: &str = "member@open";

/// One operation: the name of its row, its method and path (after `/api/v1`), what it
/// is sent with, and what is done to the container before it is asked.
struct Op {
    row: &'static str,
    method: Method,
    path: &'static str,
    body: Body,
    prepare: Prepare,
}

#[derive(Clone, Copy)]
enum Body {
    None,
    Json(fn(&Things, &People) -> Value),
    File,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Prepare {
    Nothing,
    /// The container is in the trash.
    DeleteContainer,
    /// Its page is in the trash.
    DeletePage,
    /// The comment on its page is in the trash.
    DeleteComment,
    /// The board's view is in the trash.
    DeleteView,
    /// The attachment of the board's card is in the trash.
    DeleteAttachment,
}

const fn op(row: &'static str, method: Method, path: &'static str, body: Body) -> Op {
    Op {
        row,
        method,
        path,
        body,
        prepare: Prepare::Nothing,
    }
}

/// The operations a board and a wiki share, in the order of the rows.
fn shared_ops() -> Vec<Op> {
    let mut ops = container_ops();
    ops.extend(holding_ops());
    ops
}

/// The operations of the container itself and of who is on it.
fn container_ops() -> Vec<Op> {
    use Body::{Json, None};
    vec![
        op("container: get", Method::GET, "/containers/{c}", None),
        op(
            "container: patch title",
            Method::PATCH,
            "/containers/{c}",
            Json(|_, _| json!({"title": "changed"})),
        ),
        op(
            "container: patch visibility",
            Method::PATCH,
            "/containers/{c}",
            Json(|things, _| json!({"visibility": things.scope.wire()})),
        ),
        op(
            "container: change visibility",
            Method::PATCH,
            "/containers/{c}",
            Json(|things, _| {
                let other = match things.scope {
                    Scope::Invited => Scope::Open,
                    Scope::Open => Scope::Invited,
                };
                json!({"visibility": other.wire()})
            }),
        ),
        op("container: delete", Method::DELETE, "/containers/{c}", None),
        Op {
            prepare: Prepare::DeleteContainer,
            ..op(
                "container: restore",
                Method::POST,
                "/containers/{c}/restore",
                Json(|_, _| json!({})),
            )
        },
        Op {
            prepare: Prepare::DeleteContainer,
            ..op("trash: list", Method::GET, "/trash/containers", None)
        },
        op(
            "members: list",
            Method::GET,
            "/containers/{c}/members",
            None,
        ),
        op(
            "members: add",
            Method::POST,
            "/containers/{c}/members",
            Json(|things, people| {
                let (board, user) = (&things.container, &people.extra);
                json!({"containerId": board, "userId": user, "role": "viewer"})
            }),
        ),
        op(
            "members: update",
            Method::PUT,
            "/containers/{c}/members/{viewer}",
            Json(|things, people| {
                let (board, user) = (&things.container, &people.viewer);
                json!({"containerId": board, "userId": user, "role": "commenter"})
            }),
        ),
        op(
            "members: remove",
            Method::DELETE,
            "/containers/{c}/members/{commenter}",
            None,
        ),
        op(
            "members: join",
            Method::POST,
            "/containers/{c}/join",
            Json(|_, _| json!({})),
        ),
        op(
            "members: leave",
            Method::POST,
            "/containers/{c}/leave",
            Json(|_, _| json!({})),
        ),
        op("teams: list", Method::GET, "/containers/{c}/teams", None),
        op(
            "teams: set",
            Method::PUT,
            "/containers/{c}/teams/{team}",
            Json(|_, _| json!({"role": "editor"})),
        ),
        op(
            "teams: remove",
            Method::DELETE,
            "/containers/{c}/teams/{team}",
            None,
        ),
    ]
}

/// The operations of its export in Markdown, its sharing, its files, its place in the
/// sidebar and what names what is on it.
fn holding_ops() -> Vec<Op> {
    use Body::{File, Json, None};
    vec![
        op(
            "container: Markdown export",
            Method::GET,
            "/containers/{c}/markdown/export",
            None,
        ),
        op(
            "public-link: get",
            Method::GET,
            "/containers/{c}/public-link",
            None,
        ),
        op(
            "public-link: set",
            Method::POST,
            "/containers/{c}/public-link",
            Json(|_, _| json!({"enabled": true})),
        ),
        op("files: upload", Method::POST, "/containers/{c}/files", File),
        op("files: get", Method::GET, "/containers/{c}/files/{f}", None),
        op(
            "files: info",
            Method::GET,
            "/containers/{c}/files/{f}/info",
            None,
        ),
        op(
            "file versions: list",
            Method::GET,
            "/containers/{c}/files/{f}/versions",
            None,
        ),
        op(
            "file versions: get",
            Method::GET,
            "/containers/{c}/files/{f}/versions/1",
            None,
        ),
        // From a version the file never had: who may write is told so (409), and the
        // file stays as it is for the next column.
        op(
            "file versions: add",
            Method::POST,
            "/containers/{c}/files/{f}/versions",
            Json(|things, _| json!({"baseVersion": 7, "fileId": things.file})),
        ),
        // Back to the version the file has: nothing is written.
        op(
            "file versions: restore",
            Method::POST,
            "/containers/{c}/files/{f}/versions/1/restore",
            Json(|_, _| json!({"baseVersion": 1})),
        ),
        op(
            "uploads: open",
            Method::POST,
            "/containers/{c}/uploads",
            Json(|_, _| json!({"name": "parts.bin", "size": 4})),
        ),
        op(
            "sidebar: category",
            Method::POST,
            "/categories/{category}/containers/{c}",
            Json(|_, _| json!({})),
        ),
        op(
            "sidebar: hide",
            Method::PUT,
            "/categories/{category}/containers/{c}/hide",
            None,
        ),
        op(
            "sidebar: unhide",
            Method::PUT,
            "/categories/{category}/containers/{c}/unhide",
            None,
        ),
        op("favorites: star it", Method::PUT, "/favorites/{c}", None),
        op(
            "favorites: star a block",
            Method::PUT,
            "/favorites/{p}",
            None,
        ),
        op("references: to", Method::GET, "/references/to/{p}", None),
        op(
            "references: resolve",
            Method::POST,
            "/references/resolve",
            Json(|things, _| json!([things.block])),
        ),
        op("reactions: add", Method::PUT, REACTION, None),
        op("reactions: remove", Method::DELETE, REACTION, None),
    ]
}

/// The reaction 👍 to the container's card, page or unit.
const REACTION: &str = "/containers/{c}/blocks/{p}/reactions/%F0%9F%91%8D";

/// The operations of what a board holds.
#[expect(
    clippy::too_many_lines,
    reason = "a table with one entry per operation, not logic"
)]
fn board_ops() -> Vec<Op> {
    use Body::{Json, None};
    vec![
        op("cards: list", Method::GET, "/boards/{c}/cards", None),
        op("cards: get", Method::GET, "/cards/{p}", None),
        op(
            "cards: create",
            Method::POST,
            "/boards/{c}/cards",
            Json(|_, _| json!({"title": "made"})),
        ),
        op(
            "cards: patch",
            Method::PATCH,
            "/cards/{p}",
            Json(|_, _| json!({"title": "changed"})),
        ),
        op(
            "cards: batch patch",
            Method::POST,
            "/boards/{c}/cards/batch-patch",
            Json(
                |things, _| json!({"cardIds": [things.block], "updatedProperties": {"note": "changed"}}),
            ),
        ),
        op(
            "attachments: list",
            Method::GET,
            "/boards/{c}/cards/{p}/attachments",
            None,
        ),
        op("views: list", Method::GET, "/boards/{c}/views", None),
        op(
            "views: patch",
            Method::PATCH,
            "/boards/{c}/views/{second}",
            Json(|_, _| json!({"title": "changed"})),
        ),
        op(
            "views: delete",
            Method::DELETE,
            "/boards/{c}/views/{second}",
            None,
        ),
        op(
            "views: get",
            Method::GET,
            "/boards/{c}/views/{second}",
            None,
        ),
        Op {
            prepare: Prepare::DeleteView,
            ..op(
                "views: restore",
                Method::POST,
                "/boards/{c}/views/{second}/restore",
                Json(|_, _| json!({})),
            )
        },
        op(
            "attachments: delete",
            Method::DELETE,
            "/boards/{c}/cards/{p}/attachments/{attachment}",
            None,
        ),
        Op {
            prepare: Prepare::DeleteAttachment,
            ..op(
                "attachments: restore",
                Method::POST,
                "/boards/{c}/cards/{p}/attachments/{attachment}/restore",
                Json(|_, _| json!({})),
            )
        },
        op(
            "comments: create",
            Method::POST,
            "/boards/{c}/cards/{p}/comments",
            Json(|_, _| json!({"body": said("said")})),
        ),
        op(
            "cards: body versions",
            Method::GET,
            "/cards/{p}/body-versions",
            None,
        ),
        op("cards: activity", Method::GET, "/cards/{p}/activity", None),
        op(
            "cards: keep version",
            Method::POST,
            "/cards/{p}/body-versions",
            None,
        ),
        op("cards: trash", Method::GET, "/boards/{c}/trash/cards", None),
        op(
            "board: duplicate",
            Method::POST,
            "/containers/{c}/duplicate",
            Json(|_, _| json!({})),
        ),
        op(
            "board: export",
            Method::GET,
            "/containers/{c}/archive/export",
            None,
        ),
    ]
}

/// The operations of what a wiki holds.
fn wiki_ops() -> Vec<Op> {
    use Body::{Json, None};
    vec![
        op("pages: tree", Method::GET, "/wikis/{c}/pages", None),
        op("pages: get", Method::GET, "/wikis/{c}/pages/{p}", None),
        op(
            "pages: create",
            Method::POST,
            "/wikis/{c}/pages",
            Json(|_, _| json!({"title": "made"})),
        ),
        op(
            "pages: patch",
            Method::PATCH,
            "/wikis/{c}/pages/{p}",
            Json(|_, _| json!({"title": "changed"})),
        ),
        op(
            "pages: move",
            Method::POST,
            "/wikis/{c}/pages/{p}/move",
            Json(|things, _| json!({"parentId": things.second})),
        ),
        op(
            "pages: duplicate",
            Method::POST,
            "/wikis/{c}/pages/{p}/duplicate",
            Json(|_, _| json!({})),
        ),
        op(
            "pages: delete",
            Method::DELETE,
            "/wikis/{c}/pages/{p}",
            None,
        ),
        op("pages: trash", Method::GET, "/wikis/{c}/trash/pages", None),
        op(
            "pages: comments",
            Method::GET,
            "/wikis/{c}/pages/{p}/comments",
            None,
        ),
        op(
            "pages: comment",
            Method::POST,
            "/wikis/{c}/pages/{p}/comments",
            Json(|_, _| {
                json!({"body": {"type": "doc", "content": [{"type": "paragraph", "content": [
                    {"type": "text", "text": "said"}
                ]}]}})
            }),
        ),
        op(
            "pages: versions",
            Method::GET,
            "/wikis/{c}/pages/{p}/body-versions",
            None,
        ),
        Op {
            prepare: Prepare::DeletePage,
            ..op(
                "pages: restore",
                Method::POST,
                "/wikis/{c}/pages/{p}/restore",
                Json(|_, _| json!({})),
            )
        },
    ]
}

/// The operations of a page's comments and of the versions of its body besides those
/// with the pages ([`wiki_ops`]: the list, writing one, the list of versions): one
/// comment's versions, its change, deletion and restoring, a version, going back to it,
/// keeping one now.
fn page_ops() -> Vec<Op> {
    use Body::{Json, None};
    const COMMENT: &str = "/wikis/{c}/pages/{p}/comments/{comment}";
    vec![
        op(
            "pages: comment versions",
            Method::GET,
            "/wikis/{c}/pages/{p}/comments/{comment}/versions",
            None,
        ),
        op(
            "pages: comment change",
            Method::PATCH,
            COMMENT,
            Json(|things, _| json!({"body": said("changed"), "readAt": things.comment_at})),
        ),
        op("pages: comment delete", Method::DELETE, COMMENT, None),
        Op {
            prepare: Prepare::DeleteComment,
            ..op(
                "pages: comment restore",
                Method::POST,
                "/wikis/{c}/pages/{p}/comments/{comment}/restore",
                Json(|_, _| json!({})),
            )
        },
        op(
            "pages: version",
            Method::GET,
            "/wikis/{c}/pages/{p}/body-versions/{version}",
            None,
        ),
        op(
            "pages: version restore",
            Method::POST,
            "/wikis/{c}/pages/{p}/body-versions/{version}/restore",
            None,
        ),
        op(
            "pages: keep version",
            Method::POST,
            "/wikis/{c}/pages/{p}/body-versions",
            None,
        ),
    ]
}

/// A body of one paragraph that says `text`.
fn said(text: &str) -> Value {
    json!({"type": "doc", "content": [{"type": "paragraph", "content": [
        {"type": "text", "text": text}
    ]}]})
}

/// The people, signed in, and who the operations name.
struct People {
    clients: BTreeMap<&'static str, Client>,
    editor: String,
    commenter: String,
    viewer: String,
    guest: String,
    /// A member of the workspace who asks nothing: whom `members: add` adds.
    extra: String,
    team: String,
}

impl People {
    async fn of(ts: &TestServer) -> Self {
        let mut clients = BTreeMap::new();
        clients.insert("admin", ts.client.clone());
        let mut ids = BTreeMap::new();
        for (name, role) in [
            ("editor", Some(WorkspaceRole::Member)),
            ("commenter", Some(WorkspaceRole::Member)),
            ("viewer", Some(WorkspaceRole::Member)),
            ("member", Some(WorkspaceRole::Member)),
            ("guest", Some(WorkspaceRole::Guest)),
            ("ws-admin", Some(WorkspaceRole::Admin)),
            ("extra", Some(WorkspaceRole::Member)),
            ("org-admin", None),
            ("other-ws", None),
        ] {
            let client = ts.add_user(&format!("table.{name}")).await;
            let id = client.me().await.id;
            // A new account is a member of the test workspace: what it is there is set,
            // or it is taken out, to be of the root workspace alone.
            let statement = match role {
                Some(role) => format!(
                    "UPDATE workspace_members SET role = '{}' WHERE user_id = '{id}' \
                     AND workspace_id = '{TEST_WORKSPACE_ID}'",
                    role.as_str()
                ),
                None => format!(
                    "DELETE FROM workspace_members WHERE user_id = '{id}' \
                     AND workspace_id = '{TEST_WORKSPACE_ID}'"
                ),
            };
            ts.app
                .store()
                .execute_unchecked(&statement)
                .await
                .expect("what the account is in the test workspace");
            clients.insert(name, client);
            ids.insert(name, id);
        }
        ts.app
            .store()
            .execute_unchecked(&format!(
                "UPDATE organization_members SET role = 'admin' WHERE user_id = '{}'",
                ids["org-admin"]
            ))
            .await
            .expect("the organization's administrator");
        // The workspace's administrators make its teams.
        let team = clients["ws-admin"]
            .post(&format!("{TEST_WS}/teams"), &json!({"name": "Table"}))
            .await
            .value();
        Self {
            clients,
            editor: ids["editor"].clone(),
            commenter: ids["commenter"].clone(),
            viewer: ids["viewer"].clone(),
            guest: ids["guest"].clone(),
            extra: ids["extra"].clone(),
            team: text(&team, "id"),
        }
    }

    fn client(&self, who: Who) -> &Client {
        let name = match who {
            Who::Admin => "admin",
            Who::Editor => "editor",
            Who::Commenter => "commenter",
            Who::Viewer => "viewer",
            Who::Member => "member",
            Who::Guest => "guest",
            Who::WorkspaceAdmin => "ws-admin",
            Who::OrgAdmin => "org-admin",
            Who::OtherWorkspace => "other-ws",
        };
        &self.clients[name]
    }
}

/// A container made for the operations, as its admin made it.
struct Things {
    scope: Scope,
    container: String,
    /// A card of the board, a page of the wiki.
    block: String,
    /// Another page of the wiki or unit of the course, to move the page below; a view of
    /// the board.
    second: String,
    file: String,
    /// The admin's comment on the page, and when it was written; empty for a board.
    comment: String,
    comment_at: Value,
    /// A version of the page's body, written twice; empty for a board.
    version: String,
    /// The attachment of the board's card; an identifier of nothing for other kinds.
    attachment: String,
}

fn text(value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("{member} in {value}"))
        .to_owned()
}

#[track_caller]
fn done(response: &ApiResponse) {
    assert!(
        response.status.is_success(),
        "{}: {}",
        response.status,
        response.text()
    );
}

/// Makes a container of `kind` and `scope` with its people: an editor, a commenter and a
/// viewer, the guest as an editor on one for the invited only, the team as a viewer;
/// a file, sharing on, a card and a view, or two pages, the first written twice
/// and commented on.
async fn make(people: &People, kind: Kind, scope: Scope) -> Things {
    let owner = &people.clients["admin"];
    let (at, new_body) = match kind {
        Kind::Board => (
            format!("{TEST_WS}/boards"),
            json!({"visibility": scope.wire()}),
        ),
        Kind::Wiki => (
            format!("{TEST_WS}/wikis"),
            json!({"visibility": scope.wire()}),
        ),
        Kind::Course => (
            format!("{TEST_WS}/courses"),
            json!({"visibility": scope.wire()}),
        ),
        Kind::Template => (
            format!("{TEST_WS}/boards"),
            json!({"visibility": scope.wire(), "isTemplate": true}),
        ),
    };
    let made = owner.post(&at, &new_body).await;
    done(&made);
    let container = text(&made.value(), "id");
    let at = format!("{TEST_WS}/containers/{container}");
    let mut roles = vec![
        (&people.editor, "editor"),
        (&people.commenter, "commenter"),
        (&people.viewer, "viewer"),
    ];
    if scope == Scope::Invited {
        roles.push((&people.guest, "editor"));
    }
    for (user, role) in roles {
        let member = json!({"containerId": container, "userId": user, "role": role});
        done(&owner.post(&format!("{at}/members"), &member).await);
        done(&owner.put(&format!("{at}/members/{user}"), &member).await);
    }
    let team = format!("{at}/teams/{}", people.team);
    done(&owner.put(&team, &json!({"role": "viewer"})).await);
    let file = Part::bytes(b"kept".to_vec()).file_name("kept.txt");
    let uploaded = Client::send(
        owner
            .request(Method::POST, &format!("{at}/files"))
            .multipart(Form::new().part("file", file)),
    )
    .await;
    done(&uploaded);
    done(
        &owner
            .post(&format!("{at}/public-link"), &json!({"enabled": true}))
            .await,
    );
    let file = text(&uploaded.value(), "fileId");
    make_kind_content(owner, kind, scope, container, file).await
}

/// Populates the sample card, course units, or wiki pages inside `container` for `make`.
async fn make_kind_content(
    owner: &Client,
    kind: Kind,
    scope: Scope,
    container: String,
    file: String,
) -> Things {
    match kind {
        Kind::Board | Kind::Template => {
            let card = owner
                .post(
                    &format!("{TEST_WS}/boards/{container}/cards"),
                    &json!({"title": "a card"}),
                )
                .await;
            done(&card);
            let view = owner
                .post(
                    &format!("{TEST_WS}/boards/{container}/views"),
                    &json!({"title": "a view", "viewType": "board"}),
                )
                .await;
            done(&view);
            let block = text(&card.value(), "id");
            let attachment = owner
                .post(
                    &format!("{TEST_WS}/boards/{container}/cards/{block}/attachments"),
                    &json!({"fileId": file}),
                )
                .await;
            done(&attachment);
            Things {
                scope,
                container,
                block,
                second: text(&view.value()["view"], "id"),
                file,
                comment: String::new(),
                comment_at: Value::Null,
                version: String::new(),
                attachment: text(&attachment.value(), "id"),
            }
        }
        Kind::Course => {
            let units = format!("{TEST_WS}/courses/{container}/units");
            let unit = owner.post(&units, &json!({"title": "a unit"})).await;
            let second = owner.post(&units, &json!({"title": "another"})).await;
            done(&unit);
            done(&second);
            let block = text(&unit.value(), "id");
            let at = format!("{units}/{block}");
            for body in ["first", "second"] {
                let read = owner.get(&at).await.value();
                let change = json!({"readAt": read["updatedAt"], "body": said(body)});
                done(&owner.patch(&at, &change).await);
            }
            let comment = owner
                .post(&format!("{at}/comments"), &json!({"body": said("theirs")}))
                .await;
            done(&comment);
            let comment = comment.value();
            Things {
                scope,
                container,
                block,
                second: text(&second.value(), "id"),
                file,
                comment: text(&comment, "id"),
                comment_at: comment["updatedAt"].clone(),
                version: "1".to_owned(),
                attachment: UNKNOWN_ID.to_owned(),
            }
        }
        Kind::Wiki => {
            let pages = format!("{TEST_WS}/wikis/{container}/pages");
            let page = owner.post(&pages, &json!({"title": "a page"})).await;
            let second = owner.post(&pages, &json!({"title": "another"})).await;
            done(&page);
            done(&second);
            let block = text(&page.value(), "id");
            let at = format!("{pages}/{block}");
            for body in ["first", "second"] {
                let read = owner.get(&at).await.value();
                let change = json!({"readAt": read["updatedAt"], "body": said(body)});
                done(&owner.patch(&at, &change).await);
            }
            let versions = owner.get(&format!("{at}/body-versions")).await.value();
            let comment = owner
                .post(&format!("{at}/comments"), &json!({"body": said("theirs")}))
                .await;
            done(&comment);
            let comment = comment.value();
            Things {
                scope,
                container,
                block,
                second: text(&second.value(), "id"),
                file,
                comment: text(&comment, "id"),
                comment_at: comment["updatedAt"].clone(),
                version: versions[0]["version"].to_string(),
                attachment: UNKNOWN_ID.to_owned(),
            }
        }
    }
}

/// Brings `things` to where `op` does something.
async fn prepare(people: &People, op: &Op, things: &Things) {
    let owner = &people.clients["admin"];
    match op.prepare {
        Prepare::Nothing => {}
        Prepare::DeleteContainer => {
            done(
                &owner
                    .delete(&format!("{TEST_WS}/containers/{}", things.container))
                    .await,
            );
        }
        Prepare::DeletePage => {
            let page = format!(
                "{TEST_WS}/wikis/{}/pages/{}",
                things.container, things.block
            );
            done(&owner.delete(&page).await);
        }
        Prepare::DeleteComment => {
            let comment = format!(
                "{TEST_WS}/wikis/{}/pages/{}/comments/{}",
                things.container, things.block, things.comment
            );
            done(&owner.delete(&comment).await);
        }
        Prepare::DeleteView => {
            let view = format!(
                "{TEST_WS}/boards/{}/views/{}",
                things.container, things.second
            );
            done(&owner.delete(&view).await);
        }
        Prepare::DeleteAttachment => {
            let attachment = format!(
                "{TEST_WS}/boards/{}/cards/{}/attachments/{}",
                things.container, things.block, things.attachment
            );
            done(&owner.delete(&attachment).await);
        }
    }
}

/// The category of the workspace at `at` that `client` files boards under; an
/// identifier of nothing for who has none there.
async fn category_of(client: &Client, at: &str) -> String {
    let categories = client.get(&format!("{at}/categories")).await;
    categories.value()[0]["id"]
        .as_str()
        .map_or_else(|| UNKNOWN_ID.to_owned(), str::to_owned)
}

/// Asks `op` about `things` as `client`; `category` is that client's.
async fn ask(
    op: &Op,
    client: &Client,
    at: &str,
    things: &Things,
    people: &People,
    category: &str,
) -> ApiResponse {
    let path = op
        .path
        .replace("{c}", &things.container)
        .replace("{p}", &things.block)
        .replace("{second}", &things.second)
        .replace("{f}", &things.file)
        .replace("{comment}", &things.comment)
        .replace("{version}", &things.version)
        .replace("{attachment}", &things.attachment)
        .replace("{team}", &people.team)
        .replace("{viewer}", &people.viewer)
        .replace("{commenter}", &people.commenter)
        .replace("{category}", category);
    let request = client.request(op.method.clone(), &format!("{at}{path}"));
    Client::send(match op.body {
        Body::None => request,
        Body::Json(body) => request.json(&body(things, people)),
        Body::File => {
            let file = Part::bytes(b"more".to_vec()).file_name("more.txt");
            request.multipart(Form::new().part("file", file))
        }
    })
    .await
}

/// What one answer is in the table: its status; for the trash whether it lists the
/// container (`+`) or not (`-`), and for references whether they name the block; and
/// `=` after an answer that is to the letter the one about identifiers of nothing.
fn cell(op: &Op, answer: &ApiResponse, things: &Things, nothing: Option<&ApiResponse>) -> String {
    let status = answer.status.as_u16();
    let named = match op.row {
        "trash: list" => Some(&things.container),
        "references: resolve" => Some(&things.block),
        _ => None,
    };
    if let Some(named) = named.filter(|_| answer.status.is_success()) {
        let listed = if answer.text().contains(named.as_str()) {
            "+"
        } else {
            "-"
        };
        return format!("{status}{listed}");
    }
    match nothing {
        Some(nothing)
            if nothing.status == answer.status && nothing.problem() == answer.problem() =>
        {
            format!("{status}=")
        }
        _ => status.to_string(),
    }
}

/// Identifiers of nothing, in place of what is made for a question.
fn nothing(scope: Scope) -> Things {
    Things {
        scope,
        container: UNKNOWN_ID.to_owned(),
        block: UNKNOWN_ID.to_owned(),
        second: UNKNOWN_ID.to_owned(),
        file: "missing-file".to_owned(),
        comment: UNKNOWN_ID.to_owned(),
        comment_at: json!(1),
        version: "1".to_owned(),
        attachment: UNKNOWN_ID.to_owned(),
    }
}

/// The workspace a column asks under: who works in another asks under theirs.
fn place_of(who: Who) -> &'static str {
    if who == Who::OtherWorkspace {
        ROOT_WS
    } else {
        TEST_WS
    }
}

/// The operations of what a course holds.
fn course_ops() -> Vec<Op> {
    use Body::{Json, None};
    vec![
        op("units: tree", Method::GET, "/courses/{c}/units", None),
        op("units: get", Method::GET, "/courses/{c}/units/{p}", None),
        op(
            "units: create",
            Method::POST,
            "/courses/{c}/units",
            Json(|_, _| json!({"title": "made"})),
        ),
        op(
            "units: patch",
            Method::PATCH,
            "/courses/{c}/units/{p}",
            Json(|_, _| json!({"title": "changed"})),
        ),
        op(
            "units: comments",
            Method::GET,
            "/courses/{c}/units/{p}/comments",
            None,
        ),
        op(
            "units: comment",
            Method::POST,
            "/courses/{c}/units/{p}/comments",
            Json(|_, _| json!({"body": said("said")})),
        ),
        op(
            "course: duplicate",
            Method::POST,
            "/containers/{c}/duplicate",
            Json(|_, _| json!({})),
        ),
        op(
            "course: export",
            Method::GET,
            "/containers/{c}/archive/export",
            None,
        ),
    ]
}

/// The table of `kind`: for each row, the cells in the order of [`COLUMNS`].
async fn table(people: &People, kind: Kind) -> Vec<(&'static str, Vec<String>)> {
    let own = match kind {
        Kind::Board | Kind::Template => board_ops(),
        Kind::Course => course_ops(),
        Kind::Wiki => {
            let mut ops = wiki_ops();
            ops.extend(page_ops());
            ops
        }
    };
    let mut categories = BTreeMap::new();
    for (name, who, _) in COLUMNS {
        categories.insert(name, category_of(people.client(who), place_of(who)).await);
    }
    let mut rows = Vec::new();
    for op in shared_ops().into_iter().chain(own) {
        let mut made: BTreeMap<Scope, Things> = BTreeMap::new();
        let mut cells = Vec::new();
        for (name, who, scope) in COLUMNS {
            if let std::collections::btree_map::Entry::Vacant(entry) = made.entry(scope) {
                let things = make(people, kind, scope).await;
                prepare(people, &op, &things).await;
                entry.insert(things);
            }
            let things = &made[&scope];
            let client = people.client(who);
            let (at, category) = (place_of(who), &categories[name]);
            let answer = ask(&op, client, at, things, people, category).await;
            // Who is not in the workspace is compared with a question about nothing.
            let of_nothing = if matches!(who, Who::OtherWorkspace | Who::OrgAdmin) {
                Some(ask(&op, client, at, &nothing(scope), people, category).await)
            } else {
                None
            };
            cells.push(cell(&op, &answer, things, of_nothing.as_ref()));
            if answer.status.is_success() {
                let given: Value = serde_json::from_slice(&answer.body).unwrap_or(Value::Null);
                if let Some(upload) = given["uploadId"].as_str() {
                    done(&client.delete(&format!("{at}/uploads/{upload}")).await);
                }
                if op.method != Method::GET {
                    made.remove(&scope);
                }
            }
        }
        rows.push((op.row, cells));
    }
    rows
}

/// The table as its file has it.
fn printed(kind: Kind, rows: &[(&str, Vec<String>)]) -> String {
    let mut out = format!(
        "# Status code by operation and person, on a {}. Columns: see role_table.rs.\n",
        match kind {
            Kind::Board => "board",
            Kind::Wiki => "wiki",
            Kind::Course => "course",
            Kind::Template => "template",
        }
    );
    let _ = write!(out, "{:<24}", "operation");
    for (name, _, _) in COLUMNS {
        let _ = write!(out, "{name:>12}");
    }
    out.push('\n');
    for (row, cells) in rows {
        let _ = write!(out, "{row:<24}");
        for cell in cells {
            let _ = write!(out, "{cell:>12}");
        }
        out.push('\n');
    }
    out
}

/// Compares the table with its file, or writes the file when asked to.
fn check_file(kind: Kind, actual: &str) {
    let name = match kind {
        Kind::Board => "board.txt",
        Kind::Wiki => "wiki.txt",
        Kind::Course | Kind::Template => unreachable!("only board and wiki have pinned files"),
    };
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/api/role_table")
        .join(name);
    if std::env::var("FVOCI_REGRESSION_UPDATE").as_deref() == Ok("1") {
        assert!(std::env::var_os("CI").is_none(), "no recording under CI");
        std::fs::create_dir_all(path.parent().expect("a directory")).expect("the directory");
        std::fs::write(&path, actual).expect("the table is written");
        return;
    }
    let pinned = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    if pinned != actual {
        let mut differences = String::new();
        for (want, got) in pinned.lines().zip(actual.lines()) {
            if want != got {
                let _ = writeln!(differences, "- {want}\n+ {got}");
            }
        }
        panic!("the table of {name} differs from its file:\n{differences}");
    }
}

/// The cells of one row of a table, by column.
fn row_of<'t>(rows: &'t [(&str, Vec<String>)], row: &str) -> &'t [String] {
    &rows
        .iter()
        .find(|(name, _)| *name == row)
        .unwrap_or_else(|| panic!("no row {row}"))
        .1
}

fn column(name: &str) -> usize {
    COLUMNS
        .iter()
        .position(|(header, _, _)| *header == name)
        .unwrap_or_else(|| panic!("no column {name}"))
}

/// The answers of both tables are their files, and a wiki answers as a board does
/// where both have the operation, but for who works in the workspace and has not
/// joined an open one: that one reads an open wiki, and writes in neither.
#[tokio::test]
async fn the_wiki_differs_from_the_board_only_where_an_open_wiki_is_read() {
    // PublicLink is on, as it is by default on a server: its links are among what is asked.
    let ts = Setup::local_mode().start().await.init_basic().await;
    let people = People::of(&ts).await;
    let board = table(&people, Kind::Board).await;
    let wiki = table(&people, Kind::Wiki).await;
    check_file(Kind::Board, &printed(Kind::Board, &board));
    check_file(Kind::Wiki, &printed(Kind::Wiki, &wiki));

    let unjoined = column(READS_UNJOINED);
    for op in shared_ops() {
        let (of_board, of_wiki) = (row_of(&board, op.row), row_of(&wiki, op.row));
        for (index, (name, _, _)) in COLUMNS.iter().enumerate() {
            assert_eq!(of_board[index], of_wiki[index], "{}: {name}", op.row);
        }
    }
    // Unjoined, an open wiki and an open board are both read as by their viewer, and
    // nothing of either is changed, duplicated, or exported until joined.
    let reads = [
        "container: get",
        "members: list",
        "teams: list",
        "files: get",
        "files: info",
        "file versions: list",
        "file versions: get",
        "references: to",
        "references: resolve",
        "pages: tree",
        "pages: get",
        "pages: comments",
        "pages: versions",
        "pages: comment versions",
        "pages: version",
    ];
    let viewer = column("viewer");
    for row in reads {
        let cells = row_of(&wiki, row);
        assert_eq!(cells[unjoined], cells[viewer], "{row}");
        assert!(
            cells[unjoined].starts_with('2'),
            "{row}: {}",
            cells[unjoined]
        );
    }
    assert_open_unjoined_board_rows(&board, unjoined, viewer);
    // Nor does who has not joined it write on its pages: a comment, a change or the
    // removal of the admin's, bringing that back, or a version of the body.
    for row in [
        "pages: comment",
        "pages: comment change",
        "pages: comment delete",
        "pages: comment restore",
        "pages: version restore",
        "pages: keep version",
    ] {
        assert_eq!(
            row_of(&wiki, row)[unjoined],
            "403",
            "an open wiki unjoined: {row}"
        );
    }
    // What is not a change of the wiki: joining it, leaving it (who reads it may leave
    // it, and leaving a container one is no member of changes nothing, 204), and what
    // the reader keeps of it in their own sidebar, stars and trash.
    let not_a_change = [
        "members: join",
        "members: leave",
        "sidebar: category",
        "sidebar: hide",
        "sidebar: unhide",
        "favorites: star it",
        "favorites: star a block",
        "trash: list",
    ];
    for (row, cells) in &wiki {
        if !reads.contains(row) && !not_a_change.contains(row) {
            assert!(
                !cells[unjoined].starts_with('2'),
                "{row}: {}",
                cells[unjoined]
            );
        }
    }
    // Who is not in the workspace finds nothing there.
    for (name, _, _) in COLUMNS
        .iter()
        .filter(|(name, _, _)| ["org-admin", "other-ws"].contains(name))
    {
        let index = column(name);
        for rows in [&board, &wiki] {
            for (row, cells) in rows {
                // What lists of their own answer has nothing of it (`-`); anything else
                // is to the letter what is answered about nothing.
                let cell = cells[index].as_str();
                assert!(cell == "404=" || cell == "200-", "{name}: {row}: {cell}");
            }
        }
    }
    ts.teardown().await;
}

fn assert_open_unjoined_board_rows(board: &[(&str, Vec<String>)], unjoined: usize, viewer: usize) {
    for row in [
        "members: list",
        "teams: list",
        "files: get",
        "references: to",
        "cards: list",
        "cards: get",
        "views: list",
        "views: get",
        "attachments: list",
        "cards: body versions",
    ] {
        let cells = row_of(board, row);
        assert_eq!(
            cells[unjoined], cells[viewer],
            "an open board unjoined reads as viewer: {row}"
        );
        assert!(
            cells[unjoined].starts_with('2'),
            "an open board unjoined: {row}: {}",
            cells[unjoined]
        );
    }
    for row in [
        "cards: create",
        "cards: patch",
        "comments: create",
        "cards: keep version",
        "cards: trash",
        "board: duplicate",
        "board: export",
        "container: Markdown export",
    ] {
        assert_eq!(
            row_of(board, row)[unjoined],
            "403",
            "an open board unjoined refuses write/copy/export: {row}"
        );
    }
}

/// Expected permission set for `(actor_key, role, scope)` in the pure permission matrix.
fn expected_permissions(
    actor_key: &str,
    role: Option<fvoci_model::ContainerRole>,
    scope: fvoci_server::permissions::ContainerScope,
) -> Vec<fvoci_server::permissions::ContainerPermission> {
    use fvoci_model::ContainerRole::{Admin, Commenter, Editor};
    use fvoci_server::permissions::ContainerPermission::{
        self, CommentCards, Delete, Join, ListMembers, ManageCards, ManageOthersComments,
        ManageProperties, ManageRoles, ManageType, See, Share, View,
    };
    use fvoci_server::permissions::ContainerScope::{
        BuiltIn, InvitedOnly, InvitedOnlyTemplate, OpenTemplate, OpenToRead,
    };

    if scope == BuiltIn {
        return if matches!(actor_key, "member" | "ws-admin") {
            vec![See, View, ListMembers]
        } else {
            Vec::new()
        };
    }
    if matches!(actor_key, "org-admin" | "stranger") {
        return Vec::new();
    }
    let effective_role = match (actor_key, role) {
        ("guest", Some(Admin)) => Some(Editor),
        (_, other) => other,
    };
    let mut set = Vec::new();
    if let Some(r) = effective_role {
        set.extend([See, View, ListMembers]);
        if r >= Commenter {
            set.push(CommentCards);
        }
        if r >= Editor {
            set.extend([ManageCards, ManageProperties]);
        }
        if r >= Admin {
            set.extend([ManageType, ManageRoles, Delete, Share, ManageOthersComments]);
        }
    }
    if matches!(actor_key, "member" | "ws-admin") {
        match scope {
            OpenToRead => set.extend([See, Join, View, ListMembers]),
            OpenTemplate => set.extend([See, View, ListMembers]),
            InvitedOnly | InvitedOnlyTemplate | BuiltIn => {}
        }
    }
    if actor_key == "ws-admin" {
        set.extend([See, ListMembers, ManageRoles, Delete]);
    }
    ContainerPermission::ALL
        .into_iter()
        .filter(|p| set.contains(p))
        .collect()
}

/// Tuple `(label, kind, is_template, is_builtin, visibility, expected_scope)` for matrix cases.
type MatrixContainer = (
    &'static str,
    fvoci_model::ContainerKind,
    bool,
    bool,
    fvoci_model::ContainerVisibility,
    fvoci_server::permissions::ContainerScope,
);

/// Every container shape exercised by the role × scope permission matrix.
fn matrix_containers() -> [MatrixContainer; 13] {
    use fvoci_model::ContainerKind::{Board, Course, Diary, Wiki};
    use fvoci_model::ContainerVisibility::{Open as VisOpen, Private};
    use fvoci_server::permissions::ContainerScope::{
        BuiltIn, InvitedOnly, InvitedOnlyTemplate, OpenTemplate, OpenToRead,
    };

    [
        ("board:private", Board, false, false, Private, InvitedOnly),
        ("board:open", Board, false, false, VisOpen, OpenToRead),
        ("wiki:private", Wiki, false, false, Private, InvitedOnly),
        ("wiki:open", Wiki, false, false, VisOpen, OpenToRead),
        ("course:private", Course, false, false, Private, InvitedOnly),
        ("course:open", Course, false, false, VisOpen, OpenToRead),
        ("diary:private", Diary, false, false, Private, InvitedOnly),
        ("diary:open", Diary, false, false, VisOpen, InvitedOnly),
        (
            "template:board:private",
            Board,
            true,
            false,
            Private,
            InvitedOnlyTemplate,
        ),
        (
            "template:board:open",
            Board,
            true,
            false,
            VisOpen,
            OpenTemplate,
        ),
        (
            "template:course:private",
            Course,
            true,
            false,
            Private,
            InvitedOnlyTemplate,
        ),
        (
            "template:course:open",
            Course,
            true,
            false,
            VisOpen,
            OpenTemplate,
        ),
        ("template:builtin", Board, true, true, VisOpen, BuiltIn),
    ]
}

/// Every workspace/organization actor shape exercised by the pure permission matrix.
fn matrix_actors() -> [(&'static str, fvoci_server::permissions::Actor); 5] {
    use fvoci_model::OrgRole;
    use fvoci_server::permissions::Actor;

    [
        (
            "member",
            Actor {
                org: Some(OrgRole::Member),
                workspace: Some(WorkspaceRole::Member),
            },
        ),
        (
            "guest",
            Actor {
                org: None,
                workspace: Some(WorkspaceRole::Guest),
            },
        ),
        (
            "ws-admin",
            Actor {
                org: Some(OrgRole::Member),
                workspace: Some(WorkspaceRole::Admin),
            },
        ),
        (
            "org-admin",
            Actor {
                org: Some(OrgRole::Admin),
                workspace: None,
            },
        ),
        (
            "stranger",
            Actor {
                org: None,
                workspace: None,
            },
        ),
    ]
}

/// Every role (`admin`, `editor`, `commenter`, `viewer`, `guest`, `unjoined`) across
/// every container kind (`board`, `wiki`, `course`, `diary`, `template`) yields the
/// exact permission set, collab document mode, management-only flag, and guest-invite
/// rule.
#[test]
fn role_and_scope_permission_matrix_matches_across_every_container_kind() {
    use fvoci_model::{Container, ContainerRole, Workspace};
    use fvoci_server::permissions::{
        ContainerFacts, ContainerPermission, ContainerScope, DocMode, doc_mode, grant,
        invites_guests,
    };

    let containers = matrix_containers();
    let actors = matrix_actors();
    let roles = [
        None,
        Some(ContainerRole::Viewer),
        Some(ContainerRole::Commenter),
        Some(ContainerRole::Editor),
        Some(ContainerRole::Admin),
    ];
    let team_roles = [
        None,
        Some(ContainerRole::Viewer),
        Some(ContainerRole::Commenter),
        Some(ContainerRole::Editor),
    ];
    let ws = Workspace {
        guest_invitations: true,
        ..Workspace::default()
    };

    for (label, kind, is_template, is_builtin, visibility, expected_scope) in containers {
        let container = Container {
            kind,
            is_template,
            is_builtin,
            visibility,
            ..Container::default()
        };
        let scope = ContainerScope::of(&container);
        assert_eq!(scope, expected_scope, "{label}");
        for (actor_key, actor) in actors {
            for role in roles {
                for team_role in team_roles {
                    let g = grant(
                        actor,
                        ContainerFacts {
                            scope,
                            role,
                            team_role,
                        },
                    );
                    let actual: Vec<ContainerPermission> = ContainerPermission::ALL
                        .into_iter()
                        .filter(|p| g.allows(*p))
                        .collect();
                    let effective = if matches!(actor_key, "member" | "ws-admin") {
                        role.max(team_role)
                    } else {
                        role
                    };
                    let want = expected_permissions(actor_key, effective, scope);
                    assert_eq!(
                        actual, want,
                        "{label} actor={actor_key} role={role:?} team={team_role:?}"
                    );
                    let want_doc = if actual.contains(&ContainerPermission::ManageCards) {
                        Some(DocMode::Write)
                    } else if actual.contains(&ContainerPermission::View) {
                        Some(DocMode::Read)
                    } else {
                        None
                    };
                    assert_eq!(doc_mode(g), want_doc, "{label} {actor_key} {role:?}");
                    let want_invites = !is_template
                        && !is_builtin
                        && actual.contains(&ContainerPermission::ManageRoles);
                    assert_eq!(
                        invites_guests(g, scope, &ws),
                        want_invites,
                        "{label} {actor_key} {role:?}"
                    );
                }
            }
        }
    }
}

/// Checks a diary in the owner's personal space and against every non-owner role in a
/// shared workspace.
async fn check_diary_role_matrix(ts: &TestServer, people: &People) {
    let workspaces: Vec<fvoci_model::UserWorkspace> =
        ts.client.get("/users/me/workspaces").await.ok_json();
    let own = &workspaces
        .iter()
        .find(|w| w.kind == fvoci_model::OrgKind::Personal)
        .expect("owner personal workspace")
        .id;
    let own_ws = format!("/w/{own}");
    let created = ts
        .client
        .post(&format!("{own_ws}/diaries"), &json!({"title": "Journal"}))
        .await;
    done(&created);
    let diary_id = text(&created.value(), "id");
    assert_eq!(
        ts.client
            .get(&format!("{own_ws}/containers/{diary_id}"))
            .await
            .status
            .as_u16(),
        200,
        "diary owner reads personal diary"
    );
    let entry = ts
        .client
        .post(
            &format!("{own_ws}/diaries/{diary_id}/entries"),
            &json!({"date": "2026-11-25"}),
        )
        .await;
    assert_eq!(entry.status.as_u16(), 201, "diary owner creates entry");
    let entry_id = text(&entry.value(), "id");
    // Diaries have no shared membership route: adding another member answers 404.
    assert_eq!(
        ts.client
            .post(
                &format!("{own_ws}/containers/{diary_id}/members"),
                &json!({"userId": people.viewer, "role": "viewer"}),
            )
            .await
            .status
            .as_u16(),
        404,
        "diary rejects shared membership route"
    );
    // Every other actor sees 404= under both the personal workspace and the shared workspace.
    for (_, who, _) in COLUMNS.iter().filter(|(_, who, _)| *who != Who::Admin) {
        let client = people.client(*who);
        for ws in [own_ws.as_str(), TEST_WS] {
            for (real, fake) in [
                (
                    format!("{ws}/containers/{diary_id}"),
                    format!("{ws}/containers/{UNKNOWN_ID}"),
                ),
                (
                    format!("{ws}/diaries/{diary_id}/entries/{entry_id}"),
                    format!("{ws}/diaries/{UNKNOWN_ID}/entries/{UNKNOWN_ID}"),
                ),
            ] {
                let (got, miss) = (client.get(&real).await, client.get(&fake).await);
                assert_eq!(got.status.as_u16(), 404, "{who:?} {real}");
                assert_eq!(got.problem(), miss.problem(), "{who:?} {real}");
            }
        }
    }
}

/// Live HTTP verification that `course`, `template`, and `diary` routes match the role ×
/// scope permission matrix across `admin`, `editor`, `commenter`, `viewer`, `guest`, and
/// `unjoined` callers.
#[tokio::test]
async fn course_template_and_diary_routes_match_the_role_matrix() {
    let ts = Setup::local_mode().start().await.init_basic().await;
    let people = People::of(&ts).await;
    let board = table(&people, Kind::Board).await;
    let wiki = table(&people, Kind::Wiki).await;
    let course = table(&people, Kind::Course).await;
    let template = table(&people, Kind::Template).await;

    // Every shared operation on a course matches a wiki cell-for-cell across all 11 columns.
    for op in shared_ops() {
        assert_eq!(
            row_of(&course, op.row),
            row_of(&wiki, op.row),
            "course vs wiki on {}",
            op.row
        );
    }
    for (course_row, wiki_row) in [
        ("units: tree", "pages: tree"),
        ("units: get", "pages: get"),
        ("units: create", "pages: create"),
        ("units: patch", "pages: patch"),
        ("units: comments", "pages: comments"),
        ("units: comment", "pages: comment"),
    ] {
        assert_eq!(
            row_of(&course, course_row),
            row_of(&wiki, wiki_row),
            "{course_row} vs {wiki_row}"
        );
    }
    let unjoined = column(READS_UNJOINED);
    assert_eq!(
        row_of(&course, "course: duplicate")[unjoined],
        "403",
        "unjoined member cannot duplicate open course"
    );
    assert_eq!(
        row_of(&course, "course: export")[unjoined],
        "403",
        "unjoined member cannot export open course"
    );

    // On a template, every column except `member@open` matches a board; on `member@open`
    // joining is refused (403) while reading, duplicating and exporting are allowed.
    for (row, of_template) in &template {
        // No star goes on a template, nor on what is in one (`app/favorites.rs`): it is as
        // nothing is, to every role.
        if row.starts_with("favorites:") {
            assert!(
                of_template.iter().all(|told| told.starts_with("404")),
                "{row} on a template: {of_template:?}"
            );
            continue;
        }
        let of_board = row_of(&board, row);
        for (idx, (col_name, _, _)) in COLUMNS.iter().enumerate() {
            if idx != unjoined {
                assert_eq!(
                    of_template[idx], of_board[idx],
                    "template vs board on {row}: {col_name}"
                );
            }
        }
    }
    assert_eq!(
        row_of(&template, "members: join")[unjoined],
        "403",
        "open template refuses join"
    );
    for read_row in [
        "container: get",
        "members: list",
        "teams: list",
        "files: get",
        "cards: list",
        "cards: get",
        "views: list",
        "views: get",
        "board: export",
        "container: Markdown export",
    ] {
        assert_eq!(
            row_of(&template, read_row)[unjoined],
            "200",
            "open template unjoined: {read_row}"
        );
    }
    assert_eq!(
        row_of(&template, "board: duplicate")[unjoined],
        "201",
        "open template unjoined duplicate"
    );
    for write_row in [
        "container: patch title",
        "cards: create",
        "comments: create",
    ] {
        assert_eq!(
            row_of(&template, write_row)[unjoined],
            "403",
            "open template unjoined write: {write_row}"
        );
    }

    check_diary_role_matrix(&ts, &people).await;
    ts.teardown().await;
}
