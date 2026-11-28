//! The tools of the MCP server: what each is called and says it does, what it takes
//! (an input schema made from the type its arguments are read into), the scope of the
//! token it needs, and the call of it.
//!
//! A tool enters its workspace as a route under `/w/{workspaceId}` does ([`enter`]),
//! and does there what the route does, through the same functions: what a token may
//! read, write and reach is asked of it in one place, whichever way it came.

use std::sync::LazyLock;

use fvoci_model::{ApiTokenScope, ContainerKinds};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use utoipa::PartialSchema;

use super::arguments::{
    FindPeopleArguments, GetItemArguments, GetOutlineArguments, ListActivityArguments,
    ListCardsArguments, ListCommentsArguments, ListFilesArguments, NoArguments,
    PostCommentArguments, SaveCardArguments, SaveCardsArguments, SavePageOrUnitArguments,
    SearchArguments, WorkspaceArguments,
};
use super::rpc::{INTERNAL_ERROR, RpcError};
use super::{reads, writes};
use crate::api::workspace::InWorkspace;
use crate::app::{App, Caller};
use crate::error::{AppError, ErrorKind};

/// A tool of the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Tool {
    ListWorkspaces,
    GetWorkspaceOverview,
    FindPeople,
    SearchWorkspace,
    ListCards,
    GetItem,
    GetOutline,
    ListComments,
    ListActivity,
    ListFiles,
    GetToday,
    SaveCard,
    SaveCards,
    SavePageOrUnit,
    PostComment,
}

/// Every tool, in the order they are listed: those that read, then those that write.
pub(super) const TOOLS: [Tool; 15] = [
    Tool::ListWorkspaces,
    Tool::GetWorkspaceOverview,
    Tool::FindPeople,
    Tool::SearchWorkspace,
    Tool::ListCards,
    Tool::GetItem,
    Tool::GetOutline,
    Tool::ListComments,
    Tool::ListActivity,
    Tool::ListFiles,
    Tool::GetToday,
    Tool::SaveCard,
    Tool::SaveCards,
    Tool::SavePageOrUnit,
    Tool::PostComment,
];

impl Tool {
    /// The tool called `name`.
    pub(super) fn named(name: &str) -> Option<Self> {
        TOOLS.into_iter().find(|tool| tool.name() == name)
    }

