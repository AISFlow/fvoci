//! The tools that read: each enters its workspace as the route it stands for does and
//! answers with what that route answers, cut to what an agent works by.

use fvoci_model::{
    ApiTokenScope, Block, Body, COMMENT_AGENT_TOKEN_NAME, Card, CompactCard, ContainerKind,
    ContainerKinds, ListedFile, OrgKind, Page, PageNode, Unit, UnitKind, UnitNode, User,
    UserWorkspace, WorkspaceRole,
};
use serde::Serialize;
use serde_json::{Value, json};

use super::arguments::{
    CardOrder, ContentKind, FindPeopleArguments, GetItemArguments, GetOutlineArguments, ItemKind,
    ListActivityArguments, ListCardsArguments, ListCommentsArguments, ListFilesArguments,
    SearchArguments, WorkspaceArguments,
};
use super::tools::{ToolError, enter};
use crate::api::card_activity::card_activity_of;
use crate::api::cards::{
    BodyFormatValue, CardsListQuery, card_by_number_read, card_read, cards_of_board,
};
use crate::api::comments::comments_of;
use crate::api::content_search::{ContentSearchQuery, search_content};
use crate::api::courses::{READ_DENIED as UNIT_READ_DENIED, unit_tree_read, unit_with_permission};
use crate::api::items::{CardItem, ItemPath, PageItem, UnitItem};
use crate::api::lists::{files_of_card, files_of_container};
use crate::api::overview::overview_of;
use crate::api::query::PagingQuery;
use crate::api::unit_progress::my_progress_in;
use crate::api::wikis::{READ_DENIED as PAGE_READ_DENIED, page_tree_read, page_with_permission};
use crate::api::workspace::InWorkspace;
use crate::app::{App, BodyInMarkdown, Caller};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// How many of the cards below a card `get_item` gives.
const CHILDREN_GIVEN: usize = 50;

/// A workspace as `list_workspaces` answers with it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListedWorkspace {
    id: String,
    org_id: String,
    kind: OrgKind,
    name: String,
    role: WorkspaceRole,
}

impl From<UserWorkspace> for ListedWorkspace {
    fn from(workspace: UserWorkspace) -> Self {
        Self {
            id: workspace.id,
            org_id: workspace.org_id,
            kind: workspace.kind,
            name: workspace.name,
            role: workspace.role,
        }
    }
}

/// A person as an agent is told of them: never their email.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Person {
    id: String,
    username: String,
    name: String,
}

impl From<User> for Person {
    fn from(user: User) -> Self {
        let name = if user.display_name.is_empty() {
            user.username.clone()
        } else {
            user.display_name
        };
        Self {
            id: user.id,
            username: user.username,
            name,
        }
    }
}

/// A comment as `list_comments` answers with it: its text in Markdown.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ListedComment {
    id: String,
    created_by: String,
    created_at: i64,
    updated_at: i64,
    edited_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_token_name: Option<String>,
    markdown: String,
}

/// `value` as JSON.
///
/// # Errors
///
/// Returns a failure of the server if it cannot be written as JSON.
pub(super) fn to_json(value: &impl Serialize) -> Result<Value, AppError> {
    serde_json::to_value(value)
        .map_err(|error| AppError::internal(format_args!("an answer as JSON: {error}")))
}

/// Text of an optional argument as a query takes it: empty for none.
fn text(value: Option<impl ToString>) -> String {
    value.map_or_default(|value| value.to_string())
}

/// The page a route counts from 0 of one a tool counts from 1, as people do.
fn from_one(page: Option<u32>) -> Result<Option<u32>, ToolError> {
    page.map(|page| {
        page.checked_sub(1)
            .ok_or_else(|| ToolError::arguments("page counts from 1"))
    })
    .transpose()
}

