//! Bodies in Markdown, as agents read and write them: a card's body is read in it
//! (`getCard` with `format=markdown`), written whole from it (`bodyMarkdown`), and
//! added to (`appendCardMarkdown`). The Markdown is read by the reader of the
//! importers, in fvoci's dialect ([`fvoci_markdown_to_blocks`]), and written by
//! [`write`], so that what one gives the other takes back the same.

#[cfg(test)]
mod tests;
mod write;

use std::collections::{BTreeSet, HashMap};

use fvoci_model::{
    Body, BodyChange, Card, ComplexBlock, RefusalReason, Refusals, WorkspaceRole, is_id,
};
use serde_json::Value;

use super::App;
use super::caller::Caller;
use super::import::markdown::{
    AssetResolver, ExternalOnlyResolver, ResolvedTarget, blocks_to_body, fvoci_markdown_to_blocks,
};
use crate::collab::Appender;
use crate::error::{AppError, Reason};
pub(crate) use write::{Names, body_to_markdown};

/// The scheme a file of the body's board is named by.
const FILE_SCHEME: &str = "fvoci-file://";

/// The scheme a person is named by, in a mention.
const USER_SCHEME: &str = "fvoci-user://";

/// How deep Markdown may nest.
const MAX_DEPTH: usize = 16;

/// Names files, people and blocks by their identifiers, as [`write`] does; any other
/// target is a link, or text where its scheme is not one a link may have.
struct Targets;

impl Targets {
    fn named<'d>(dest: &'d str, scheme: &str) -> Option<&'d str> {
        let id = dest.trim().strip_prefix(scheme)?;
        (!id.is_empty() && !id.contains(['/', '?', '#'])).then_some(id)
    }
}

impl AssetResolver for Targets {
    fn resolve_image(&mut self, dest: &str) -> ResolvedTarget {
        match Self::named(dest, FILE_SCHEME) {
            Some(id) => ResolvedTarget::LocalImage {
                file_id: id.to_owned(),
            },
            None => ExternalOnlyResolver.resolve_image(dest),
        }
    }

    fn resolve_link(&mut self, dest: &str) -> ResolvedTarget {
        if let Some(id) = Self::named(dest, FILE_SCHEME) {
            return ResolvedTarget::LocalFile {
                file_id: id.to_owned(),
            };
        }
        if let Some(id) = Self::named(dest, USER_SCHEME).filter(|id| is_id(id)) {
            return ResolvedTarget::Mention {
                user_id: id.to_owned(),
            };
        }
        if is_id(dest) {
            return ResolvedTarget::InternalReference {
                block_id: dest.to_owned(),
            };
        }
        ExternalOnlyResolver.resolve_link(dest)
    }
}

/// Reads Markdown into the top-level nodes of a body; 400 or 413 at `field`.
fn markdown_nodes(markdown: &str, field: &str) -> Result<Vec<Value>, AppError> {
    Ok(fvoci_markdown_to_blocks(markdown, MAX_DEPTH, &mut Targets)
        .map_err(|error| error.with_field(field))?
        .nodes)
}

/// A body in Markdown ([`App::body_in_markdown`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyInMarkdown {
    /// The body, written as Markdown.
    pub markdown: String,
    /// The blocks the Markdown does not hold as they are, in the order of the body: a
    /// body that has any is not written whole from its Markdown
    /// (`complex_blocks_use_append`).
    pub complex_blocks: Vec<ComplexBlock>,
}

/// Reads Markdown into a body, `None` for Markdown that says nothing.
///
/// # Errors
///
/// As [`markdown_nodes`], and 400 or 413 at `field` for a body that is none.
fn markdown_body(markdown: &str, field: &str) -> Result<Option<Body>, AppError> {
    let nodes = markdown_nodes(markdown, field)?;
    blocks_to_body(nodes).map_err(|error| error.with_field(field))
}

/// The text of a comment, read of `markdown`, as a comment's body is written.
///
/// # Errors
///
/// Returns 400 or 413 at `field` for Markdown that is no body, and 400 at `field` for
/// Markdown that says nothing, as for a comment sent without its text.
pub(crate) fn comment_text_of_markdown(markdown: &str, field: &str) -> Result<Body, AppError> {
    markdown_body(markdown, field)?.ok_or_else(|| {
        let mut refusals = Refusals::new();
        refusals.refuse(field, RefusalReason::Required, "a comment has a text");
        refusals.into()
    })
}

