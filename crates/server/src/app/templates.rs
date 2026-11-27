//! The built-in template boards.
//!
//! The templates are the files of this crate's `assets/templates/`, embedded in the
//! binary: each template once in every language the server writes in, with the pictures
//! and documents its bodies show (`assets/templates/files/`). They are stored once,
//! when a database has none or an older version of them.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use fvoci_model::{
    Block, BlockType, Container, ContainerKind, ContainerVisibility, ContainersAndBlocks,
    CourseScheduleUpdate, FileRecord, TEMPLATE_SCHEDULE_PROPERTY, WorkspaceRole,
    renew_containers_and_blocks_ids,
};
use fvoci_store::{CopyChanges, SYSTEM_ORG_ID, SYSTEM_WORKSPACE_ID, Tenant};
use serde::Deserialize;
use serde_json::{Value, json};

use super::App;
use super::files::{FilePlace, file_record};
use super::language::Language;
use super::loose_objects::WrittenPlace;
use crate::error::AppError;

/// The current version of the built-in templates. Raising it stores them again.
const DEFAULT_TEMPLATE_VERSION: i64 = 13;

/// The longest the storing of one file of the templates may take. The files are a few
/// kilobytes each, and a server does not serve, nor a request for a first board that
/// finds no template answer, before the storing has ended one way or the other: a file
/// storage that does not answer holds either for this long, not for as long as an
/// upload may take.
const TEMPLATE_FILE_TIMEOUT: Duration = Duration::from_secs(15);

/// The board property that marks the built-in template a new account's first board is
/// copied from: the welcome board, once in each language.
///
/// A property alone proves nothing, since any user sets the properties of their own
/// boards; see [`is_onboarding_template`].
pub(crate) const ONBOARDING_PROPERTY: &str = "onboarding";

/// The board property that names a built-in template, as its file does (`key`). It
/// stays when the title changes. Any user sets the properties of their own boards, so it
/// names a template only together with `isBuiltin`.
const TEMPLATE_KEY_PROPERTY: &str = "templateKey";

/// The board property that says which language a built-in template is written in: a
/// tag of [`Language`], as its file gives it (`language`).
const TEMPLATE_LANGUAGE_PROPERTY: &str = "templateLanguage";

/// The board property that says which day a built-in template was written for, as its
/// file gives it (`baseDate`, `YYYY-MM-DD`): the days of the template are so many days
/// before or after it, and a copy has them as many days before or after the day it is
/// made on.
const TEMPLATE_BASE_DATE_PROPERTY: &str = "templateBaseDate";

/// The properties that say of a board what only a built-in template is: a copy, which
/// is somebody's own board, is made without them.
pub(super) const TEMPLATE_MARKS: [&str; 5] = [
    ONBOARDING_PROPERTY,
    TEMPLATE_KEY_PROPERTY,
    TEMPLATE_LANGUAGE_PROPERTY,
    TEMPLATE_BASE_DATE_PROPERTY,
    TEMPLATE_SCHEDULE_PROPERTY,
];

/// The files of `assets/templates/` under `$folder` that are named, each with what it
/// holds: as text (`text`) or as bytes (`bytes`).
macro_rules! embedded {
    (@text $path:expr) => {
        include_str!($path)
    };
    (@bytes $path:expr) => {
        include_bytes!($path).as_slice()
    };
    ($held:tt, $folder:literal, [$($name:literal),* $(,)?]) => {
        [$((
            $name,
            embedded!(@$held concat!("../../assets/templates/", $folder, $name)),
        )),*]
    };
}

/// The `templateKey` of the built-in welcome board for a personal space (`OrgKind::Personal`).
pub(crate) const PERSONAL_ONBOARDING_KEY: &str = "welcome-personal";

/// The template files of `assets/templates/`, by name, in the order of their names:
/// the order the templates are stored and offered in.
const TEMPLATE_FILES: [(&str, &str); 18] = embedded!(
    text,
    "",
    [
        "01-welcome.en.json",
        "01-welcome.ko.json",
        "01-welcome-personal.en.json",
        "01-welcome-personal.ko.json",
        "02-project-schedule.en.json",
        "02-project-schedule.ko.json",
        "03-issue-tracker.en.json",
        "03-issue-tracker.ko.json",
        "04-meeting-notes.en.json",
        "04-meeting-notes.ko.json",
        "05-content-calendar.en.json",
        "05-content-calendar.ko.json",
        "06-customer-contacts.en.json",
        "06-customer-contacts.ko.json",
        "07-study-notes.en.json",
        "07-study-notes.ko.json",
        "08-course-study-plan.en.json",
        "08-course-study-plan.ko.json",
    ]
);