pub(super) async fn list_workspaces(app: &App, caller: &Caller) -> Result<Value, ToolError> {
    caller.require_scope(ApiTokenScope::Read)?;
    let workspaces: Vec<ListedWorkspace> = app
        .workspaces_let_in(caller.asker())
        .await?
        .into_iter()
        .map(ListedWorkspace::from)
        .collect();
    Ok(json!({ "workspaces": workspaces }))
}

pub(super) async fn overview(
    app: &App,
    caller: &Caller,
    arguments: WorkspaceArguments,
) -> Result<Value, ToolError> {
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Read,
        ContainerKinds::EVERY,
    )
    .await?;
    Ok(to_json(&overview_of(&ws).await?)?)
}

pub(super) async fn find_people(
    app: &App,
    caller: &Caller,
    arguments: FindPeopleArguments,
) -> Result<Value, ToolError> {
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Read,
        ContainerKinds::BOARDS,
    )
    .await?;
    let people: Vec<Person> = ws
        .app
        .search_users_in_workspace(
            &ws.caller.user_id,
            &ws.workspace_id,
            ws.role,
            arguments.q.as_deref().unwrap_or_default(),
        )
        .await?
        .into_iter()
        .map(Person::from)
        .collect();
    Ok(json!({ "people": people }))
}

pub(super) async fn search(
    app: &App,
    caller: &Caller,
    arguments: SearchArguments,
) -> Result<Value, ToolError> {
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Read,
        ContainerKinds::EVERY,
    )
    .await?;
    let query = ContentSearchQuery {
        q: arguments.q,
        kind: text(arguments.kind.map(ContentKind::as_str)),
        limit: text(arguments.limit),
    };
    let hits = search_content(&ws, &query, arguments.container_id.as_deref()).await?;
    Ok(json!({ "hits": hits }))
}

pub(super) async fn list_cards(
    app: &App,
    caller: &Caller,
    arguments: ListCardsArguments,
) -> Result<Value, ToolError> {
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Read,
        ContainerKinds::BOARDS,
    )
    .await?;
    let query = CardsListQuery {
        page: text(from_one(arguments.page)?),
        per_page: text(arguments.per_page),
        q: text(arguments.q),
        parent_id: text(arguments.parent_id),
        assignee_id: text(arguments.assignee_id),
        property_id: text(arguments.property_id),
        property_value: text(arguments.property_value),
        updated_since: text(arguments.updated_since),
        sort: text(arguments.sort.map(CardOrder::as_str)),
        ..CardsListQuery::default()
    };
    let cards: Vec<CompactCard> = cards_of_board(&ws, &arguments.board_id, &query)
        .await?
        .into_iter()
        .map(CompactCard::from)
        .collect();
    Ok(json!({ "cards": cards }))
}

/// `item`, a card, page or unit as JSON, with its body in Markdown in place of it.
fn in_markdown(mut item: Value, written: BodyInMarkdown) -> Result<Value, AppError> {
    if let Value::Object(members) = &mut item {
        members.remove("body");
        members.insert("bodyMarkdown".to_owned(), Value::String(written.markdown));
        members.insert(
            "hasComplexBlocks".to_owned(),
            Value::Bool(!written.complex_blocks.is_empty()),
        );
        members.insert(
            "complexBlocks".to_owned(),
            to_json(&written.complex_blocks)?,
        );
    }
    Ok(item)
}

/// `item` of the container `container_id` as JSON, with `body` in Markdown.
async fn written(
    ws: &InWorkspace,
    container_id: &str,
    item: &impl Serialize,
    body: Option<&Body>,
) -> Result<Value, AppError> {
    let markdown = ws
        .app
        .body_in_markdown(ws.markdown_reader(), container_id, body)
        .await?;
    in_markdown(to_json(item)?, markdown)
}

/// A card as JSON, its body in Markdown (`getCard` with `format=markdown`).
fn card_json(card: &Card) -> Result<Value, AppError> {
    let mut item = to_json(card)?;
    if let Value::Object(members) = &mut item {
        members.remove("body");
    }
    Ok(item)
}

