//! The permission table: the status code of every board-related route for every kind of
//! caller, on a private and on an open board.
//!
//! The same request is sent as six kinds of caller: anonymous, a signed-in user who is no
//! member of the board, and a viewer, commenter, editor and admin of it. Every request
//! gets a board of its own, so that what one caller changes cannot influence the next
//! answer.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use reqwest::Method;
use serde_json::{Value, json};

use crate::common::ROOT_WORKSPACE_ID;
use crate::fixtures::{PNG, block, id_of, merged, role_flags};
use crate::golden;
use crate::harness::ROOT_API;
use crate::harness::{Caller, Http, Payload, id_by_title, multipart, start_server};

/// The kinds of caller, in the order of the table's columns.
const ROLES: [&str; 6] = [
    "anonymous",
    "outsider",
    "viewer",
    "commenter",
    "editor",
    "admin",
];

/// The roles a fixture board gives a member each.
const MEMBER_ROLES: [&str; 4] = ["viewer", "commenter", "editor", "admin"];

/// The rows of the table, in order. [`request`] builds the request of each.
const ROUTES: [&str; 53] = [
    "board: get",
    "board: patch title",
    "board: patch properties",
    "board: patch visibility",
    "board: patch minimum role",
    "board: delete",
    "board: restore",
    "board: duplicate",
    "board: duplicate as template",
    "board: metadata",
    "members: list",
    "members: add viewer",
    "members: add admin",
    "members: update",
    "members: delete another",
    "members: join",
    "members: leave",
    "blocks: list",
    "blocks: list all",
    "blocks: list by parent",
    "blocks: insert card",
    "blocks: insert view",
    "blocks: insert comment",
    "blocks: patch card",
    "blocks: patch another's comment",
    "blocks: patch several",
    "blocks: delete card",
    "blocks: delete view",
    "blocks: delete another's comment",
    "blocks: restore",
    "blocks: duplicate",
    "containers-and-blocks: patch",
    "containers-and-blocks: delete",
    "cards: create",
    "cards: list",
    "cards: get",
    "cards: patch",
    "public-link: get",
    "public-link: post",
    "subscriptions: create",
    "subscriptions: delete",
    "files: upload",
    "files: download",
    "files: info",
    "uploads: open",
    "archive: export board",
    "archive: open an upload",
    "archive: export workspace",
    "categories: list",
    "workspace: boards",
    "workspace: templates",
    "workspace: search",
    "workspace: users",
];

/// The users of a permission run: the board owner, one user per role, and `extra`, whom
/// the member routes add.
#[derive(Debug)]
struct Users {
    /// User identifiers by name.
    ids: HashMap<&'static str, String>,
    /// Session tokens by name.
    tokens: HashMap<&'static str, String>,
}

impl Users {
    async fn register(http: &Http) -> Self {
        let mut tokens = HashMap::new();
        tokens.insert("owner", http.register("owner", "").await);
        let link = http.member_link(&tokens["owner"]).await;
        for name in [
            "outsider",
            "viewer",
            "commenter",
            "editor",
            "admin",
            "extra",
        ] {
            tokens.insert(name, http.register(name, &link).await);
        }
        let mut ids = HashMap::new();
        for (name, token) in &tokens {
            ids.insert(*name, http.user_id(token).await);
        }
        Self { ids, tokens }
    }

    fn caller(&self, name: &str) -> Caller {
        Caller::Token(self.tokens[name].clone())
    }
}

/// A board with one member per role, some blocks, and on demand a file.
#[derive(Debug)]
struct Fixture<'a> {
    http: &'a Http,
    users: &'a Users,
    visibility: String,
    board: String,
    card: String,
    view: String,
    comment: String,
}