/// The pictures and documents of `assets/templates/files/` that the templates' bodies
/// show, by name.
const TEMPLATE_ASSETS: [(&str, &[u8]); 7] = embedded!(
    bytes,
    "files/",
    [
        "agenda.en.docx",
        "agenda.ko.docx",
        "board.png",
        "chart.png",
        "timeline.png",
        "viewer-sample.en.docx",
        "viewer-sample.ko.docx",
    ]
);

/// One template file of `assets/templates/`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateFile {
    /// A name that stays when the title changes, the same in every language.
    key: String,
    /// The tag of the language the template is written in.
    language: String,
    /// The day the template is written for: what its days are counted from.
    #[serde(rename = "baseDate")]
    base_date: String,
    /// Whether this is the board a new account starts on; one template has it.
    onboarding: bool,
    board: TemplateBoard,
    /// Optional shared course schedule (only for `kind: "course"` templates).
    #[serde(default)]
    schedule: Option<TemplateSchedule>,
    #[serde(default)]
    views: Vec<Block>,
    #[serde(default)]
    cards: Vec<Block>,
    #[serde(default)]
    chapters: Vec<Block>,
    #[serde(default)]
    units: Vec<Block>,
    /// The files the bodies show.
    files: Vec<TemplateAsset>,
}

/// Example shared schedule embedded in a built-in course template.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TemplateSchedule {
    start_date: String,
    target_date: String,
    study_days: i32,
}

/// The board of a template file: what a template says of its board. The rest is the
/// server's to set.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TemplateBoard {
    id: String,
    #[serde(default)]
    kind: ContainerKind,
    title: String,
    description: String,
    icon: String,
    show_description: bool,
    #[serde(default)]
    card_properties: Vec<fvoci_model::CardPropertyDefinition>,
    #[serde(default)]
    due_date_property_id: Option<String>,
    #[serde(default)]
    done_when: Option<fvoci_model::DoneWhen>,
}

/// A file that the bodies of a template show.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateAsset {
    /// Its name in `assets/templates/files/`, which is what the bodies name it by.
    asset: String,
    /// The name it is shown under.
    name: String,
}

/// The built-in templates as boards and blocks with new identifiers, ready to be
/// stored for the system, and the files their bodies show.
struct BuiltinTemplates {
    boards_and_blocks: ContainersAndBlocks,
    files: Vec<StoredAsset>,
}

/// A file of a built-in template: its record, at a place of its board, and its bytes.
struct StoredAsset {
    record: FileRecord,
    bytes: &'static [u8],
}

/// Why the built-in templates were not stored.
#[derive(Debug)]
pub(super) enum NotStored {
    /// The file storage did not take a file of theirs. It may another time.
    Files(AppError),
    /// Anything else: the database, or a mistake in the embedded files.
    Other(AppError),
}

impl From<NotStored> for AppError {
    fn from(not_stored: NotStored) -> Self {
        match not_stored {
            NotStored::Files(error) | NotStored::Other(error) => error,
        }
    }
}

/// A mistake in the embedded template `name`, which the tests catch.
fn mistake(name: &str, what: impl std::fmt::Display) -> AppError {
    AppError::from(anyhow::anyhow!("the built-in template {name} {what}"))
}