/// The card `card_id` as `getCard` with `format=markdown` answers with it, without the
/// body it reads into Markdown.
///
/// # Errors
///
/// As `getCard` answers.
pub(super) async fn card_in_markdown(ws: &InWorkspace, card_id: &str) -> Result<Value, AppError> {
    card_json(&card_read(ws, card_id, BodyFormatValue::Markdown).await?)
}

/// The page `page_id` of the wiki `wiki_id`, as `getPage` reads it, without its body,
/// and the body.
async fn page_read(
    ws: &InWorkspace,
    wiki_id: &str,
    page_id: &str,
) -> Result<(Page, Option<Body>), AppError> {
    let block = page_with_permission(
        ws,
        wiki_id,
        page_id,
        ContainerPermission::View,
        PAGE_READ_DENIED,
    )
    .await?;
    let mut page = Page::from(block);
    let body = page.body.take();
    Ok((page, body))
}

/// The page `page_id` of the wiki `wiki_id` as JSON, its body in Markdown.
///
/// # Errors
///
/// As `getPage` answers.
pub(super) async fn page_in_markdown(
    ws: &InWorkspace,
    wiki_id: &str,
    page_id: &str,
) -> Result<Value, AppError> {
    let (page, body) = page_read(ws, wiki_id, page_id).await?;
    written(ws, wiki_id, &page, body.as_ref()).await
}

/// The unit `unit_id` of the course `course_id`, as `getUnit` reads it, without its
/// body, and the body.
async fn unit_read(
    ws: &InWorkspace,
    course_id: &str,
    unit_id: &str,
) -> Result<(Unit, Option<Body>), AppError> {
    let block = unit_with_permission(
        ws,
        course_id,
        unit_id,
        ContainerPermission::View,
        UNIT_READ_DENIED,
    )
    .await?;
    let mut unit = Unit::from(block);
    let body = unit.body.take();
    Ok((unit, body))
}

/// The unit `unit_id` of the course `course_id` as JSON, its body in Markdown.
///
/// # Errors
///
/// As `getUnit` answers.
pub(super) async fn unit_in_markdown(
    ws: &InWorkspace,
    course_id: &str,
    unit_id: &str,
) -> Result<Value, AppError> {
    let (unit, body) = unit_read(ws, course_id, unit_id).await?;
    written(ws, course_id, &unit, body.as_ref()).await
}

/// The files of the container `container_id` that `body` names, oldest first.
async fn files_of_body(
    app: &App,
    container_id: &str,
    body: Option<&Body>,
) -> Result<Vec<ListedFile>, AppError> {
    let mut files = Vec::new();
    for file_id in body.map_or_default(Body::file_ids) {
        if let Some(record) = app.file_record(container_id, file_id).await? {
            files.push(ListedFile::from(record));
        }
    }
    files.sort_by(|a, b| (a.created_at, &a.file_id).cmp(&(b.created_at, &b.file_id)));
    Ok(files)
}

/// The item of a page or unit: what it is in, and its identifier.
fn page_or_unit_named(arguments: &GetItemArguments) -> Result<(&str, &str), ToolError> {
    match (&arguments.container_id, &arguments.id, arguments.number) {
        (Some(container_id), Some(id), None) => Ok((container_id, id)),
        _ => Err(ToolError::arguments(
            "a page or unit is named by containerId and id, and has no number",
        )),
    }
}

