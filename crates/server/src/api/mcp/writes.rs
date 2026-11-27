//! The tools that write: each makes the request body of the route it stands for and
//! hands it to what the route does with it, so that what is checked of a change, who is
//! told of it and how it is kept (the version before an agent's, the agent's own, a body
//! added to while people type) are the route's.

use axum::body::Bytes;
use fvoci_model::{
    ApiTokenScope, BlockError, Body, BodyChange, CardPatch, ContainerKinds, PagePatch, Unit,
    UnitKind, UnitPatch,
};
use serde::Serialize;
use serde_json::{Map, Value, json};

use super::arguments::{
    ItemKind, OutlineEntry, PageOrUnit, PostCommentArguments, ProgressArguments, SaveCardArguments,
    SavePageOrUnitArguments,
};
use super::reads::{card_in_markdown, listed_comment, page_in_markdown, to_json, unit_in_markdown};
use super::tools::{Partly, ToolError, enter};
use crate::api::access::require_container_permission;
use crate::api::card_comments::{CardCommentParent, NewCardComment};
use crate::api::card_markdown::append_to_card;
use crate::api::cards::{card_with_permission, create_card_from, patch_card_from};
use crate::api::comments::{CommentParentKind, CommenterOf, create_comment_on};
use crate::api::courses::{create_unit_from, patch_unit_from, unit_with_permission};
use crate::api::page_comments::{NewPageComment, PageCommentParent};
use crate::api::unit_comments::{NewUnitComment, UnitCommentParent};
use crate::api::unit_progress::{ProgressToRecord, progress_to_record, record_progress};
use crate::api::wikis::{create_page_from, page_with_permission, patch_page_from};
use crate::api::workspace::InWorkspace;
use crate::app::{App, Caller, ChangeNotification, body_of_markdown, comment_text_of_markdown};
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// What a page or unit is changed with to be denied.
const CHANGE_DENIED: &str = "access denied to change it";

/// Why a `readAt` sent with nothing written whole is refused: it would be checked by a
/// change of nothing.
const READ_AT_ALONE: &str = "readAt is checked of what is written whole (the title, the \
     body and the like); appendMarkdown keeps what changed since and takes none";

/// `value` as a request body.
fn request_body(value: &impl Serialize) -> Result<Bytes, AppError> {
    serde_json::to_vec(value)
        .map(Bytes::from)
        .map_err(|error| AppError::internal(format_args!("a request body as JSON: {error}")))
}

/// The members of a request body, those of `members` that are there.
fn members<const N: usize>(members: [(&str, Option<Value>); N]) -> Value {
    Value::Object(
        members
            .into_iter()
            .filter_map(|(name, value)| value.map(|value| (name.to_owned(), value)))
            .collect::<Map<_, _>>(),
    )
}

/// Refuses the arguments that write a body two ways at once.
fn one_body(
    body_markdown: Option<&String>,
    append_markdown: Option<&String>,
) -> Result<(), ToolError> {
    if body_markdown.is_some() && append_markdown.is_some() {
        return Err(ToolError::arguments(
            "bodyMarkdown writes the body whole and appendMarkdown adds to it: send one",
        ));
    }
    Ok(())
}

/// What a tool wrote before what came after it was refused: told of with the refusal
/// (`written`, its identifier and whether it was made or changed), so that an agent
/// changes what is there by its identifier rather than make it again.
struct Done {
    /// The identifier of what is written.
    id: String,
    /// Whether it was made, or one that was there was changed.
    made: bool,
    /// Whether anything of it is written yet.
    wrote: bool,
}

impl Done {
    /// The card, page, unit or comment `id`, just made.
    fn made(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            made: true,
            wrote: true,
        }
    }

    /// The card, page or unit `id`, which is there, before anything of it is changed.
    fn changing(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            made: false,
            wrote: false,
        }
    }

    /// Something of it is written.
    fn wrote(&mut self) {
        self.wrote = true;
    }

    /// `error`, which refused what came after what is written, if anything is.
    fn told(&self, error: AppError) -> ToolError {
        if !self.wrote {
            return error.into();
        }
        let was = if self.made { "made" } else { "changed" };
        ToolError::Partly(Box::new(Partly {
            error,
            written: json!({ "id": self.id, "was": was }),
        }))
    }
}