/// The built-in templates with new identifiers, in `workspace_id`, their bodies naming
/// the places their files are to be written at.
///
/// # Errors
///
/// Returns a server error if a template file is not of the shape of [`TemplateFile`],
/// is written in no language of [`Language`], or shows a file otherwise than it says
/// in `files`: mistakes in the embedded files, which the tests catch.
fn builtin_templates(workspace_id: &str) -> Result<BuiltinTemplates, AppError> {
    let mut templates = BuiltinTemplates {
        boards_and_blocks: ContainersAndBlocks::default(),
        files: Vec::new(),
    };
    for (name, text) in TEMPLATE_FILES {
        let template: TemplateFile = serde_json::from_str(text).map_err(|error| {
            AppError::from(
                anyhow::Error::new(error).context(format!("the built-in template {name}")),
            )
        })?;
        let assets = template.files;
        let board = template_board(name, workspace_id, template.board, &template.key)?;
        let mut board = in_language(board, &template.language)
            .ok_or_else(|| mistake(name, "is written in no language of the server"))?;
        if template.onboarding {
            board
                .properties
                .insert(ONBOARDING_PROPERTY.to_owned(), Value::Bool(true));
        }
        if fvoci_model::days_between(&template.base_date, &template.base_date).is_none() {
            return Err(mistake(name, "is written for no day of the calendar"));
        }
        board.properties.insert(
            TEMPLATE_BASE_DATE_PROPERTY.to_owned(),
            Value::String(template.base_date),
        );
        let blocks = template_blocks_for_kind(
            name,
            &mut board,
            template.views,
            template.cards,
            template.schedule,
            template.chapters,
            template.units,
        )?;

        // Each file by itself: a template has the identifiers of its file in every
        // language, and what is stored has its own.
        let mut renewed = renew_containers_and_blocks_ids(ContainersAndBlocks {
            containers: vec![board],
            blocks,
        });
        let [board] = renewed.containers.as_slice() else {
            return Err(mistake(
                name,
                "is not one board once its identifiers are new",
            ));
        };
        let placed = place_assets(name, &board.id, &assets, renewed.blocks.iter_mut())?;
        templates.files.extend(placed);
        templates
            .boards_and_blocks
            .containers
            .append(&mut renewed.containers);
        templates
            .boards_and_blocks
            .blocks
            .append(&mut renewed.blocks);
    }
    Ok(templates)
}

/// Builds the blocks for a built-in template according to its container kind and stores
/// any validated example course schedule on the template container.
fn template_blocks_for_kind(
    name: &str,
    board: &mut Container,
    views: Vec<Block>,
    cards: Vec<Block>,
    schedule: Option<TemplateSchedule>,
    chapters: Vec<Block>,
    units: Vec<Block>,
) -> Result<Vec<Block>, AppError> {
    match board.kind {
        ContainerKind::Board => {
            if !chapters.is_empty() || !units.is_empty() || schedule.is_some() {
                return Err(mistake(name, "mixes course fields into a board template"));
            }
            template_blocks(name, &board.id, views, cards)
        }
        ContainerKind::Course => {
            if !views.is_empty() || !cards.is_empty() {
                return Err(mistake(name, "mixes board blocks into a course template"));
            }
            if let Some(schedule) = schedule {
                let update = CourseScheduleUpdate {
                    start_date: schedule.start_date,
                    target_date: schedule.target_date,
                    study_days: schedule.study_days,
                    auto_assign: false,
                };
                let validated = update.check_request().map_err(|error| {
                    mistake(name, format_args!("has an invalid schedule: {error}"))
                })?;
                board.properties.insert(
                    TEMPLATE_SCHEDULE_PROPERTY.to_owned(),
                    json!({
                        "startDate": validated.start_date.to_string(),
                        "targetDate": validated.target_date.to_string(),
                        "studyDays": validated.study_days.bits(),
                    }),
                );
            }
            template_course_blocks(name, &board.id, chapters, units)
        }
        other => Err(mistake(
            name,
            format_args!("has unsupported template kind {}", other.as_str()),
        )),
    }
}

/// The board of the template `name`, as a built-in template of `workspace_id` that
/// carries its `key`.
fn template_board(
    name: &str,
    workspace_id: &str,
    board: TemplateBoard,
    key: &str,
) -> Result<Container, AppError> {
    if key.is_empty() {
        return Err(mistake(name, "has no key"));
    }
    let TemplateBoard {
        id,
        kind,
        title,
        description,
        icon,
        show_description,
        card_properties,
        due_date_property_id,
        done_when,
    } = board;
    let mut board = Container {
        id,
        workspace_id: workspace_id.to_owned(),
        kind,
        title,
        description,
        icon,
        show_description,
        card_properties,
        due_date_property_id,
        done_when,
        is_template: true,
        template_version: DEFAULT_TEMPLATE_VERSION,
        visibility: ContainerVisibility::Open,
        ..Container::default()
    };
    board.properties.insert(
        TEMPLATE_KEY_PROPERTY.to_owned(),
        Value::String(key.to_owned()),
    );
    Ok(board)
}

/// The board with the language it is written in: `tag`, which has to be exactly a tag
/// of [`Language`]. `None` for any other tag.
fn in_language(mut board: Container, tag: &str) -> Option<Container> {
    let language = Language::written_in(tag).filter(|language| language.tag() == tag)?;
    board.properties.insert(
        TEMPLATE_LANGUAGE_PROPERTY.to_owned(),
        Value::String(language.tag().to_owned()),
    );
    Some(board)
}