impl<'a> Fixture<'a> {
    async fn create(http: &'a Http, users: &'a Users, board_type: &str) -> Self {
        let owner = users.caller("owner");
        let board = http
            .api(
                Method::POST,
                &format!("{ROOT_API}/boards"),
                Payload::Json(json!({
                    "workspaceId": ROOT_WORKSPACE_ID, "visibility": board_type, "title": "Matrix",
                    "properties": {}, "cardProperties": [],
                })),
                &owner,
            )
            .await
            .string("id");
        for role in MEMBER_ROLES {
            let member = merged(
                &json!({"containerId": board, "userId": users.ids[role]}),
                role_flags(role),
            );
            let added = http
                .api(
                    Method::POST,
                    &format!("{ROOT_API}/containers/{board}/members"),
                    Payload::Json(member),
                    &owner,
                )
                .await;
            assert_eq!(added.status, 200, "adding the {role} to the board");
        }
        let card = http
            .api(
                Method::POST,
                &format!("{ROOT_API}/boards/{board}/cards"),
                Payload::Json(json!({"title": "card"})),
                &owner,
            )
            .await
            .string("id");
        let blocks = format!("{ROOT_API}/boards/{board}/blocks");
        let created = http
            .api(
                Method::POST,
                &blocks,
                Payload::Json(json!([block(
                    "view",
                    &board,
                    "view",
                    "view",
                    "",
                    json!({"viewType": "board"})
                )])),
                &owner,
            )
            .await;
        let created = created.json();
        let comment = http
            .api(
                Method::POST,
                &blocks,
                Payload::Json(json!([block(
                    "comment",
                    &board,
                    "comment",
                    "comment",
                    &card,
                    json!({})
                )])),
                &owner,
            )
            .await;
        Self {
            http,
            users,
            visibility: board_type.to_owned(),
            view: id_by_title(created, "view"),
            comment: id_of(&comment.json()[0]),
            card,
            board,
        }
    }

    /// A request as the board's owner, to arrange what the route under test needs. It
    /// must succeed: a cell recorded after a failed arrangement would pin a status that
    /// has another reason than the one the table is about.
    async fn owner(&self, method: Method, path: &str, payload: Payload) -> crate::harness::Reply {
        let reply = self
            .http
            .api(method.clone(), path, payload, &self.users.caller("owner"))
            .await;
        assert!(
            (200..300).contains(&reply.status),
            "arranging with {method} {path} as the owner answered {}",
            reply.status
        );
        reply
    }

    /// Uploads a file to the board as its owner and returns the file's name.
    async fn upload_file(&self) -> String {
        self.owner(
            Method::POST,
            &format!("{ROOT_API}/containers/{}/files", self.board),
            multipart("file", "pixel.png", &PNG, "image/png"),
        )
        .await
        .string("fileId")
    }
}

/// The request of one row of the table.
#[derive(Debug)]
struct Request {
    method: Method,
    path: String,
    payload: Payload,
}

impl Request {
    fn new(method: Method, path: String) -> Self {
        Self {
            method,
            path,
            payload: Payload::None,
        }
    }

    fn json(method: Method, path: String, body: Value) -> Self {
        Self {
            method,
            path,
            payload: Payload::Json(body),
        }
    }
}

/// Prepares the fixture for a route and returns the request to send. `me` is the
/// caller's user identifier, empty for the anonymous caller. The routes go by the
/// family their row names before its colon.
async fn request(route: &str, fx: &Fixture<'_>, me: &str) -> Request {
    match route.split_once(": ").map(|(family, _)| family) {
        Some("board" | "members") => board_request(route, fx).await,
        Some("blocks" | "containers-and-blocks" | "cards") => block_request(route, fx).await,
        _ => workspace_request(route, fx, me).await,
    }
}

