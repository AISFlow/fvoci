//! Exporting a board, a wiki, a course or a diary as Markdown files in a ZIP
//! (`exportContainerMarkdown`), for another tool to read: a folder of the container's
//! name, a `.md` file for each page, card, unit or entry where [`layout`] puts it, a
//! board's cards also as a table (`board.csv`), and the files their bodies and a card's
//! attachments show in `files/`.
//!
//! A file starts with its front matter (its title, when it was made and last changed,
//! and what else it is: a card's number and attachments, a unit's amount and day, an
//! entry's day, mood, weather and place), then its title as a heading and its body in
//! Markdown as the reader reads it (`body_in_exported_markdown`): a file, a page, card
//! or unit the export holds is linked where it is, a person is named in plain text, and
//! what else a reference names is its title where the reader reads it, `[[id]]` where
//! not. Comments, the trash, the versions of bodies and what no reader of the export
//! would read as Markdown (a board's views) are not in it.

mod board_csv;
mod layout;
mod names;

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use fvoci_model::{
    Block, BlockType, Card, Container, ContainerKind, ENTRY_DATE_FIELD, ENTRY_MOOD_FIELD,
    ENTRY_PLACE_FIELD, ENTRY_WEATHER_FIELD, FileRecord, PAGE_ICON_FIELD, PropertyType,
    UNIT_AMOUNT_FIELD, UNIT_AMOUNT_UNIT_FIELD, UNIT_SCHEDULED_DATE_FIELD, is_id,
    relation_property_ids,
};
use serde_json::Value;

use self::board_csv::{CellNames, board_csv, related_cards};
use self::layout::{FILES_FOLDER, Placed, layout};
use self::names::{Folder, front_matter, name_of};
use super::App;
use super::body_markdown::{ExportPlaces, Exported, MarkdownReader, relative};
use super::export::{ArchiveWriter, ExportedArchive, is_stored_name, utc};
use super::references::MAX_REFERENCE_IDS;
use crate::error::AppError;
use crate::metrics::ContainerWork;

impl App {
    /// Builds the export of `container` in Markdown files, as `reader` reads it, in a
    /// temporary file, and returns it open (as [`App::export_archive`] does an archive).
    ///
    /// # Errors
    ///
    /// Returns the store's error if what it holds cannot be read, or a server error if
    /// the archive cannot be written.
    pub async fn export_markdown(
        &self,
        reader: MarkdownReader<'_>,
        container: &Container,
    ) -> Result<ExportedArchive, AppError> {
        self.write_container_bodies(&container.id).await?;
        let blocks = self.blocks_for_container(&container.id).await?;
        let root = vec![name_of(&container.title, kind_name(container.kind))];
        let entries = layout(container.kind, &root, &blocks);
        let attachments = attachments_of(&entries, &blocks);
        let records = self.store().get_files_of_container(&container.id).await?;
        let files = file_places(&root, &entries, &attachments, &records);
        let relations = relation_property_ids(&container.card_properties);
        let places = Arc::new(
            self.export_places(reader, &entries, &files, &relations)
                .await?,
        );

        let path = self.new_export_file("zip").await?;
        let mut writer = ArchiveWriter::create(path.path()).await?;
        for one in &entries {
            let text = self
                .exported_file(reader, container, one, &attachments, &places)
                .await?;
            writer.start_entry(&one.path().join("/")).await?;
            writer.write(text.as_bytes()).await?;
        }
        if container.kind == ContainerKind::Board {
            let table = self
                .board_table(reader, container, &entries, &records, &places)
                .await?;
            writer
                .start_entry(&format!("{}/board.csv", root[0]))
                .await?;
            writer.write(&table).await?;
        }
        for (stored, to) in &files {
            if !self
                .write_file(&mut writer, &container.id, stored, &to.join("/"))
                .await?
            {
                tracing::warn!(
                    container_id = container.id,
                    file = stored,
                    "a file a body shows is missing from the Markdown export"
                );
            }
        }
        writer.finish().await?;
        let archive = ExportedArchive::open(path).await?;
        self.metrics().container_copied(
            ContainerWork::Export,
            super::container_copies::counted_as(container.kind),
        );
        Ok(archive)
    }