/// The views and then the cards of the template `name`, as blocks directly on the
/// board `board_id`.
///
/// # Errors
///
/// Returns a server error if a view is no view or a card no card.
fn template_blocks(
    name: &str,
    board_id: &str,
    views: Vec<Block>,
    cards: Vec<Block>,
) -> Result<Vec<Block>, AppError> {
    let mut blocks = Vec::with_capacity(views.len() + cards.len());
    let views = views.into_iter().map(|view| (view, BlockType::View));
    let cards = cards.into_iter().map(|card| (card, BlockType::Card));
    for (mut block, block_type) in views.chain(cards) {
        if block.block_type != block_type {
            return Err(mistake(
                name,
                format_args!(
                    "has a {} among its {}s",
                    block.block_type.as_str(),
                    block_type.as_str()
                ),
            ));
        }
        board_id.clone_into(&mut block.container_id);
        // Directly on the board: no parent.
        block.parent_id.clear();
        blocks.push(block);
    }
    Ok(blocks)
}

/// The chapters and then the units of a built-in course template `name`.
fn template_course_blocks(
    name: &str,
    course_id: &str,
    chapters: Vec<Block>,
    units: Vec<Block>,
) -> Result<Vec<Block>, AppError> {
    let mut chapter_ids = HashSet::with_capacity(chapters.len());
    let mut blocks = Vec::with_capacity(chapters.len() + units.len());
    for mut chapter in chapters {
        if chapter.block_type != BlockType::Chapter {
            return Err(mistake(name, "has a non-chapter block among its chapters"));
        }
        if !chapter.parent_id.is_empty() {
            return Err(mistake(name, "has a chapter with a parentId"));
        }
        course_id.clone_into(&mut chapter.container_id);
        chapter_ids.insert(chapter.id.clone());
        chapter
            .validate()
            .map_err(|error| mistake(name, format_args!("has an invalid chapter: {error}")))?;
        blocks.push(chapter);
    }
    for mut unit in units {
        if unit.block_type != BlockType::Unit {
            return Err(mistake(name, "has a non-unit block among its units"));
        }
        if !unit.parent_id.is_empty() && !chapter_ids.contains(&unit.parent_id) {
            return Err(mistake(name, "has a unit whose parentId is not a chapter"));
        }
        course_id.clone_into(&mut unit.container_id);
        unit.validate()
            .map_err(|error| mistake(name, format_args!("has an invalid unit: {error}")))?;
        blocks.push(unit);
    }
    Ok(blocks)
}

/// Gives each of the `assets` of the template `name` a place among the files of its
/// board, and points the bodies of the board's `blocks` at those places.
///
/// # Errors
///
/// Returns a server error for an asset that is not embedded or that no body shows, and
/// for a body that shows a file the template does not name.
fn place_assets<'b>(
    name: &str,
    board_id: &str,
    assets: &[TemplateAsset],
    blocks: impl Iterator<Item = &'b mut Block>,
) -> Result<Vec<StoredAsset>, AppError> {
    let mut stored = Vec::with_capacity(assets.len());
    let mut stored_names: HashMap<&str, String> = HashMap::with_capacity(assets.len());
    for asset in assets {
        let bytes = TEMPLATE_ASSETS
            .iter()
            .find_map(|(embedded, bytes)| (*embedded == asset.asset).then_some(*bytes))
            .ok_or_else(|| {
                mistake(
                    name,
                    format_args!("names the file {}, which is not embedded", asset.asset),
                )
            })?;
        let place = FilePlace::new(SYSTEM_ORG_ID, board_id, &asset.name);
        let size = u64::try_from(bytes.len()).map_err(|_| {
            mistake(
                name,
                format_args!("names the file {}, which is too large", asset.asset),
            )
        })?;
        let record = file_record(board_id, place, size, "");
        if stored_names
            .insert(&asset.asset, record.stored_name.clone())
            .is_some()
        {
            return Err(mistake(
                name,
                format_args!("names the file {} twice", asset.asset),
            ));
        }
        stored.push(StoredAsset { record, bytes });
    }

    let mut shown: HashSet<&str> = HashSet::with_capacity(assets.len());
    for block in blocks {
        let Some(body) = block.body.take() else {
            continue;
        };
        for file in body.file_ids() {
            let Some((asset, _)) = stored_names.get_key_value(file) else {
                return Err(mistake(
                    name,
                    format_args!("shows the file {file}, which it does not name"),
                ));
            };
            shown.insert(asset);
        }
        block.body = body.with_files_renamed(|asset| stored_names.get(asset).cloned());
    }
    if let Some(unused) = assets
        .iter()
        .find(|asset| !shown.contains(asset.asset.as_str()))
    {
        return Err(mistake(
            name,
            format_args!("names the file {}, which no body shows", unused.asset),
        ));
    }
    Ok(stored)
}