/// The request of a row of the board and of its members.
async fn board_request(route: &str, fx: &Fixture<'_>) -> Request {
    let board = &fx.board;
    let board_path = format!("{ROOT_API}/containers/{board}");
    let viewer = &fx.users.ids["viewer"];
    let extra = &fx.users.ids["extra"];
    match route {
        "board: get" => Request::new(Method::GET, board_path),
        "board: patch title" => {
            Request::json(Method::PATCH, board_path, json!({"title": "renamed"}))
        }
        "board: patch properties" => Request::json(
            Method::PATCH,
            board_path,
            json!({"updatedProperties": {"a": 1}}),
        ),
        "board: patch visibility" => {
            Request::json(Method::PATCH, board_path, json!({"visibility": "open"}))
        }
        "board: patch minimum role" => {
            Request::json(Method::PATCH, board_path, json!({"minimumRole": "editor"}))
        }
        "board: delete" => Request::new(Method::DELETE, board_path),
        "board: restore" => {
            fx.owner(Method::DELETE, &board_path, Payload::None).await;
            Request::new(Method::POST, format!("{board_path}/restore"))
        }
        "board: duplicate" => Request::new(Method::POST, format!("{board_path}/duplicate")),
        "board: duplicate as template" => Request::new(
            Method::POST,
            format!("{board_path}/duplicate?asTemplate=true"),
        ),
        "board: metadata" => Request::new(Method::GET, format!("{board_path}/metadata")),
        "members: list" => Request::new(Method::GET, format!("{board_path}/members")),
        "members: add viewer" => Request::json(
            Method::POST,
            format!("{board_path}/members"),
            json!({"containerId": board, "userId": extra, "role": "viewer"}),
        ),
        "members: add admin" => Request::json(
            Method::POST,
            format!("{board_path}/members"),
            json!({"containerId": board, "userId": extra, "role": "admin"}),
        ),
        "members: update" => Request::json(
            Method::PUT,
            format!("{board_path}/members/{viewer}"),
            json!({"containerId": board, "userId": viewer, "role": "editor"}),
        ),
        "members: delete another" => {
            Request::new(Method::DELETE, format!("{board_path}/members/{viewer}"))
        }
        "members: join" => Request::new(Method::POST, format!("{board_path}/join")),
        "members: leave" => Request::new(Method::POST, format!("{board_path}/leave")),
        other => panic!("no request is defined for the route {other:?}"),
    }
}

/// The request of a row of the blocks and the cards of the board.
async fn block_request(route: &str, fx: &Fixture<'_>) -> Request {
    let board = &fx.board;
    let blocks = format!("{ROOT_API}/boards/{board}/blocks");
    match route {
        "blocks: list" => Request::new(Method::GET, blocks),
        "blocks: list all" => Request::new(Method::GET, format!("{blocks}?all=true")),
        "blocks: list by parent" => {
            Request::new(Method::GET, format!("{blocks}?parentId={}", fx.card))
        }
        "blocks: insert card" => Request::json(
            Method::POST,
            blocks,
            json!([block("n", board, "card", "n", "", json!({}))]),
        ),
        "blocks: insert view" => Request::json(
            Method::POST,
            blocks,
            json!([block("n", board, "view", "n", "", json!({}))]),
        ),
        "blocks: insert comment" => Request::json(
            Method::POST,
            blocks,
            json!([block("n", board, "comment", "n", &fx.card, json!({}))]),
        ),
        "blocks: patch card" => Request::json(
            Method::PATCH,
            format!("{blocks}/{}", fx.card),
            json!({"title": "patched"}),
        ),
        "blocks: patch another's comment" => Request::json(
            Method::PATCH,
            format!("{blocks}/{}", fx.comment),
            json!({"title": "patched"}),
        ),
        "blocks: patch several" => Request::json(
            Method::PATCH,
            blocks,
            json!({"blockIds": [fx.card], "blockPatches": [{"title": "p"}]}),
        ),
        "blocks: delete card" => Request::new(Method::DELETE, format!("{blocks}/{}", fx.card)),
        "blocks: delete view" => Request::new(Method::DELETE, format!("{blocks}/{}", fx.view)),
        "blocks: delete another's comment" => {
            Request::new(Method::DELETE, format!("{blocks}/{}", fx.comment))
        }
        "blocks: restore" => {
            let view = format!("{blocks}/{}", fx.view);
            fx.owner(Method::DELETE, &view, Payload::None).await;
            Request::new(Method::POST, format!("{view}/restore"))
        }
        "blocks: duplicate" => {
            Request::new(Method::POST, format!("{blocks}/{}/duplicate", fx.card))
        }
        "containers-and-blocks: patch" => Request::json(
            Method::PATCH,
            format!("{ROOT_API}/containers-and-blocks"),
            json!({
                "containerIds": [board], "containerPatches": [{"title": "p"}],
                "blockIds": [fx.card], "blockPatches": [{"title": "p"}],
            }),
        ),
        "containers-and-blocks: delete" => Request::json(
            Method::DELETE,
            format!("{ROOT_API}/containers-and-blocks"),
            json!({"containers": [board], "blocks": [fx.card]}),
        ),
        "cards: create" => Request::json(
            Method::POST,
            format!("{ROOT_API}/boards/{board}/cards"),
            json!({"title": "new"}),
        ),
        "cards: list" => Request::new(Method::GET, format!("{ROOT_API}/boards/{board}/cards")),
        "cards: get" => Request::new(Method::GET, format!("{ROOT_API}/cards/{}", fx.card)),
        "cards: patch" => Request::json(
            Method::PATCH,
            format!("{ROOT_API}/cards/{}", fx.card),
            json!({"title": "patched"}),
        ),
        other => panic!("no request is defined for the route {other:?}"),
    }
}