/// The body `markdown` writes whole over `current` in the container `container_id`,
/// checked as the routes check it before they write it.
async fn body_checked(
    ws: &InWorkspace,
    container_id: &str,
    current: Option<&Body>,
    markdown: &str,
) -> Result<BodyChange, AppError> {
    let change = body_of_markdown(current, markdown)?;
    ws.app
        .check_body_files(container_id, change.new_body())
        .await
        .map_err(|error| error.moved("/body", "/bodyMarkdown"))?;
    Ok(change)
}

pub(super) async fn save_card(
    app: &App,
    caller: &Caller,
    arguments: SaveCardArguments,
) -> Result<Value, ToolError> {
    one_body(
        arguments.body_markdown.as_ref(),
        arguments.append_markdown.as_ref(),
    )?;
    let writes_whole = arguments.title.is_some()
        || arguments.icon.is_some()
        || arguments.parent_id.is_some()
        || arguments.properties.is_some()
        || arguments.deleted_properties.is_some()
        || arguments.body_markdown.is_some();
    if arguments.read_at.is_some() && !writes_whole {
        return Err(ToolError::arguments(READ_AT_ALONE));
    }
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Write,
        ContainerKinds::BOARDS,
    )
    .await?;
    match (arguments.card_id.clone(), arguments.board_id.clone()) {
        (None, Some(board_id)) => new_card(&ws, &board_id, arguments).await,
        (Some(card_id), None) => change_card(&ws, &card_id, arguments).await,
        _ => Err(ToolError::arguments(
            "boardId makes a card and cardId changes one: send one of them",
        )),
    }
}

/// Makes a card on the board `board_id`. Its body, if it has one, is checked before the
/// card is made and written after it as the agent's change, as a page's is: kept as a
/// version by the token, and told to the card's people as the agent's.
async fn new_card(
    ws: &InWorkspace,
    board_id: &str,
    arguments: SaveCardArguments,
) -> Result<Value, ToolError> {
    if arguments.read_at.is_some() || arguments.deleted_properties.is_some() {
        return Err(ToolError::arguments(
            "readAt and deletedProperties change a card: a new card has neither",
        ));
    }
    // A new card has no body to add to: what is added is its body.
    let body_markdown = arguments.body_markdown.or(arguments.append_markdown);
    if let Some(markdown) = &body_markdown {
        // Asked before anything of the board is read for the body.
        require_container_permission(
            &ws.app,
            &ws.caller.user_id,
            board_id,
            ContainerPermission::ManageCards,
            "access denied to create card",
        )
        .await?;
        body_checked(ws, board_id, None, markdown).await?;
    }
    let new = members([
        ("title", arguments.title.map(Value::String)),
        ("icon", arguments.icon.map(Value::String)),
        ("parentId", arguments.parent_id.map(Value::String)),
        ("properties", arguments.properties.map(Value::Object)),
    ]);
    let card =
        create_card_from(ws, board_id, &request_body(&new)?, ChangeNotification::Send).await?;
    let done = Done::made(&card.id);
    if let Some(markdown) = body_markdown {
        let patch = CardPatch {
            body_markdown: Some(markdown),
            read_at: Some(card.updated_at),
            ..CardPatch::default()
        };
        let body = request_body(&patch).map_err(|error| done.told(error))?;
        patch_card_from(ws, &card.id, &body, ChangeNotification::Send)
            .await
            .map_err(|error| done.told(error))?;
    }
    card_in_markdown(ws, &card.id)
        .await
        .map_err(|error| done.told(error))
}