    /// What the export holds and where (its files, its pages, cards and units), and the
    /// titles of what else the bodies and the cards' `relations` refer to that `reader`
    /// reads.
    async fn export_places(
        &self,
        reader: MarkdownReader<'_>,
        entries: &[Placed<'_>],
        files: &[(String, Vec<String>)],
        relations: &HashSet<&str>,
    ) -> Result<ExportPlaces, AppError> {
        let mut places = ExportPlaces {
            files: files.iter().cloned().collect(),
            ..ExportPlaces::default()
        };
        for one in entries {
            places
                .blocks
                .insert(one.block.id.clone(), (one.path(), one.title.clone()));
        }
        let mut outside = BTreeSet::new();
        for one in entries {
            if let Some(body) = &one.block.body {
                references_in(body.document(), &mut outside);
            }
            relations_in(one.block, relations, &mut outside);
        }
        outside.retain(|id| !places.blocks.contains_key(id));
        let outside: Vec<String> = outside.into_iter().collect();
        for some in outside.chunks(MAX_REFERENCE_IDS) {
            for linked in self.resolve_references(reader.user_id, some).await? {
                places.titles.insert(linked.id, linked.title);
            }
        }
        Ok(places)
    }

    /// The text of one file of the export: its front matter, its title, its body.
    async fn exported_file(
        &self,
        reader: MarkdownReader<'_>,
        container: &Container,
        one: &Placed<'_>,
        attachments: &HashMap<&str, Vec<&Block>>,
        places: &Arc<ExportPlaces>,
    ) -> Result<String, AppError> {
        let block = one.block;
        let exported = Exported {
            from: one.folder.clone(),
            places: Arc::clone(places),
        };
        let linked: Vec<String> = attachments
            .get(block.id.as_str())
            .map_or_default(|attached| {
                attached
                    .iter()
                    .filter_map(|attachment| attachment.attachment_file())
                    .filter_map(|stored| places.files.get(stored))
                    .map(|to| relative(&one.folder, to))
                    .collect()
            });
        let mut text = front_matter(&fields_of(block, &one.title), &[("attachments", linked)]);
        // A heading is one line: a separator of lines ends it as a line break does.
        let heading = one.title.replace(['\n', '\r', '\u{2028}', '\u{2029}'], " ");
        if !heading.trim().is_empty() {
            let _ = write!(text, "# {}\n\n", heading.trim());
        }
        let body = self
            .body_in_exported_markdown(reader, &container.id, block.body.as_ref(), exported)
            .await?;
        text.push_str(&body);
        if !text.ends_with('\n') {
            text.push('\n');
        }
        Ok(text)
    }

    /// `board.csv`: the cards the export holds, by number, under the board's properties.
    async fn board_table(
        &self,
        reader: MarkdownReader<'_>,
        container: &Container,
        entries: &[Placed<'_>],
        records: &[FileRecord],
        places: &ExportPlaces,
    ) -> Result<Vec<u8>, AppError> {
        let mut cards = Vec::with_capacity(entries.len());
        let mut people = BTreeSet::new();
        for one in entries {
            let card = Card::from_block(one.block, 0)
                .map_err(|error| AppError::internal(format_args!("a card is no card: {error}")))?;
            people.insert(card.created_by.clone());
            people.insert(card.modified_by.clone());
            for property in &container.card_properties {
                if matches!(
                    property.property_type,
                    PropertyType::Person | PropertyType::MultiPerson
                ) && let Some(value) = card.properties.get(&property.id)
                {
                    people.extend(ids_in(value));
                }
            }
            cards.push(card);
        }
        let people: Vec<String> = people.into_iter().collect();
        let mut names = CellNames {
            users: self.names_seen_by(reader, &people).await?,
            files: records
                .iter()
                .map(|record| (record.stored_name.clone(), record.name.clone()))
                .collect(),
            cards: places.titles.clone(),
        };
        for (id, (_, title)) in &places.blocks {
            names.cards.insert(id.clone(), title.clone());
        }
        board_csv(&container.card_properties, &cards, &names)
    }
}

/// What a file of a container of `kind` is called whose title gives no name.
fn kind_name(kind: ContainerKind) -> &'static str {
    match kind {
        ContainerKind::Board => "board",
        ContainerKind::Wiki => "wiki",
        ContainerKind::Course => "course",
        ContainerKind::Diary => "diary",
    }
}

/// The attachments of each card the export holds, by the card's identifier.
fn attachments_of<'a>(
    entries: &[Placed<'_>],
    blocks: &'a [Block],
) -> HashMap<&'a str, Vec<&'a Block>> {
    let cards: BTreeSet<&str> = entries
        .iter()
        .filter(|one| one.block.block_type == BlockType::Card)
        .map(|one| one.block.id.as_str())
        .collect();
    let mut attachments: HashMap<&str, Vec<&Block>> = HashMap::new();
    for block in blocks {
        if block.block_type == BlockType::Attachment && cards.contains(block.parent_id.as_str()) {
            attachments
                .entry(block.parent_id.as_str())
                .or_default()
                .push(block);
        }
    }
    attachments
}

/// Where each file the export holds goes, in `files/`, by its stored name: the files the
/// bodies of what it holds show, and the attachments of its cards, each once, named as
/// it was uploaded (its stored name where it has no record), one name of its own.
fn file_places(
    root: &[String],
    entries: &[Placed<'_>],
    attachments: &HashMap<&str, Vec<&Block>>,
    records: &[FileRecord],
) -> Vec<(String, Vec<String>)> {
    let uploaded: HashMap<&str, &str> = records
        .iter()
        .map(|record| (record.stored_name.as_str(), record.name.as_str()))
        .collect();
    let mut shown = Vec::new();
    for one in entries {
        shown.extend(one.block.shown_files());
        for attachment in attachments.get(one.block.id.as_str()).into_iter().flatten() {
            shown.extend(attachment.shown_files());
        }
    }
    let mut seen = BTreeSet::new();
    let mut folder = Folder::default();
    let mut files = Vec::new();
    for stored in shown {
        if !is_stored_name(&stored) || !seen.insert(stored.clone()) {
            continue;
        }
        let name = uploaded
            .get(stored.as_str())
            .copied()
            .unwrap_or(stored.as_str());
        let (stem, extension) = match name.rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() => {
                (stem, format!(".{}", name_of(extension, "bin")))
            }
            _ => (name, String::new()),
        };
        let mut path = root.to_vec();
        path.push(FILES_FOLDER.to_owned());
        path.push(folder.take(&name_of(stem, "file"), &extension));
        files.push((stored, path));
    }
    files
}

/// The front matter's fields of `block`, by its type.
fn fields_of(block: &Block, title: &str) -> Vec<(&'static str, String)> {
    let text = |field: &str| match block.fields.get(field) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        _ => String::new(),
    };
    let mut fields = vec![("title", title.to_owned())];
    match block.block_type {
        BlockType::Card => fields.push((
            "number",
            block
                .shown_number()
                .map_or_default(|number| number.to_string()),
        )),
        BlockType::Page => fields.push(("icon", text(PAGE_ICON_FIELD))),
        BlockType::Unit => fields.extend([
            ("amount", text(UNIT_AMOUNT_FIELD)),
            ("amountUnit", text(UNIT_AMOUNT_UNIT_FIELD)),
            ("scheduledDate", text(UNIT_SCHEDULED_DATE_FIELD)),
        ]),
        BlockType::Entry => fields.extend([
            ("date", text(ENTRY_DATE_FIELD)),
            ("mood", text(ENTRY_MOOD_FIELD)),
            ("weather", text(ENTRY_WEATHER_FIELD)),
            ("place", text(ENTRY_PLACE_FIELD)),
        ]),
        _ => {}
    }
    fields.extend([
        ("created", utc(block.created_at)),
        ("updated", utc(block.updated_at)),
    ]);
    fields
}