/// The request of a row of what else the board has (its public link, subscriptions,
/// files, uploads and archive) and of its workspace.
async fn workspace_request(route: &str, fx: &Fixture<'_>, me: &str) -> Request {
    let board_path = format!("{ROOT_API}/containers/{}", fx.board);
    let workspace = ROOT_API.to_owned();
    match route {
        "public-link: get" => {
            fx.owner(
                Method::POST,
                &format!("{board_path}/public-link"),
                Payload::Json(json!({"enabled": true})),
            )
            .await;
            Request::new(Method::GET, format!("{board_path}/public-link"))
        }
        "public-link: post" => Request::json(
            Method::POST,
            format!("{board_path}/public-link"),
            json!({"enabled": true}),
        ),
        "subscriptions: create" => Request::json(
            Method::POST,
            format!("{ROOT_API}/subscriptions"),
            subscription(fx, me),
        ),
        "subscriptions: delete" => {
            let subscriber = subscribe_before_delete(fx, me).await;
            Request::new(
                Method::DELETE,
                format!("{ROOT_API}/subscriptions/{}/{subscriber}", fx.card),
            )
        }
        "files: upload" => Request {
            method: Method::POST,
            path: format!("{board_path}/files"),
            payload: multipart("file", "pixel.png", &PNG, "image/png"),
        },
        "files: download" => {
            let file = fx.upload_file().await;
            Request::new(Method::GET, format!("{board_path}/files/{file}"))
        }
        "files: info" => {
            let file = fx.upload_file().await;
            Request::new(Method::GET, format!("{board_path}/files/{file}/info"))
        }
        "uploads: open" => Request::json(
            Method::POST,
            format!("{board_path}/uploads"),
            json!({"name": "pixel.png", "size": PNG.len()}),
        ),
        "archive: export board" => {
            Request::new(Method::GET, format!("{board_path}/archive/export"))
        }
        "archive: open an upload" => Request::json(
            Method::POST,
            format!("{workspace}/archive/uploads"),
            json!({"name": "a.fvoci", "size": 100}),
        ),
        "archive: export workspace" => {
            Request::new(Method::GET, format!("{workspace}/archive/export"))
        }
        "categories: list" => Request::new(Method::GET, format!("{workspace}/categories")),
        "workspace: boards" => Request::new(Method::GET, format!("{workspace}/boards")),
        "workspace: templates" => Request::new(Method::GET, format!("{workspace}/templates")),
        "workspace: search" => {
            Request::new(Method::GET, format!("{workspace}/boards/search?q=Matrix"))
        }
        "workspace: users" => Request::new(Method::GET, format!("{workspace}/users")),
        other => panic!("no request is defined for the route {other:?}"),
    }
}

async fn subscribe_before_delete<'a>(fx: &Fixture<'_>, me: &'a str) -> &'a str {
    if me.is_empty() {
        return "nobody";
    }
    let (name, _) = fx
        .users
        .ids
        .iter()
        .find(|(_, id)| id.as_str() == me)
        .expect("the caller is a registered user");
    let token = fx.users.caller(name);
    let reply = fx
        .http
        .api(
            Method::POST,
            &format!("{ROOT_API}/subscriptions"),
            Payload::Json(subscription(fx, me)),
            &token,
        )
        .await;
    // A member has a subscription to end. Who is no member cannot subscribe,
    // and their cell says what ending a subscription one does not have answers.
    let wanted = if MEMBER_ROLES.contains(name) || fx.visibility == "open" {
        201
    } else {
        404
    };
    assert_eq!(
        reply.status, wanted,
        "subscribing as {name} before the subscription is ended"
    );
    me
}