/// Changes the card `card_id`: the fields first, then what is added to the body.
async fn change_card(
    ws: &InWorkspace,
    card_id: &str,
    arguments: SaveCardArguments,
) -> Result<Value, ToolError> {
    // What is added after the other changes is checked before them, so that nothing is
    // changed when it is refused.
    if let Some(markdown) = &arguments.append_markdown {
        let card = card_with_permission(
            &ws.app,
            &ws.caller.user_id,
            card_id,
            ContainerPermission::ManageCards,
            "access denied to change card",
        )
        .await?;
        ws.app
            .markdown_to_append(&card.container_id, markdown)
            .await?;
    }
    let patch = CardPatch {
        parent_id: arguments.parent_id,
        title: arguments.title,
        icon: arguments.icon,
        updated_properties: arguments.properties.unwrap_or_default(),
        deleted_properties: arguments.deleted_properties.unwrap_or_default(),
        body: BodyChange::Keep,
        read_at: arguments.read_at,
        body_markdown: arguments.body_markdown,
    };
    let mut done = Done::changing(card_id);
    if patch != CardPatch::default() {
        let body = request_body(&patch)?;
        patch_card_from(ws, card_id, &body, ChangeNotification::Send).await?;
        done.wrote();
    } else if arguments.append_markdown.is_none() {
        return Err(ToolError::arguments("there is nothing to change"));
    }
    // The body's room is asked for last, by the one write that keeps what its editors
    // type.
    if let Some(markdown) = &arguments.append_markdown {
        append_to_card(ws, card_id, markdown)
            .await
            .map_err(|error| done.told(error))?;
        done.wrote();
    }
    card_in_markdown(ws, card_id)
        .await
        .map_err(|error| done.told(error))
}

pub(super) async fn save_page_or_unit(
    app: &App,
    caller: &Caller,
    arguments: SavePageOrUnitArguments,
) -> Result<Value, ToolError> {
    one_body(
        arguments.body_markdown.as_ref(),
        arguments.append_markdown.as_ref(),
    )?;
    if arguments.id.is_some() && (arguments.parent_id.is_some() || arguments.after.is_some()) {
        return Err(ToolError::arguments(
            "parentId and after place a new one; one that is there stays where it is",
        ));
    }
    if arguments.id.is_none() && arguments.read_at.is_some() {
        return Err(ToolError::arguments("readAt is of one that is changed"));
    }
    let writes_whole =
        arguments.title.is_some() || arguments.icon.is_some() || arguments.body_markdown.is_some();
    if arguments.read_at.is_some() && !writes_whole {
        return Err(ToolError::arguments(READ_AT_ALONE));
    }
    match arguments.kind {
        PageOrUnit::Page => save_page(app, caller, arguments).await,
        PageOrUnit::Unit => save_unit(app, caller, arguments).await,
    }
}

/// What a page's or unit's body is to be written whole with, and what is to be added
/// after it, both checked before anything is written.
struct BodyWrites {
    /// The body written whole, with the fields.
    whole: BodyChange,
    /// The Markdown added after them.
    added: Option<String>,
}

/// The page or unit whose body is written: one that is made, or one that is there,
/// with its body.
#[derive(Clone, Copy)]
enum Written<'a> {
    New,
    There(Option<&'a Body>),
}

/// The writes of the body of a page or unit of the container `container_id`, checked
/// as the routes check them: a new one has no body to add to, and what is added is its
/// body.
async fn body_writes(
    ws: &InWorkspace,
    container_id: &str,
    written: Written<'_>,
    body_markdown: Option<String>,
    append_markdown: Option<String>,
) -> Result<BodyWrites, AppError> {
    let (whole, added, current) = match written {
        Written::New => (body_markdown.or(append_markdown), None, None),
        Written::There(current) => (body_markdown, append_markdown, current),
    };
    let whole = match whole {
        None => BodyChange::Keep,
        Some(markdown) => body_checked(ws, container_id, current, &markdown).await?,
    };
    if let Some(markdown) = &added {
        ws.app.markdown_to_append(container_id, markdown).await?;
    }
    Ok(BodyWrites { whole, added })
}

/// Of the page or unit `current`, if it is there, its body as it is written.
fn written_of(current: Option<&Option<Body>>) -> Written<'_> {
    current.map_or(Written::New, |body| Written::There(body.as_ref()))
}

/// The progress `progress` says, checked before anything is written.
async fn progress_checked(
    ws: &InWorkspace,
    course_id: &str,
    progress: Option<&ProgressArguments>,
) -> Result<Option<ProgressToRecord>, AppError> {
    let Some(progress) = progress else {
        return Ok(None);
    };
    let update = members([
        ("completed", progress.completed.map(Value::Bool)),
        (
            "lastPosition",
            progress.last_position.clone().map(Value::String),
        ),
    ]);
    Ok(Some(
        progress_to_record(ws, course_id, &request_body(&update)?).await?,
    ))
}