    /// What it is called.
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::ListWorkspaces => "list_workspaces",
            Self::GetWorkspaceOverview => "get_workspace_overview",
            Self::FindPeople => "find_people",
            Self::SearchWorkspace => "search_workspace",
            Self::ListCards => "list_cards",
            Self::GetItem => "get_item",
            Self::GetOutline => "get_outline",
            Self::ListComments => "list_comments",
            Self::ListActivity => "list_activity",
            Self::ListFiles => "list_files",
            Self::GetToday => "get_today",
            Self::SaveCard => "save_card",
            Self::SaveCards => "save_cards",
            Self::SavePageOrUnit => "save_page_or_unit",
            Self::PostComment => "post_comment",
        }
    }

    /// The scope of the token it needs.
    pub(super) fn scope(self) -> ApiTokenScope {
        match self {
            Self::SaveCard | Self::SaveCards | Self::SavePageOrUnit | Self::PostComment => {
                ApiTokenScope::Write
            }
            _ => ApiTokenScope::Read,
        }
    }

    /// What a person reads it as.
    fn title(self) -> &'static str {
        match self {
            Self::ListWorkspaces => "List workspaces",
            Self::GetWorkspaceOverview => "Workspace overview",
            Self::FindPeople => "Find people",
            Self::SearchWorkspace => "Search a workspace",
            Self::ListCards => "List cards",
            Self::GetItem => "Read a card, page or unit",
            Self::GetOutline => "Outline of a wiki or course",
            Self::ListComments => "List comments",
            Self::ListActivity => "Activity of a card",
            Self::ListFiles => "List files",
            Self::GetToday => "Today",
            Self::SaveCard => "Save a card",
            Self::SaveCards => "Change cards alike",
            Self::SavePageOrUnit => "Save a page or unit",
            Self::PostComment => "Post a comment",
        }
    }

    /// What it does, as the agent is told.
    fn description(self) -> &'static str {
        match self {
            Self::ListWorkspaces => {
                "The workspaces this token reaches: those it was given that its owner still works in, \
                 and the owner's own space if it was given that. Start here; every other tool takes a \
                 workspaceId of this list."
            }
            Self::GetWorkspaceOverview => {
                "The workspace in one answer: its name, who the caller is, and every board, wiki and \
                 course they read, with whether they joined it, their role, and for a board the \
                 properties its cards take (cardProperties: ids, types, options). Read it before \
                 list_cards or save_card."
            }
            Self::FindPeople => {
                "Up to ten people of the workspace whose login name or name holds q, as the caller \
                 sees them (no email): the ids to give a card's people property or mention in \
                 Markdown as @[name](fvoci-user://<id>)."
            }
            Self::SearchWorkspace => {
                "Search what the cards, pages and units of the workspace say, by words that a hit's \
                 title and body hold together (#12 finds card number 12). Each hit names its \
                 container and kind, with a snippet."
            }
            Self::ListCards => {
                "One page of a board's cards in short (id, number, title, parent, property values, \
                 updatedAt), chosen by title text, parent, assignee, one property's value or a time \
                 of change. Property ids and option ids are in get_workspace_overview."
            }
            Self::GetItem => {
                "A card, a page or a unit with its body in Markdown (bodyMarkdown), whether the body \
                 has blocks Markdown does not hold (hasComplexBlocks: then add to it with \
                 appendMarkdown rather than writing it whole), the items below it and the files it \
                 shows: of a card, the first 50 cards below it (hasMoreChildren: list_cards with \
                 parentId reads them all). A card by id, or by containerId and number; a page or \
                 unit by containerId and id."
            }
            Self::GetOutline => {
                "The tree of a wiki's pages, or of a course's chapters and units with the caller's own \
                 progress (empty for an open course they have not joined), each with updatedAt, \
                 without bodies."
            }
            Self::ListComments => {
                "The comments on a card, page or unit, oldest first, each in Markdown with who wrote \
                 it and when; one an agent wrote names its token (agentTokenName)."
            }
            Self::ListActivity => {
                "What was done to a card, the newest first, a page at a time: its making, each \
                 change of its title, of a property's value (as the card stores it: option and \
                 person ids, dates), of its parent and of whether it is a template, and its coming \
                 back from the trash, each with who did it (actorId) and, for an agent, its token \
                 (actorTokenName). One person's changes of one thing within five minutes are one \
                 line. Its body's changes are not in it."
            }
            Self::ListFiles => {
                "The files uploaded to a board or wiki, or those one card shows: their ids \
                 (fvoci-file://<fileId> in Markdown), names, types and sizes."
            }
            Self::GetToday => {
                "The caller's day in the workspace: their cards due or overdue, units scheduled, \
                 reviews due and meetings coming."
            }
            Self::SaveCard => {
                "Make a card on boardId, or change cardId: title, icon, parent, property values, and \
                 the body, whole (bodyMarkdown, with readAt; refused while people type in it) or \
                 added to (appendMarkdown, which keeps what they type), and how it repeats \
                 (recurrence: completed, it makes its next card). Kept as a version by this token, \
                 and told to the card's people as the agent's."
            }
            Self::SaveCards => {
                "Set or clear the same property values of 1 to 200 cards of one board at once, all \
                 or none: each card changes as save_card with only properties and \
                 deletedProperties would change it. Answers the cards in short."
            }
            Self::SavePageOrUnit => {
                "Make a page of a wiki or a unit (or chapter) of a course, or change one: title, \
                 icon, place, and the body, whole (bodyMarkdown, with readAt; refused while people \
                 type in it) or added to (appendMarkdown). With progress, records the caller's own \
                 progress on a unit: only in a course they joined (403 otherwise)."
            }
            Self::PostComment => {
                "Comment on a card, page or unit, in Markdown, as the caller by this token: the \
                 comment names the token, and whoever follows the item is told."
            }
        }
    }

    /// The schema of what it takes, made from the type its arguments are read into.
    pub(super) fn input_schema(self) -> Value {
        let schema = match self {
            Self::ListWorkspaces => NoArguments::schema(),
            Self::GetWorkspaceOverview | Self::GetToday => WorkspaceArguments::schema(),
            Self::FindPeople => FindPeopleArguments::schema(),
            Self::SearchWorkspace => SearchArguments::schema(),
            Self::ListCards => ListCardsArguments::schema(),
            Self::GetItem => GetItemArguments::schema(),
            Self::GetOutline => GetOutlineArguments::schema(),
            Self::ListComments => ListCommentsArguments::schema(),
            Self::ListActivity => ListActivityArguments::schema(),
            Self::ListFiles => ListFilesArguments::schema(),
            Self::SaveCard => SaveCardArguments::schema(),
            Self::SaveCards => SaveCardsArguments::schema(),
            Self::SavePageOrUnit => SavePageOrUnitArguments::schema(),
            Self::PostComment => PostCommentArguments::schema(),
        };
        serde_json::to_value(schema).expect("a schema is JSON")
    }

    /// What a client is told of how it behaves.
    fn annotations(self) -> Value {
        let reads = self.scope() == ApiTokenScope::Read;
        json!({
            "title": self.title(),
            "readOnlyHint": reads,
            "destructiveHint": matches!(
                self,
                Self::SaveCard | Self::SaveCards | Self::SavePageOrUnit
            ),
            "idempotentHint": reads,
            "openWorldHint": false,
        })
    }

    /// The tool as `tools/list` lists it.
    fn listed(self) -> Value {
        json!({
            "name": self.name(),
            "title": self.title(),
            "description": self.description(),
            "inputSchema": self.input_schema(),
            "annotations": self.annotations(),
        })
    }
}