/// The identifiers the references of a body's document name; what names nothing by an
/// identifier is asked of no one.
fn references_in(node: &Value, into: &mut BTreeSet<String>) {
    if node.get("type").and_then(Value::as_str) == Some("reference")
        && let Some(id) = node.pointer("/attrs/id").and_then(Value::as_str)
        && is_id(id)
    {
        into.insert(id.to_owned());
    }
    if let Some(children) = node.get("content").and_then(Value::as_array) {
        for child in children {
            references_in(child, into);
        }
    }
}

/// The cards a card's relations name: the values of the board's `relations` alone, of
/// which an option or a file is none.
fn relations_in(block: &Block, relations: &HashSet<&str>, into: &mut BTreeSet<String>) {
    if block.block_type != BlockType::Card {
        return;
    }
    let Some(Value::Object(properties)) = block.fields.get("properties") else {
        return;
    };
    for (id, value) in properties {
        if relations.contains(id.as_str()) {
            into.extend(related_cards(value).into_iter().map(str::to_owned));
        }
    }
}

/// The identifiers a value of a property holds: itself, or those of a list.
fn ids_in(value: &Value) -> Vec<String> {
    match value {
        Value::String(id) if !id.is_empty() => vec![id.clone()],
        Value::Array(ids) => ids
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}