/// Refuses a page or unit to be made in the container `container_id` by a caller who
/// may not, before anything of it is read for the body: as the route asks.
async fn may_make_in(ws: &InWorkspace, container_id: &str, what: &str) -> Result<(), AppError> {
    require_container_permission(
        &ws.app,
        &ws.caller.user_id,
        container_id,
        ContainerPermission::ManageCards,
        &format!("access denied to create {what}"),
    )
    .await
}

async fn save_page(
    app: &App,
    caller: &Caller,
    arguments: SavePageOrUnitArguments,
) -> Result<Value, ToolError> {
    if arguments.progress.is_some() || arguments.unit_kind.is_some() {
        return Err(ToolError::arguments(
            "progress and unitKind are of a course's units, not of a page",
        ));
    }
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Write,
        ContainerKinds::WIKIS,
    )
    .await?;
    let wiki_id = arguments.container_id.as_str();
    let current = match &arguments.id {
        None => {
            may_make_in(&ws, wiki_id, "page").await?;
            None
        }
        Some(page_id) => Some(
            page_with_permission(
                &ws,
                wiki_id,
                page_id,
                ContainerPermission::ManageCards,
                CHANGE_DENIED,
            )
            .await?,
        ),
    };
    let is_new = current.is_none();
    let fields = arguments.title.is_some() || arguments.icon.is_some();
    if !is_new
        && !fields
        && arguments.body_markdown.is_none()
        && arguments.append_markdown.is_none()
    {
        return Err(ToolError::arguments("there is nothing to change"));
    }
    let writes = body_writes(
        &ws,
        wiki_id,
        written_of(current.as_ref().map(|page| &page.body)),
        arguments.body_markdown,
        arguments.append_markdown,
    )
    .await?;
    let (mut done, patch) = match current {
        None => {
            let new = members([
                ("title", arguments.title.map(Value::String)),
                ("icon", arguments.icon.map(Value::String)),
                ("parentId", arguments.parent_id.map(Value::String)),
                ("after", arguments.after.map(Value::String)),
            ]);
            let page = create_page_from(&ws, wiki_id, &request_body(&new)?).await?;
            let patch = PagePatch {
                read_at: Some(page.updated_at),
                body: writes.whole,
                ..PagePatch::default()
            };
            (Done::made(&page.id), patch)
        }
        Some(page) => {
            let patch = PagePatch {
                title: arguments.title,
                icon: arguments.icon,
                body: writes.whole,
                read_at: arguments.read_at,
            };
            (Done::changing(&page.id), patch)
        }
    };
    if !patch.body.is_keep() || (!is_new && fields) {
        let body = request_body(&patch).map_err(|error| done.told(error))?;
        patch_page_from(&ws, wiki_id, &done.id, &body)
            .await
            .map_err(|error| done.told(error))?;
        done.wrote();
    }
    if let Some(markdown) = &writes.added {
        ws.app
            .append_markdown(&ws.caller, &done.id, wiki_id, markdown)
            .await
            .map_err(|error| done.told(error))?;
        done.wrote();
    }
    page_in_markdown(&ws, wiki_id, &done.id)
        .await
        .map_err(|error| done.told(error))
}