/// The answers to `tools/list`, made once: for a token that reads, and for one that
/// writes.
static LISTED: LazyLock<[Value; 2]> =
    LazyLock::new(|| [listing(ApiTokenScope::Read), listing(ApiTokenScope::Write)]);

/// The answer to `tools/list` for a token of `scope`: the tools it covers, and no
/// other, so that an agent that may only read is not told of what it may not do.
pub(super) fn listed_for(scope: ApiTokenScope) -> Value {
    let [read, write] = &*LISTED;
    match scope {
        ApiTokenScope::Read => read.clone(),
        ApiTokenScope::Write => write.clone(),
    }
}

/// The tools a token of `scope` covers, as `tools/list` lists them.
fn listing(scope: ApiTokenScope) -> Value {
    let tools: Vec<Value> = TOOLS
        .into_iter()
        .filter(|tool| scope.covers(tool.scope()))
        .map(Tool::listed)
        .collect();
    json!({ "tools": tools })
}

/// What stops a tool short of its result.
#[derive(Debug)]
pub(super) enum ToolError {
    /// The arguments are not what the tool takes.
    Arguments(String),
    /// The application refused, or failed.
    App(AppError),
    /// The application refused, or failed, after the tool wrote something.
    Partly(Box<Partly>),
}

/// A refusal, or a failure, after a tool wrote something.
#[derive(Debug)]
pub(super) struct Partly {
    /// What refused, or failed.
    pub(super) error: AppError,
    /// What was written: its identifier and whether it was made or changed, told of
    /// with the problem.
    pub(super) written: Value,
}

impl From<AppError> for ToolError {
    fn from(error: AppError) -> Self {
        Self::App(error)
    }
}

impl ToolError {
    /// Arguments that are not what the tool takes, for `why`.
    pub(super) fn arguments(why: &str) -> Self {
        Self::Arguments(why.to_owned())
    }
}

/// How a call of `tools/call` is answered when not with a result.
#[derive(Debug)]
pub(super) enum Failure {
    /// An error of the protocol.
    Rpc(RpcError),
    /// An answer of HTTP: a token that sent too many requests.
    Http(AppError),
}

/// Calls the tool `tools/call` names with `params`, for `caller`.
///
/// What the server refuses (a request it cannot do, what is not there or not the
/// caller's to do) is the tool's result, marked `isError`, with the problem as the
/// HTTP routes answer it. Arguments that are not what the tool takes and a tool that
/// is not there are errors of the protocol (`-32602`), a failure of the server is
/// `-32603`, and too many requests are answered as HTTP answers them (429).
///
/// # Errors
///
/// Returns the failure to answer with.
pub(super) async fn call(
    app: &App,
    caller: &Caller,
    mut params: Map<String, Value>,
) -> Result<Value, Failure> {
    let Some(Value::String(name)) = params.remove("name") else {
        return Err(Failure::Rpc(RpcError::invalid_params(
            "tools/call names the tool (name)",
        )));
    };
    let arguments = match params.remove("arguments") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(arguments @ Value::Object(_)) => arguments,
        Some(_) => {
            return Err(Failure::Rpc(RpcError::invalid_params(
                "the arguments are an object",
            )));
        }
    };
    let Some(tool) = Tool::named(&name) else {
        return Err(Failure::Rpc(RpcError::invalid_params(format!(
            "there is no tool {name}"
        ))));
    };
    // The tools a token does not cover are not listed to it; one called all the same is
    // refused here, before its arguments are read, and again where it enters its
    // workspace ([`enter`]).
    let outcome = match caller.require_scope(tool.scope()) {
        Ok(()) => run(tool, app, caller, arguments).await,
        Err(refused) => Err(ToolError::App(refused)),
    };
    match outcome {
        Ok(value) => Ok(answered(&value)),
        Err(ToolError::Arguments(why)) => Err(Failure::Rpc(RpcError::invalid_params(format!(
            "{name}: {why}"
        )))),
        Err(ToolError::App(error)) => refused(error),
        Err(ToolError::Partly(partly)) => Ok(refused_after(&partly.error, partly.written)),
    }
}