impl App {
    /// Stores the built-in templates if the database has none, an older version, or
    /// no welcome board in some language. Returns whether they were stored.
    ///
    /// Their files are written first, at marked places, and recorded in the transaction
    /// that stores the templates: a server that stops in between leaves marks that the
    /// housekeeping removes with their objects, and no template whose file is missing.
    ///
    /// # Errors
    ///
    /// Returns a server error if a file cannot be written, and the store's error if the
    /// templates cannot be read, removed or stored.
    pub async fn init_templates(&self) -> Result<bool, AppError> {
        Ok(self.try_init_templates().await?)
    }

    /// [`App::init_templates`], saying of a failure whether it was the file storage's.
    /// Templates that are stored, or found current, are owed no more.
    pub(super) async fn try_init_templates(&self) -> Result<bool, NotStored> {
        let stored = self.store_templates_if_needed().await?;
        self.inner.template_stock.settle();
        Ok(stored)
    }

    async fn store_templates_if_needed(&self) -> Result<bool, NotStored> {
        let other = |error: AppError| NotStored::Other(error);
        // The built-in templates are the system organization's, in its workspace:
        // offered to every organization, and nobody's to change.
        let system = self.scoped(Tenant::org(SYSTEM_ORG_ID).map_err(|error| other(error.into()))?);
        let stored = system
            .store()
            .get_builtin_templates()
            .await
            .map_err(|error| other(error.into()))?;
        if initialization_reason(&stored).is_none() {
            tracing::debug!("Template import not needed, skipping");
            return Ok(false);
        }

        let BuiltinTemplates {
            boards_and_blocks: mut templates,
            files,
        } = builtin_templates(SYSTEM_WORKSPACE_ID).map_err(other)?;
        let written_places = system.write_template_files(&files).await?;
        let records: Vec<FileRecord> = files.iter().map(|file| file.record.clone()).collect();
        // One transaction that decides again: of two servers, or requests for a first
        // board, that found the templates missing, one stores them. What is stored on
        // behalf of nobody is the system's own.
        let replaced = system
            .store()
            .replace_builtin_templates(&mut templates, &records, |stored| {
                initialization_reason(stored)
                    .inspect(|reason| tracing::debug!(reason, "Storing the built-in templates"))
                    .is_some()
            })
            .await;
        match replaced {
            // The records of the old templates' files went with them, and their
            // objects are doomed: the housekeeping removes them.
            Ok(Some(_)) => {
                for place in written_places {
                    place.settled_in_transaction();
                }
                system.announce_created(&templates, &[]).await;
                Ok(true)
            }
            // Every write is over: what was written is removed now.
            Ok(None) => {
                give_up_written_places(written_places).await;
                Ok(false)
            }
            Err(error) => {
                give_up_written_places(written_places).await;
                Err(other(
                    AppError::from(error).context("cannot store the built-in templates"),
                ))
            }
        }
    }

    /// Writes the files of the templates, each at its place, marked as being written.
    async fn write_template_files(
        &self,
        files: &[StoredAsset],
    ) -> Result<Vec<WrittenPlace>, NotStored> {
        let mut written = Vec::with_capacity(files.len());
        for file in files {
            let path = &file.record.path;
            let marked = match self.mark_place(path).await {
                Ok(marked) => marked,
                Err(error) => {
                    give_up_written_places(written).await;
                    return Err(NotStored::Other(error));
                }
            };
            let mut bytes = file.bytes;
            match marked.write_within(&mut bytes, TEMPLATE_FILE_TIMEOUT).await {
                Ok(place) => written.push(place),
                Err(Some(error)) => {
                    give_up_written_places(written).await;
                    return Err(NotStored::Files(AppError::internal(format_args!(
                        "unable to store a file of the built-in templates at {path}: {error}"
                    ))));
                }
                Err(None) => {
                    give_up_written_places(written).await;
                    return Err(NotStored::Files(AppError::internal(format_args!(
                        "storing a file of the built-in templates at {path} took longer than {} seconds",
                        TEMPLATE_FILE_TIMEOUT.as_secs()
                    ))));
                }
            }
        }
        Ok(written)
    }