async fn save_unit(
    app: &App,
    caller: &Caller,
    arguments: SavePageOrUnitArguments,
) -> Result<Value, ToolError> {
    if arguments.icon.is_some() {
        return Err(ToolError::arguments("a unit has no icon"));
    }
    if arguments.id.is_some() && arguments.unit_kind.is_some() {
        return Err(ToolError::arguments(
            "unitKind is of a new entry; one that is there stays what it is",
        ));
    }
    let ws = enter(
        app,
        caller,
        &arguments.workspace_id,
        ApiTokenScope::Write,
        ContainerKinds::COURSES,
    )
    .await?;
    let course_id = arguments.container_id.as_str();
    let has_body = arguments.body_markdown.is_some() || arguments.append_markdown.is_some();
    let changes = arguments.title.is_some() || arguments.read_at.is_some() || has_body;
    let current = match arguments.id.as_deref() {
        None => {
            may_make_in(&ws, course_id, "unit").await?;
            None
        }
        // Progress alone is a reader's to record; a change is an editor's.
        Some(unit_id) if changes => {
            Some(unit_there(&ws, course_id, unit_id, ContainerPermission::ManageCards).await?)
        }
        Some(unit_id) => {
            Some(unit_there(&ws, course_id, unit_id, ContainerPermission::View).await?)
        }
    };
    if current.is_some() && !changes && arguments.progress.is_none() {
        return Err(ToolError::arguments("there is nothing to change"));
    }
    let kind = current
        .as_ref()
        .map_or_else(|| kind_asked(arguments.unit_kind), |unit| unit.kind);
    if kind == UnitKind::Chapter && has_body {
        return Err(AppError::from(BlockError::BodyNotAllowed).into());
    }
    if kind == UnitKind::Chapter && arguments.progress.is_some() {
        return Err(
            AppError::bad_request("a chapter has no progress: its units have")
                .with_field("progress")
                .into(),
        );
    }
    // Everything that is written is checked before the first of it.
    let progress = progress_checked(&ws, course_id, arguments.progress.as_ref()).await?;
    let writes = body_writes(
        &ws,
        course_id,
        written_of(current.as_ref().map(|unit| &unit.body)),
        arguments.body_markdown,
        arguments.append_markdown,
    )
    .await?;
    let entry = Entry {
        title: arguments.title,
        parent_id: arguments.parent_id,
        after: arguments.after,
        read_at: arguments.read_at,
    };
    let done = unit_written(&ws, course_id, current, kind, entry, writes).await?;
    let mut saved = unit_in_markdown(&ws, course_id, &done.id)
        .await
        .map_err(|error| done.told(error))?;
    if let Some(progress) = &progress {
        let recorded = record_progress(&ws, course_id, &done.id, progress)
            .await
            .map_err(|error| done.told(error))?;
        if let Value::Object(members) = &mut saved {
            members.insert(
                "myProgress".to_owned(),
                to_json(&recorded).map_err(|error| done.told(error))?,
            );
        }
    }
    Ok(saved)
}

/// What of a unit is written besides its body: its title, and where a new one goes.
struct Entry {
    title: Option<String>,
    parent_id: Option<String>,
    after: Option<String>,
    read_at: Option<i64>,
}

/// Makes the unit of `kind` in the course `course_id`, or changes `current`, with
/// `entry` and the checked `writes` of its body: what is made or changed, and whether
/// anything is.
async fn unit_written(
    ws: &InWorkspace,
    course_id: &str,
    current: Option<Unit>,
    kind: UnitKind,
    entry: Entry,
    writes: BodyWrites,
) -> Result<Done, ToolError> {
    let is_new = current.is_none();
    let (mut done, patch) = match current {
        None => {
            let new = members([
                ("kind", Some(Value::String(kind_name(kind).to_owned()))),
                ("title", entry.title.map(Value::String)),
                ("parentId", entry.parent_id.map(Value::String)),
                ("after", entry.after.map(Value::String)),
            ]);
            let unit = create_unit_from(ws, course_id, &request_body(&new)?).await?;
            let patch = UnitPatch {
                read_at: Some(unit.updated_at),
                body: writes.whole,
                ..UnitPatch::default()
            };
            (Done::made(&unit.id), patch)
        }
        Some(unit) => {
            let patch = UnitPatch {
                title: entry.title,
                body: writes.whole,
                read_at: entry.read_at,
                ..UnitPatch::default()
            };
            (Done::changing(&unit.id), patch)
        }
    };
    if !patch.body.is_keep() || (!is_new && patch.title.is_some()) {
        let body = request_body(&patch).map_err(|error| done.told(error))?;
        patch_unit_from(ws, course_id, &done.id, &body)
            .await
            .map_err(|error| done.told(error))?;
        done.wrote();
    }
    if let Some(markdown) = &writes.added {
        ws.app
            .append_markdown(&ws.caller, &done.id, course_id, markdown)
            .await
            .map_err(|error| done.told(error))?;
        done.wrote();
    }
    Ok(done)
}

/// The unit `unit_id` of the course `course_id`, for a caller who has `permission`
/// there.
async fn unit_there(
    ws: &InWorkspace,
    course_id: &str,
    unit_id: &str,
    permission: ContainerPermission,
) -> Result<Unit, AppError> {
    let block = unit_with_permission(ws, course_id, unit_id, permission, CHANGE_DENIED).await?;
    Ok(Unit::from(block))
}