/// What a tool answers for an error of the application after it wrote `written`: the
/// problem with what was written (`written`), marked `isError`, whatever the error,
/// for the agent to change what is there rather than make it again. A failure of the
/// server is the server's problem, its reason in its log.
fn refused_after(error: &AppError, written: Value) -> Value {
    error.log();
    let mut problem = serde_json::to_value(error.body()).expect("a problem is JSON");
    if let Value::Object(members) = &mut problem {
        members.insert("written".to_owned(), written);
    }
    refused_with(&problem)
}

/// What a tool answers for an error of the application.
fn refused(error: AppError) -> Result<Value, Failure> {
    match error.kind() {
        ErrorKind::RateLimited => Err(Failure::Http(error)),
        ErrorKind::Internal => {
            error.log();
            Err(Failure::Rpc(RpcError::new(
                INTERNAL_ERROR,
                "the server failed; the failure is in its log",
            )))
        }
        _ => {
            error.log();
            let problem = serde_json::to_value(error.body()).expect("a problem is JSON");
            Ok(refused_with(&problem))
        }
    }
}

/// The result of a tool that did what it was called for: `value` as the one text it
/// answers with.
fn answered(value: &Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": value.to_string() }],
        "isError": false,
    })
}

/// The result of a tool that was refused: the problem, as the one text it answers with.
fn refused_with(problem: &Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": problem.to_string() }],
        "isError": true,
    })
}

async fn run(tool: Tool, app: &App, caller: &Caller, arguments: Value) -> Result<Value, ToolError> {
    match tool {
        Tool::ListWorkspaces => {
            let NoArguments {} = decode(arguments)?;
            reads::list_workspaces(app, caller).await
        }
        Tool::GetWorkspaceOverview => reads::overview(app, caller, decode(arguments)?).await,
        Tool::FindPeople => reads::find_people(app, caller, decode(arguments)?).await,
        Tool::SearchWorkspace => reads::search(app, caller, decode(arguments)?).await,
        Tool::ListCards => reads::list_cards(app, caller, decode(arguments)?).await,
        Tool::GetItem => reads::get_item(app, caller, decode(arguments)?).await,
        Tool::GetOutline => reads::get_outline(app, caller, decode(arguments)?).await,
        Tool::ListComments => reads::list_comments(app, caller, decode(arguments)?).await,
        Tool::ListActivity => reads::list_activity(app, caller, decode(arguments)?).await,
        Tool::ListFiles => reads::list_files(app, caller, decode(arguments)?).await,
        Tool::GetToday => reads::today(app, caller, decode(arguments)?).await,
        Tool::SaveCard => writes::save_card(app, caller, decode(arguments)?).await,
        Tool::SaveCards => writes::save_cards(app, caller, decode(arguments)?).await,
        Tool::SavePageOrUnit => writes::save_page_or_unit(app, caller, decode(arguments)?).await,
        Tool::PostComment => writes::post_comment(app, caller, decode(arguments)?).await,
    }
}

/// The arguments of a tool, read into what it takes.
fn decode<T: DeserializeOwned>(arguments: Value) -> Result<T, ToolError> {
    serde_json::from_value(arguments).map_err(|error| ToolError::Arguments(error.to_string()))
}

/// The workspace `workspace_id` for `caller`, as `Delegated<N, W>` enters it for a
/// request: refused to a token whose scope does not cover `needs`, then entered as any
/// route under `/w/{workspaceId}` enters it (who is let in, and what a token reaches
/// of it, asked of `App::enter_workspace_as`), reaching the containers of `kinds`,
/// never more than the caller may (a token no diary).
///
/// # Errors
///
/// Returns 403 with the reason `token_scope_insufficient`, 404 unless the caller is a
/// member of the workspace that a token reaches, and the errors of
/// `App::enter_workspace_as`.
pub(super) async fn enter(
    app: &App,
    caller: &Caller,
    workspace_id: &str,
    needs: ApiTokenScope,
    kinds: ContainerKinds,
) -> Result<InWorkspace, AppError> {
    caller.require_scope(needs)?;
    Ok(
        InWorkspace::enter_as(app, caller.clone(), workspace_id.to_owned())
            .await?
            .reaching(kinds),
    )
}