    /// Returns the template boards of a workspace, for the templates route: for an
    /// administrator of the workspace (`role`) every one of it, for anybody else those
    /// that are open or that the user is a member of; and the built-in ones that are
    /// written in the language the server writes in for the user
    /// (`App::language_for`, by `accept_language`, the request's `Accept-Language`).
    ///
    /// # Errors
    ///
    /// Returns the store's error if a query fails.
    pub async fn template_boards(
        &self,
        workspace_id: &str,
        user_id: &str,
        role: WorkspaceRole,
        accept_language: Option<&str>,
    ) -> Result<Vec<Container>, AppError> {
        let language = self.language_for(user_id, accept_language).await?;
        // What is owed is not waited for: the list has what the database holds.
        self.restock_templates_soon();
        let mut boards = if role == WorkspaceRole::Admin {
            self.store()
                .get_templates_of_workspace(workspace_id)
                .await?
        } else {
            self.store()
                .get_template_boards(workspace_id, user_id)
                .await?
        };
        let builtin = self.store().get_builtin_templates().await?;
        boards.extend(
            builtin
                .into_iter()
                .filter(|board| written_in(board) == Some(language)),
        );
        Ok(boards)
    }
}

async fn give_up_written_places(places: Vec<WrittenPlace>) {
    for place in places {
        place.give_up().await;
    }
}

/// Returns whether a board is a built-in template that a new account's first board is
/// copied from: a template of the system organization that the system created and
/// marked.
///
/// The system's authorship is what cannot be forged. A board's creator is always the
/// session's user, on every route that makes a board; nobody signs in as the system.
pub(crate) fn is_onboarding_template(board: &Container) -> bool {
    board.is_template
        && board.is_builtin
        && board.properties.get(ONBOARDING_PROPERTY) == Some(&Value::Bool(true))
}

/// Returns whether `board` is the built-in personal-space welcome template (`welcome-personal`).
pub(crate) fn is_personal_onboarding_template(board: &Container) -> bool {
    board.is_template
        && board.is_builtin
        && board
            .properties
            .get(TEMPLATE_KEY_PROPERTY)
            .and_then(Value::as_str)
            == Some(PERSONAL_ONBOARDING_KEY)
}

/// What a copy of `source` made on the day `today` (`YYYY-MM-DD`) does not take from it
/// as it stands. Of a built-in template, the marks that say it is one, and its days:
/// they are moved by as many days as `today` is after the day the template was written
/// for, and stay where `today` is unknown. Of any other board, nothing.
pub(super) fn changes_of_copy(source: &Container, today: Option<&str>) -> CopyChanges<'static> {
    if !(source.is_template && source.is_builtin) {
        return CopyChanges::default();
    }
    let written_for = source
        .properties
        .get(TEMPLATE_BASE_DATE_PROPERTY)
        .and_then(Value::as_str);
    let days_later = written_for
        .zip(today)
        .and_then(|(written_for, today)| fvoci_model::days_between(written_for, today))
        .unwrap_or_default();
    CopyChanges {
        without_properties: &TEMPLATE_MARKS,
        days_later,
    }
}

/// The language a built-in template says it is written in.
pub(crate) fn written_in(board: &Container) -> Option<Language> {
    board
        .properties
        .get(TEMPLATE_LANGUAGE_PROPERTY)
        .and_then(Value::as_str)
        .and_then(Language::written_in)
}

/// Why the templates must be imported, or `None` if they are current. The templates are
/// needed when there are none, when a built-in one has an older version, or when a
/// language has no welcome board among them.
fn initialization_reason(boards: &[Container]) -> Option<&'static str> {
    if boards.is_empty() {
        return Some("no default templates found");
    }
    for board in boards {
        if board.template_version < DEFAULT_TEMPLATE_VERSION {
            return Some("template_version too old");
        }
    }
    let welcomes = |language: &Language| {
        boards
            .iter()
            .any(|board| is_onboarding_template(board) && written_in(board) == Some(*language))
    };
    if !Language::ALL.iter().all(welcomes) {
        return Some("a language has no welcome board");
    }
    None
}

#[cfg(test)]
mod tests;