fn subscription(fx: &Fixture<'_>, user: &str) -> Value {
    json!({
        "blockType": "card", "blockId": fx.card, "userId": user,
    })
}

/// The status codes of one route, in the order of [`ROLES`].
type Row = [u16; ROLES.len()];

/// Records the table for boards of one visibility (`private` or `open`) on a server of
/// its own and compares it with `golden/<name>.txt`.
pub(crate) async fn record(name: &str, visibility: &str) {
    let server = start_server().await;
    let http = Http::new(&server);
    let users = Users::register(&http).await;

    let mut rows: Vec<(&str, Row)> = Vec::with_capacity(ROUTES.len());
    for route in ROUTES {
        let mut row = [0; ROLES.len()];
        for (cell, role) in row.iter_mut().zip(ROLES) {
            let fx = Fixture::create(&http, &users, visibility).await;
            let (me, caller) = if role == "anonymous" {
                ("", Caller::Anonymous)
            } else {
                (users.ids[role].as_str(), users.caller(role))
            };
            let request = request(route, &fx, me).await;
            *cell = http
                .api(request.method, &request.path, request.payload, &caller)
                .await
                .status;
        }
        rows.push((route, row));
    }
    server.teardown().await;

    golden::check_table(name, &render(visibility, &rows), |expected| {
        differences(expected, &rows)
    });
}

fn render(visibility: &str, rows: &[(&str, Row)]) -> String {
    let kind = if visibility == "private" {
        "private"
    } else {
        "open"
    };
    let mut table = format!("# Status code by route and caller, on a board of the {kind} type.\n");
    let _ = write!(table, "{:36}", "route");
    for role in ROLES {
        let _ = write!(table, "{role:>11}");
    }
    table.push('\n');
    for (route, row) in rows {
        let _ = write!(table, "{route:36}");
        for status in row {
            let _ = write!(table, "{status:>11}");
        }
        table.push('\n');
    }
    table
}

/// Reads the rows of a rendered table: the route, then one status code per role.
fn parse(table: &str) -> BTreeMap<String, Vec<String>> {
    table
        .lines()
        .skip(2)
        .filter_map(|line| {
            let mut cells: Vec<&str> = line.split_whitespace().collect();
            if cells.len() <= ROLES.len() {
                return None;
            }
            let codes = cells.split_off(cells.len() - ROLES.len());
            Some((
                cells.join(" "),
                codes.into_iter().map(str::to_owned).collect(),
            ))
        })
        .collect()
}

fn differences(expected: &str, rows: &[(&str, Row)]) -> String {
    let mut expected = parse(expected);
    let mut report = String::new();
    for (route, row) in rows {
        let Some(want) = expected.remove(*route) else {
            let _ = writeln!(report, "  {route}: not in the golden file");
            continue;
        };
        for ((role, want), got) in ROLES.iter().zip(&want).zip(row) {
            if *want != got.to_string() {
                let _ = writeln!(report, "  {route}: {role}: expected {want}, got {got}");
            }
        }
    }
    for route in expected.keys() {
        let _ = writeln!(report, "  {route}: only in the golden file");
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rendered_table_reads_back_and_shows_changed_cells() {
        let rows = [
            ("board: get", [401, 403, 200, 200, 200, 200]),
            (
                "blocks: delete another's comment",
                [401, 403, 403, 403, 403, 200],
            ),
        ];
        let table = render("private", &rows);
        assert_eq!(differences(&table, &rows), "", "a table equals itself");

        let mut changed = rows;
        changed[1].1[4] = 200;
        assert_eq!(
            differences(&table, &changed),
            "  blocks: delete another's comment: editor: expected 403, got 200\n",
            "the changed cell is named by route and role"
        );
    }
}