pub(super) async fn get_item(
    app: &App,
    caller: &Caller,
    arguments: GetItemArguments,
) -> Result<Value, ToolError> {
    let kinds = match arguments.kind {
        ItemKind::Card => ContainerKinds::BOARDS,
        ItemKind::Page => ContainerKinds::WIKIS,
        ItemKind::Unit => ContainerKinds::COURSES,
    };
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Read,
        kinds,
    )
    .await?;
    match arguments.kind {
        ItemKind::Card => card_item(&ws, &arguments).await,
        ItemKind::Page => {
            let (wiki_id, page_id) = page_or_unit_named(&arguments)?;
            let (page, body) = page_read(&ws, wiki_id, page_id).await?;
            let files = files_of_body(&ws.app, wiki_id, body.as_ref()).await?;
            let children: Vec<PageNode> = page_tree_read(&ws, wiki_id)
                .await?
                .into_iter()
                .filter(|node| node.parent_id == page.id)
                .collect();
            Ok(json!({
                "kind": "page",
                "item": written(&ws, wiki_id, &page, body.as_ref()).await?,
                "children": children,
                "files": files,
            }))
        }
        ItemKind::Unit => {
            let (course_id, unit_id) = page_or_unit_named(&arguments)?;
            let (unit, body) = unit_read(&ws, course_id, unit_id).await?;
            let files = files_of_body(&ws.app, course_id, body.as_ref()).await?;
            let children: Vec<UnitNode> = if unit.kind == UnitKind::Chapter {
                unit_tree_read(&ws, course_id)
                    .await?
                    .into_iter()
                    .filter(|node| node.parent_id == unit.id)
                    .collect()
            } else {
                Vec::new()
            };
            Ok(json!({
                "kind": "unit",
                "item": written(&ws, course_id, &unit, body.as_ref()).await?,
                "children": children,
                "files": files,
            }))
        }
    }
}

/// A card of `get_item`: by its identifier, or by its board and number.
async fn card_item(ws: &InWorkspace, arguments: &GetItemArguments) -> Result<Value, ToolError> {
    let card = match (&arguments.id, &arguments.container_id, arguments.number) {
        (Some(card_id), board_id, None) => {
            let card = card_read(ws, card_id, BodyFormatValue::Markdown).await?;
            if board_id
                .as_ref()
                .is_some_and(|board_id| *board_id != card.board_id)
            {
                return Err(AppError::not_found("card").into());
            }
            card
        }
        (None, Some(board_id), Some(number)) => {
            let mut card = card_by_number_read(ws, board_id, &number.to_string()).await?;
            ws.app
                .put_card_in_markdown(ws.markdown_reader(), &mut card)
                .await?;
            card
        }
        _ => {
            return Err(ToolError::arguments(
                "a card is named by id, or by containerId and number",
            ));
        }
    };
    // One more than are given, to tell whether there are more.
    let below = CardsListQuery {
        parent_id: card.id.clone(),
        per_page: (CHILDREN_GIVEN + 1).to_string(),
        ..CardsListQuery::default()
    };
    let mut children: Vec<CompactCard> = cards_of_board(ws, &card.board_id, &below)
        .await?
        .into_iter()
        .map(CompactCard::from)
        .collect();
    let has_more = children.len() > CHILDREN_GIVEN;
    children.truncate(CHILDREN_GIVEN);
    let files = files_of_card(ws, &card.board_id, &card.id).await?;
    Ok(json!({
        "kind": "card",
        "item": card_json(&card)?,
        "children": children,
        "hasMoreChildren": has_more,
        "files": files,
    }))
}

pub(super) async fn get_outline(
    app: &App,
    caller: &Caller,
    arguments: GetOutlineArguments,
) -> Result<Value, ToolError> {
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Read,
        ContainerKinds::WIKIS.union(ContainerKinds::COURSES),
    )
    .await?;
    let container_id = arguments.container_id.as_str();
    // Which it is is told only of one the caller reads: of any other, the answer is
    // that there is none, as the routes answer.
    let kind = ws
        .app
        .container_access(&ws.caller.user_id, container_id)
        .await?
        .filter(|access| access.grant.allows(ContainerPermission::View))
        .map(|access| access.container.kind);
    match kind {
        Some(ContainerKind::Wiki) => {
            let ws = ws.reaching(ContainerKinds::WIKIS);
            let pages = page_tree_read(&ws, container_id).await?;
            Ok(json!({ "kind": "wiki", "pages": pages }))
        }
        Some(ContainerKind::Course) => {
            let ws = ws.reaching(ContainerKinds::COURSES);
            let units = unit_tree_read(&ws, container_id).await?;
            let progress = my_progress_in(&ws, container_id).await?;
            Ok(json!({ "kind": "course", "units": units, "myProgress": progress }))
        }
        _ => Err(AppError::not_found("board").into()),
    }
}