/// The identifiers a document names, of files and of people.
fn named_in(document: &Value, files: &mut BTreeSet<String>, users: &mut BTreeSet<String>) {
    let attr = |name: &str| {
        document
            .get("attrs")
            .and_then(|attrs| attrs.get(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    match document.get("type").and_then(Value::as_str) {
        Some("file") => files.extend(attr("fileId")),
        Some("mention") => users.extend(attr("userId")),
        _ => {}
    }
    for child in document
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        named_in(child, files, users);
    }
}

/// The body `markdown` writes whole over `current`, the body of a card: Markdown
/// that says nothing takes the body away.
///
/// # Errors
///
/// Returns 409 `complex_blocks_use_append` if `current` has blocks its Markdown
/// does not hold as they are, which the new body would lose, and 400 or 413 at
/// `/bodyMarkdown` for Markdown that is no body.
pub fn body_of_markdown(current: Option<&Body>, markdown: &str) -> Result<BodyChange, AppError> {
    if let Some(current) = current {
        // What the Markdown does not hold is the same whatever it names things by.
        if !body_to_markdown(current.document(), &Names::default())
            .complex
            .is_empty()
        {
            return Err(AppError::conflict(
                "the body has blocks its Markdown does not hold: add Markdown after it",
            )
            .with_reason(Reason::ComplexBlocksUseAppend));
        }
    }
    Ok(match markdown_body(markdown, "bodyMarkdown")? {
        Some(body) => BodyChange::Set(body),
        None => BodyChange::Clear,
    })
}

/// Who reads a body in Markdown: the people it names are named as they see them in the
/// workspace, and those they do not see, or who are no account, by no name.
#[derive(Debug, Clone, Copy)]
pub struct MarkdownReader<'a> {
    /// The user, by a session or through a token of theirs.
    pub user_id: &'a str,
    /// The workspace the card is read in.
    pub workspace_id: &'a str,
    /// What the user is there.
    pub role: WorkspaceRole,
}

impl App {
    /// What the Markdown of a body of the board `container_id` calls the files and
    /// the people it names, for `reader`: the files by their names on the board, the
    /// people by the names they are shown by to the reader in the workspace. Whom the
    /// reader does not see there (another organization's account, an identifier of
    /// nobody) has no name, and is told of by the identifier the body holds.
    async fn names_in(
        &self,
        reader: MarkdownReader<'_>,
        container_id: &str,
        document: &Value,
    ) -> Result<Names, AppError> {
        let (mut files, mut users) = (BTreeSet::new(), BTreeSet::new());
        named_in(document, &mut files, &mut users);
        let mut names = Names::default();
        for file_id in files {
            if let Some(record) = self.file_record(container_id, &file_id).await? {
                names.files.insert(file_id, record.name);
            }
        }
        let users: Vec<String> = users.into_iter().filter(|id| is_id(id)).collect();
        if !users.is_empty() {
            let found = self
                .users_in_workspace(reader.user_id, reader.workspace_id, reader.role, &users)
                .await?;
            names.users = found
                .into_iter()
                .map(|user| {
                    let shown = if user.display_name.is_empty() {
                        user.username
                    } else {
                        user.display_name
                    };
                    (user.id, shown)
                })
                .collect::<HashMap<_, _>>();
        }
        Ok(names)
    }

    /// `body`, a body of the board `container_id` (a card's, a page's, a unit's or a
    /// comment's), in Markdown as `reader` reads it, with the blocks the Markdown does
    /// not hold as they are; no body is empty Markdown.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a file or a person cannot be read.
    pub async fn body_in_markdown(
        &self,
        reader: MarkdownReader<'_>,
        container_id: &str,
        body: Option<&Body>,
    ) -> Result<BodyInMarkdown, AppError> {
        let written = match body {
            Some(body) => {
                let names = self.names_in(reader, container_id, body.document()).await?;
                body_to_markdown(body.document(), &names)
            }
            None => body_to_markdown(&Value::Null, &Names::default()),
        };
        Ok(BodyInMarkdown {
            markdown: written.markdown,
            complex_blocks: written.complex,
        })
    }

    /// Gives `card` its body in Markdown as `reader` reads it, in place of the body,
    /// with the blocks the Markdown does not hold as they are.
    ///
    /// # Errors
    ///
    /// Returns the store's error if a file or a person cannot be read.
    pub async fn put_card_in_markdown(
        &self,
        reader: MarkdownReader<'_>,
        card: &mut Card,
    ) -> Result<(), AppError> {
        let body = card.body.take();
        let written = self
            .body_in_markdown(reader, &card.board_id, body.as_ref())
            .await?;
        card.has_complex_blocks = Some(!written.complex_blocks.is_empty());
        card.complex_blocks = Some(written.complex_blocks);
        card.body_markdown = Some(written.markdown);
        Ok(())
    }

    /// Adds `markdown` after the last of the body of the block `block_id` of the board
    /// `container_id` (a card), by `caller`: read into a body's nodes, whose files must
    /// be the board's, and written into the block's document as one more editor writes
    /// ([`crate::collab::Collab::append`]), never over what its editors typed. The
    /// caller may change the block, which the route checked.
    ///
    /// # Errors
    ///
    /// Returns 400 at `/markdown` for Markdown that adds nothing, nests too deep or
    /// names a file the board does not have; 413 for a body or document that would be
    /// larger than it may; and the errors of [`crate::collab::Collab::append`].
    pub async fn append_markdown(
        &self,
        caller: &Caller,
        block_id: &str,
        container_id: &str,
        markdown: &str,
    ) -> Result<(), AppError> {
        let nodes = self.markdown_to_append(container_id, markdown).await?;
        let by = Appender {
            user_id: &caller.user_id,
            agent: caller.agent(),
        };
        self.collab().append(self, block_id, by, &nodes).await
    }

    /// The nodes `markdown` adds after the last of a body of the container
    /// `container_id`, checked as [`Self::append_markdown`] checks them before it
    /// writes: so that what is to be added after other changes is refused before them.
    ///
    /// # Errors
    ///
    /// Returns 400 at `/markdown` for Markdown that adds nothing, nests too deep or
    /// names a file the container does not have, and 413 for a body that would be
    /// larger than it may.
    pub async fn markdown_to_append(
        &self,
        container_id: &str,
        markdown: &str,
    ) -> Result<Vec<Value>, AppError> {
        let nodes = markdown_nodes(markdown, "markdown")?;
        let Some(added) =
            blocks_to_body(nodes.clone()).map_err(|error| error.with_field("markdown"))?
        else {
            let mut refusals = Refusals::new();
            refusals.refuse(
                "markdown",
                RefusalReason::Required,
                "the Markdown adds nothing",
            );
            return Err(refusals.into());
        };
        self.check_body_files(container_id, Some(&added))
            .await
            .map_err(|error| error.moved("/body", "/markdown"))?;
        Ok(nodes)
    }
}