/// What a new entry of a course is, as `unitKind` asks: a unit if it is left out.
fn kind_asked(asked: Option<OutlineEntry>) -> UnitKind {
    match asked {
        Some(OutlineEntry::Chapter) => UnitKind::Chapter,
        Some(OutlineEntry::Unit) | None => UnitKind::Unit,
    }
}

/// The word `createUnit` takes for a unit of `kind`.
fn kind_name(kind: UnitKind) -> &'static str {
    match kind {
        UnitKind::Chapter => OutlineEntry::Chapter.as_str(),
        UnitKind::Unit => OutlineEntry::Unit.as_str(),
    }
}

pub(super) async fn post_comment(
    app: &App,
    caller: &Caller,
    arguments: PostCommentArguments,
) -> Result<Value, ToolError> {
    let text = comment_text_of_markdown(&arguments.markdown, "markdown")?;
    let body = request_body(&json!({ "body": text }))?;
    let PostCommentArguments {
        workspace_id,
        kind,
        container_id,
        item_id,
        ..
    } = arguments;
    let ws = enter(
        app,
        caller,
        &workspace_id,
        ApiTokenScope::Write,
        ContainerKinds::EVERY,
    )
    .await?;
    match kind {
        ItemKind::Card => {
            comment_on::<CardCommentParent, NewCardComment>(
                ws,
                container_id,
                &item_id,
                "card",
                &body,
            )
            .await
        }
        ItemKind::Page => {
            comment_on::<PageCommentParent, NewPageComment>(
                ws,
                container_id,
                &item_id,
                "page",
                &body,
            )
            .await
        }
        ItemKind::Unit => {
            comment_on::<UnitCommentParent, NewUnitComment>(
                ws,
                container_id,
                &item_id,
                "unit",
                &body,
            )
            .await
        }
    }
}

/// Writes the comment `body` on the item `parent_id` of `container_id`, as the route
/// that comments on a `K` does, and answers with it as `list_comments` lists it.
async fn comment_on<K, T>(
    ws: InWorkspace,
    container_id: String,
    parent_id: &str,
    parent_what: &'static str,
    body: &Bytes,
) -> Result<Value, ToolError>
where
    K: CommentParentKind,
    T: serde::de::DeserializeOwned,
{
    let commenter = CommenterOf::<K>::admit(ws, container_id, parent_id, None).await?;
    let comment = create_comment_on::<T>(
        &commenter.ws,
        &commenter.container_id,
        commenter.parent,
        parent_what,
        body,
        ChangeNotification::Send,
    )
    .await?;
    let done = Done::made(&comment.id);
    let listed = listed_comment(&commenter.ws, &commenter.container_id, comment)
        .await
        .map_err(|error| done.told(error))?;
    to_json(&listed).map_err(|error| done.told(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_body_has_the_members_that_are_there() {
        let body = members([
            ("title", Some(Value::String("A".to_owned()))),
            ("icon", None),
        ]);
        assert_eq!(body, json!({ "title": "A" }));
    }

    #[test]
    fn a_refusal_after_a_write_names_what_was_written() {
        let mut done = Done::changing("card-1");
        assert!(matches!(
            done.told(AppError::not_found("card")),
            ToolError::App(_)
        ));
        done.wrote();
        let ToolError::Partly(partly) = done.told(AppError::not_found("card")) else {
            panic!("a refusal after a change is told of with it");
        };
        assert_eq!(partly.written, json!({ "id": "card-1", "was": "changed" }));
        let ToolError::Partly(partly) = Done::made("page-1").told(AppError::conflict("late"))
        else {
            panic!("a refusal after a page is made is told of with it");
        };
        assert_eq!(partly.written, json!({ "id": "page-1", "was": "made" }));
    }

    #[test]
    fn a_body_is_written_one_way() {
        let markdown = "text".to_owned();
        assert!(one_body(Some(&markdown), None).is_ok());
        assert!(matches!(
            one_body(Some(&markdown), Some(&markdown)),
            Err(ToolError::Arguments(_))
        ));
    }
}