pub(super) async fn list_comments(
    app: &App,
    caller: &Caller,
    arguments: ListCommentsArguments,
) -> Result<Value, ToolError> {
    let kinds = match arguments.kind {
        ItemKind::Card => ContainerKinds::BOARDS,
        ItemKind::Page => ContainerKinds::WIKIS,
        ItemKind::Unit => ContainerKinds::COURSES,
    };
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Read,
        kinds,
    )
    .await?;
    let (container_id, item_id) = (&arguments.container_id, &arguments.item_id);
    let item = ItemPath::named(container_id, item_id);
    let comments = match arguments.kind {
        ItemKind::Card => comments_of::<CardItem>(&ws, &item).await?,
        ItemKind::Page => comments_of::<PageItem>(&ws, &item).await?,
        ItemKind::Unit => comments_of::<UnitItem>(&ws, &item).await?,
    };
    let mut listed = Vec::with_capacity(comments.len());
    for comment in comments {
        listed.push(listed_comment(&ws, container_id, comment).await?);
    }
    Ok(json!({ "comments": listed }))
}

pub(super) async fn list_activity(
    app: &App,
    caller: &Caller,
    arguments: ListActivityArguments,
) -> Result<Value, ToolError> {
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Read,
        ContainerKinds::BOARDS,
    )
    .await?;
    let query = PagingQuery {
        page: text(from_one(arguments.page)?),
        per_page: text(arguments.per_page),
    };
    let lines = card_activity_of(&ws, &arguments.card_id, query).await?;
    Ok(json!({ "lines": to_json(&lines)? }))
}

/// `comment`, of the container `container_id`, as `list_comments` lists it.
///
/// # Errors
///
/// Returns the store's error.
pub(super) async fn listed_comment(
    ws: &InWorkspace,
    container_id: &str,
    comment: Block,
) -> Result<ListedComment, AppError> {
    let written = ws
        .app
        .body_in_markdown(ws.markdown_reader(), container_id, comment.body.as_ref())
        .await?;
    Ok(ListedComment {
        agent_token_name: comment
            .fields
            .get(COMMENT_AGENT_TOKEN_NAME)
            .and_then(Value::as_str)
            .map(str::to_owned),
        id: comment.id,
        created_by: comment.created_by,
        created_at: comment.created_at,
        updated_at: comment.updated_at,
        edited_at: comment.edited_at,
        markdown: written.markdown,
    })
}

pub(super) async fn list_files(
    app: &App,
    caller: &Caller,
    arguments: ListFilesArguments,
) -> Result<Value, ToolError> {
    let files = match &arguments.card {
        None => {
            let ws = enter(
                app,
                caller,
                &arguments.workspace_id,
                ApiTokenScope::Read,
                ContainerKinds::EVERY,
            )
            .await?;
            files_of_container(&ws, &arguments.container_id).await?
        }
        Some(card_id) => {
            let ws = enter(
                app,
                caller,
                &arguments.workspace_id,
                ApiTokenScope::Read,
                ContainerKinds::BOARDS,
            )
            .await?;
            files_of_card(&ws, &arguments.container_id, card_id).await?
        }
    };
    Ok(json!({ "files": files }))
}

pub(super) async fn today(
    app: &App,
    caller: &Caller,
    arguments: WorkspaceArguments,
) -> Result<Value, ToolError> {
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Read,
        ContainerKinds::EVERY,
    )
    .await?
    .require_no_guest()?;
    let today = ws
        .app
        .workspace_today(&ws.workspace_id, &ws.caller.user_id)
        .await?;
    Ok(to_json(&today)?)
}
