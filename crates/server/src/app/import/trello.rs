//! Trello board JSON export converter.
//!
//! Converts a Trello board export JSON file into a [`StagedArchive`] containing one
//! `ContainerKind::Board`, a Kanban view grouped by Trello lists, card properties for
//! lists (`select`), labels (`multiSelect`), and due dates (`date`), cards with Markdown
//! descriptions, checklist `taskList`s, external attachment links (never fetched: SSRF
//! blocked), and `commentCard` comments whose original author is preserved solely as
//! `"가져옴: <name>"` text without linking any fvoci account.
//!
//! Closed/archived Trello lists and cards (including cards inside closed lists) are
//! consistently skipped and reported under [`ImportSummary::skipped`] with
//! [`ImportWarningCode::ClosedItemSkipped`].

use std::collections::{HashMap, HashSet};
use std::path::Path;

use fvoci_model::{
    BLOCK_TITLE_MAX_CHARS, BODY_MAX_NODES, Block, BlockType, CardPropertyDefinition, Container,
    ContainerKind, ContainerVisibility, JsonMap, PropertyColor, PropertyOption, PropertyType,
    check_date_value, new_id,
};
use serde_json::{Map, Value, json};

use super::bundle::{check_json_safety, has_url_scheme, read_bounded_file};
use super::markdown::{
    BodyContext, ExternalOnlyResolver, blocks_to_body, linked_text_node, markdown_to_blocks,
    text_heading, text_paragraph,
};
use super::markdown_nodes::BodyBudget;
use super::staged::{StagedArchive, StagedContainer};
use super::summary::{
    BundleLimits, ImportFormat, ImportSummary, ImportWarningCode, truncate_chars,
};
use super::trello_wire::{
    TRELLO_LIMIT_PREFIX, TrelloAction, TrelloAttachment, TrelloBoard, TrelloCard, TrelloCheckItem,
    TrelloChecklist, TrelloLabel, TrelloList,
};
use crate::error::{AppError, Reason};

/// Rotating option colors assigned to Trello lists.
const LIST_COLORS: [PropertyColor; 9] = [
    PropertyColor::Gray,
    PropertyColor::Blue,
    PropertyColor::Green,
    PropertyColor::Orange,
    PropertyColor::Purple,
    PropertyColor::Yellow,
    PropertyColor::Pink,
    PropertyColor::Brown,
    PropertyColor::Red,
];

/// Reads and converts a Trello JSON export file at `path` into a [`StagedArchive`].
///
/// # Errors
///
/// Returns 413 if the file or item counts exceed `limits`, or 400 if the file is not
/// valid UTF-8, exceeds JSON nesting depth, contains U+0000, or is not a Trello board JSON.
pub(crate) async fn convert_trello_file(
    path: &Path,
    limits: &BundleLimits,
) -> Result<StagedArchive, AppError> {
    let bytes = read_bounded_file(path, limits.max_trello_json_bytes).await?;
    let limits = *limits;
    tokio::task::spawn_blocking(move || convert_trello_bytes(&bytes, &limits))
        .await
        .map_err(|error| {
            AppError::internal(format_args!("trello conversion task failed: {error}"))
        })?
}

/// Converts raw Trello JSON bytes into a [`StagedArchive`].
///
/// # Errors
///
/// Returns 400 or 413 on malformed JSON, excessive nesting, NUL bytes, or exceeded limits.
pub(crate) fn convert_trello_bytes(
    bytes: &[u8],
    limits: &BundleLimits,
) -> Result<StagedArchive, AppError> {
    let board = decode_trello_board(bytes, limits)?;
    let lists = board.lists.map_or_default(|b| b.0);
    let cards = board.cards.map_or_default(|b| b.0);
    let labels = board.labels.0;
    let checklists = board.checklists.0;
    let actions = board.actions.0;
    check_trello_counts(&lists, &labels, &cards, &checklists, &actions, limits)?;

    let mut summary = ImportSummary::new(ImportFormat::Trello);
    let mut body_budget = BodyBudget::default();
    let board_id = new_id();
    let (status_prop, list_options, closed_lists, list_rank) =
        build_list_property(lists, &mut summary);
    let (labels_prop, label_options) =
        build_label_property(&labels, &cards, limits.max_trello_labels)?;
    let due_prop_id = cards
        .iter()
        .any(|card| !card.closed && (card.due.is_some() || card.start.is_some()))
        .then(new_id);

    let mut card_properties = vec![status_prop.definition];
    let mut shown_properties = Vec::new();
    if let Some((prop_id, definition)) = labels_prop {
        shown_properties.push(prop_id);
        card_properties.push(definition);
    }
    if let Some(ref due_id) = due_prop_id {
        shown_properties.push(due_id.clone());
        card_properties.push(date_property_definition(due_id, "Due"));
    }

    let checklists_by_card = group_checklists(&checklists);
    let mapping = CardMapping {
        board_id: &board_id,
        list_options: &list_options,
        closed_lists: &closed_lists,
        list_rank: &list_rank,
        status_prop_id: &status_prop.id,
        labels_map: label_options.as_ref(),
        due_prop_id: due_prop_id.as_deref(),
        checklists: &checklists_by_card,
    };
    let built_cards = build_cards(cards, &mapping, limits, &mut body_budget, &mut summary)?;

    let comment_blocks = build_comments(
        &board_id,
        actions,
        &built_cards.id_map,
        limits,
        &mut body_budget,
        &mut summary,
    )?;

    let view = build_board_view(
        &board_id,
        &status_prop.id,
        &status_prop.option_ids,
        &shown_properties,
        &built_cards.order,
    );

    let mut blocks = Vec::with_capacity(1 + built_cards.blocks.len() + comment_blocks.len());
    blocks.push(view);
    blocks.extend(built_cards.blocks);
    blocks.extend(comment_blocks);

    let container = build_trello_container(
        board_id,
        &board.name,
        &board.desc,
        card_properties,
        &mut summary,
    );

    Ok(StagedArchive {
        containers: vec![StagedContainer {
            container,
            blocks,
            files: Vec::new(),
        }],
        summary,
    })
}

fn decode_trello_board(bytes: &[u8], limits: &BundleLimits) -> Result<TrelloBoard, AppError> {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    check_json_safety(bytes, limits.max_json_nesting_depth)?;
    let board: TrelloBoard = serde_json::from_slice(bytes).map_err(|error| {
        let msg = error.to_string();
        if let Some((_, rest)) = msg.split_once(TRELLO_LIMIT_PREFIX) {
            let parsed_limit = rest
                .split(|ch: char| !ch.is_ascii_digit())
                .next()
                .and_then(|digits| digits.parse::<usize>().ok())
                .unwrap_or(usize::MAX);
            return too_many("items", parsed_limit);
        }
        AppError::bad_request(format_args!("invalid Trello board JSON: {error}"))
    })?;
    if board.lists.is_none() && board.cards.is_none() {
        return Err(AppError::bad_request(
            "JSON does not look like a Trello board export: missing `lists` and `cards`",
        ));
    }
    Ok(board)
}

fn build_trello_container(
    board_id: String,
    raw_name: &str,
    raw_desc: &str,
    card_properties: Vec<CardPropertyDefinition>,
    summary: &mut ImportSummary,
) -> Container {
    let title = normalize_title(raw_name, "Trello Board", summary);
    let description = raw_desc.trim().to_owned();
    Container {
        id: board_id,
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Board,
        title,
        show_description: !description.is_empty(),
        description,
        card_properties,
        ..Container::default()
    }
}

fn check_trello_counts(
    lists: &[TrelloList],
    labels: &[TrelloLabel],
    cards: &[TrelloCard],
    checklists: &[TrelloChecklist],
    actions: &[TrelloAction],
    limits: &BundleLimits,
) -> Result<(), AppError> {
    if lists.len() > limits.max_trello_lists {
        return Err(too_many("Trello lists", limits.max_trello_lists));
    }
    if labels.len() > limits.max_trello_labels {
        return Err(too_many("Trello labels", limits.max_trello_labels));
    }
    if cards.len() > limits.max_trello_cards {
        return Err(too_many("Trello cards", limits.max_trello_cards));
    }
    if checklists.len() > limits.max_trello_checklists {
        return Err(too_many("Trello checklists", limits.max_trello_checklists));
    }
    let check_items: usize = checklists.iter().fold(0_usize, |acc, list| {
        acc.saturating_add(list.check_items.0.len())
    });
    if check_items > limits.max_trello_check_items {
        return Err(too_many(
            "Trello checklist items",
            limits.max_trello_check_items,
        ));
    }
    let attachments: usize = cards.iter().fold(0_usize, |acc, card| {
        acc.saturating_add(card.attachments.0.len())
    });
    if attachments > limits.max_trello_attachments {
        return Err(too_many(
            "Trello attachments",
            limits.max_trello_attachments,
        ));
    }
    let comments = actions
        .iter()
        .filter(|action| action.action_type == "commentCard")
        .count();
    if comments > limits.max_trello_comments {
        return Err(too_many("Trello comments", limits.max_trello_comments));
    }
    Ok(())
}

fn too_many(what: &str, limit: usize) -> AppError {
    let limit_u64 = u64::try_from(limit).unwrap_or(u64::MAX);
    AppError::too_large(
        format_args!("Trello export has more than {limit} {what}"),
        limit_u64,
    )
    .with_reason(Reason::ArchiveTooLarge)
}

struct SelectProperty {
    id: String,
    option_ids: Vec<String>,
    definition: CardPropertyDefinition,
}

fn build_list_property(
    mut lists: Vec<TrelloList>,
    summary: &mut ImportSummary,
) -> (
    SelectProperty,
    HashMap<String, String>,
    HashSet<String>,
    HashMap<String, usize>,
) {
    lists.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    let prop_id = new_id();
    let mut options = Vec::new();
    let mut option_ids = Vec::new();
    let mut by_trello_id = HashMap::new();
    let mut closed = HashSet::new();
    let mut rank = HashMap::new();

    for list in lists {
        if list.closed {
            closed.insert(list.id);
            summary.push_skipped(
                ImportWarningCode::ClosedItemSkipped,
                &list.name,
                "archived Trello list skipped",
            );
            continue;
        }
        let opt_id = new_id();
        let color = LIST_COLORS[options.len() % LIST_COLORS.len()];
        let label = if list.name.trim().is_empty() {
            "Untitled".to_owned()
        } else {
            list.name.trim().to_owned()
        };
        rank.insert(list.id.clone(), options.len());
        by_trello_id.insert(list.id, opt_id.clone());
        option_ids.push(opt_id.clone());
        options.push(PropertyOption::new(opt_id, label, color));
    }

    let definition = CardPropertyDefinition {
        id: prop_id.clone(),
        name: "Status".to_owned(),
        property_type: PropertyType::Select,
        options,
    };
    (
        SelectProperty {
            id: prop_id,
            option_ids,
            definition,
        },
        by_trello_id,
        closed,
        rank,
    )
}

struct LabelPropertyMap {
    prop_id: String,
    by_trello_id: HashMap<String, String>,
}

type LabelPropertyBuild = (
    Option<(String, CardPropertyDefinition)>,
    Option<LabelPropertyMap>,
);

fn build_label_property(
    board_labels: &[TrelloLabel],
    cards: &[TrelloCard],
    max_labels: usize,
) -> Result<LabelPropertyBuild, AppError> {
    let mut seen_ids = HashSet::new();
    let mut ordered = Vec::new();
    for label in board_labels
        .iter()
        .chain(cards.iter().flat_map(|card| card.labels.0.iter()))
    {
        if label.id.is_empty() || !seen_ids.insert(label.id.clone()) {
            continue;
        }
        if seen_ids.len() > max_labels {
            return Err(too_many("Trello labels", max_labels));
        }
        let text = label.name.trim();
        let fallback_color = label.color.as_deref().unwrap_or("").trim();
        if text.is_empty() && fallback_color.is_empty() {
            continue;
        }
        ordered.push(label.clone());
    }
    if ordered.is_empty() {
        return Ok((None, None));
    }

    let prop_id = new_id();
    let mut options = Vec::with_capacity(ordered.len());
    let mut by_trello_id = HashMap::with_capacity(ordered.len());
    for label in ordered {
        let opt_id = new_id();
        let display = if label.name.trim().is_empty() {
            label.color.as_deref().unwrap_or("Label").trim().to_owned()
        } else {
            label.name.trim().to_owned()
        };
        let color = trello_color_to_fvoci(label.color.as_deref());
        by_trello_id.insert(label.id, opt_id.clone());
        options.push(PropertyOption::new(opt_id, display, color));
    }
    let definition = CardPropertyDefinition {
        id: prop_id.clone(),
        name: "Labels".to_owned(),
        property_type: PropertyType::MultiSelect,
        options,
    };
    Ok((
        Some((prop_id.clone(), definition)),
        Some(LabelPropertyMap {
            prop_id,
            by_trello_id,
        }),
    ))
}

fn trello_color_to_fvoci(color: Option<&str>) -> PropertyColor {
    let base = color
        .unwrap_or_default()
        .split('_')
        .next()
        .unwrap_or_default();
    match base {
        "green" | "lime" => PropertyColor::Green,
        "yellow" => PropertyColor::Yellow,
        "orange" => PropertyColor::Orange,
        "red" => PropertyColor::Red,
        "purple" => PropertyColor::Purple,
        "blue" | "sky" => PropertyColor::Blue,
        "pink" => PropertyColor::Pink,
        "black" => PropertyColor::Gray,
        _ => PropertyColor::Default,
    }
}

fn date_property_definition(prop_id: &str, name: &str) -> CardPropertyDefinition {
    CardPropertyDefinition::new(prop_id, name, PropertyType::Date)
}

struct ChecklistIndex<'a> {
    by_card: HashMap<&'a str, Vec<&'a TrelloChecklist>>,
    by_id: HashMap<&'a str, &'a TrelloChecklist>,
}

fn group_checklists(checklists: &[TrelloChecklist]) -> ChecklistIndex<'_> {
    let mut by_id: HashMap<&str, &TrelloChecklist> = HashMap::with_capacity(checklists.len());
    let mut by_card: HashMap<&str, Vec<&TrelloChecklist>> = HashMap::new();
    for checklist in checklists {
        by_id.insert(checklist.id.as_str(), checklist);
        if !checklist.id_card.is_empty() {
            by_card
                .entry(checklist.id_card.as_str())
                .or_default()
                .push(checklist);
        }
    }
    for list in by_card.values_mut() {
        list.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    }
    ChecklistIndex { by_card, by_id }
}

struct BuiltCards {
    blocks: Vec<Block>,
    id_map: HashMap<String, String>,
    order: Vec<String>,
}

/// What the cards of a Trello board are mapped with: the board they go on, its lists
/// (the status option of each, the closed ones, their order), the identifiers of the
/// properties of the status, the labels and the due date, and the checklists of each
/// card.
#[derive(Clone, Copy)]
struct CardMapping<'a> {
    board_id: &'a str,
    list_options: &'a HashMap<String, String>,
    closed_lists: &'a HashSet<String>,
    list_rank: &'a HashMap<String, usize>,
    status_prop_id: &'a str,
    labels_map: Option<&'a LabelPropertyMap>,
    due_prop_id: Option<&'a str>,
    checklists: &'a ChecklistIndex<'a>,
}

fn build_cards(
    mut cards: Vec<TrelloCard>,
    mapping: &CardMapping<'_>,
    limits: &BundleLimits,
    body_budget: &mut BodyBudget,
    summary: &mut ImportSummary,
) -> Result<BuiltCards, AppError> {
    let CardMapping {
        board_id,
        list_options,
        closed_lists,
        list_rank,
        status_prop_id,
        labels_map,
        due_prop_id,
        checklists,
    } = *mapping;
    cards.sort_by(|a, b| {
        let ra = list_rank.get(&a.id_list).copied().unwrap_or(usize::MAX);
        let rb = list_rank.get(&b.id_list).copied().unwrap_or(usize::MAX);
        ra.cmp(&rb).then_with(|| a.pos.total_cmp(&b.pos))
    });

    let mut blocks = Vec::new();
    let mut id_map = HashMap::new();
    let mut order = Vec::new();
    let mut claimed_checklists: HashSet<&str> = HashSet::new();

    for card in cards {
        if card.closed || closed_lists.contains(&card.id_list) {
            summary.push_skipped(
                ImportWarningCode::ClosedItemSkipped,
                &card.name,
                "archived Trello card or card in archived list skipped",
            );
            continue;
        }
        let card_id = new_id();
        let title = normalize_title(&card.name, "Untitled card", summary);
        let properties = build_card_properties(
            &card,
            list_options,
            status_prop_id,
            labels_map,
            due_prop_id,
            summary,
        );
        let matched_checklists = checklists_for_card(&card, checklists, &mut claimed_checklists);
        let body = build_card_body(&card, &matched_checklists, limits, summary)?;
        body_budget.charge(body.as_ref())?;

        let mut fields = JsonMap::new();
        fields.insert("properties".to_owned(), Value::Object(properties));
        fields.insert("contentOrder".to_owned(), Value::Array(Vec::new()));

        id_map.insert(card.id, card_id.clone());
        order.push(card_id.clone());
        blocks.push(Block {
            id: card_id,
            container_id: board_id.to_owned(),
            block_type: BlockType::Card,
            title,
            fields,
            body,
            ..Block::default()
        });
    }
    Ok(BuiltCards {
        blocks,
        id_map,
        order,
    })
}

fn build_card_properties(
    card: &TrelloCard,
    list_options: &HashMap<String, String>,
    status_prop_id: &str,
    labels_map: Option<&LabelPropertyMap>,
    due_prop_id: Option<&str>,
    summary: &mut ImportSummary,
) -> JsonMap {
    let mut properties = JsonMap::new();
    if let Some(opt_id) = list_options.get(&card.id_list) {
        properties.insert(status_prop_id.to_owned(), Value::String(opt_id.clone()));
    } else if !card.id_list.is_empty() {
        summary.push_warning(
            ImportWarningCode::UnresolvedReference,
            &card.name,
            "card referenced an unknown Trello list; imported without status",
        );
    }

    if let Some(labels) = labels_map {
        let mut selected = Vec::new();
        let mut seen = HashSet::new();
        let raw_ids = card
            .id_labels
            .0
            .iter()
            .map(String::as_str)
            .chain(card.labels.0.iter().map(|label| label.id.as_str()));
        for trello_label_id in raw_ids {
            if let Some(opt_id) = labels.by_trello_id.get(trello_label_id)
                && seen.insert(opt_id.as_str())
            {
                selected.push(Value::String(opt_id.clone()));
            }
        }
        if !selected.is_empty() {
            properties.insert(labels.prop_id.clone(), Value::Array(selected));
        }
    }

    if let Some(due_id) = due_prop_id
        && let Some(date_value) = trello_date_value(card.start.as_deref(), card.due.as_deref())
    {
        properties.insert(due_id.to_owned(), Value::Object(date_value));
    }
    properties
}

/// Collects checklists belonging to `card`, enforcing single-card ownership via `claimed`
/// so a crafted export cannot amplify memory by referencing one checklist from many cards.
fn checklists_for_card<'a>(
    card: &TrelloCard,
    index: &ChecklistIndex<'a>,
    claimed: &mut HashSet<&'a str>,
) -> Vec<&'a TrelloChecklist> {
    let mut matched = Vec::new();
    if let Some(owned) = index.by_card.get(card.id.as_str()) {
        for &checklist in owned {
            if claimed.insert(checklist.id.as_str()) {
                matched.push(checklist);
            }
        }
    }
    if !card.id_checklists.0.is_empty() {
        for checklist_id in &card.id_checklists.0 {
            if let Some(&checklist) = index.by_id.get(checklist_id.as_str())
                && (checklist.id_card.is_empty() || checklist.id_card == card.id)
                && claimed.insert(checklist.id.as_str())
            {
                matched.push(checklist);
            }
        }
        matched.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    }
    matched
}

fn build_card_body(
    card: &TrelloCard,
    checklists: &[&TrelloChecklist],
    limits: &BundleLimits,
    summary: &mut ImportSummary,
) -> Result<Option<fvoci_model::Body>, AppError> {
    let mut nodes = Vec::new();
    let mut estimated_nodes: usize = 1;
    if !card.desc.trim().is_empty() {
        let mut resolver = ExternalOnlyResolver;
        let converted = markdown_to_blocks(
            &card.desc,
            BodyContext::Document,
            limits.max_markdown_depth,
            &mut resolver,
        )?;
        for notice in converted.notices {
            summary.push_warning(notice.code, &card.name, notice.reason);
        }
        estimated_nodes = estimated_nodes.saturating_add(converted.nodes.len());
        nodes.extend(converted.nodes);
    }

    for checklist in checklists {
        append_checklist(&mut nodes, &mut estimated_nodes, checklist)?;
    }
    append_attachments(
        &mut nodes,
        &mut estimated_nodes,
        &card.name,
        &card.attachments.0,
        summary,
    )?;
    blocks_to_body(nodes)
}

fn append_checklist(
    nodes: &mut Vec<Value>,
    estimated_nodes: &mut usize,
    checklist: &TrelloChecklist,
) -> Result<(), AppError> {
    if checklist.check_items.0.is_empty() {
        return Ok(());
    }
    // Each taskItem contributes taskItem + paragraph + text (~3 nodes), plus heading/taskList.
    let needed = checklist
        .check_items
        .0
        .len()
        .saturating_mul(3)
        .saturating_add(3);
    *estimated_nodes = estimated_nodes.saturating_add(needed);
    if *estimated_nodes > BODY_MAX_NODES {
        return Err(AppError::bad_request(format_args!(
            "Trello card body exceeds maximum of {BODY_MAX_NODES} nodes"
        )));
    }
    let name = checklist.name.trim();
    if !name.is_empty() {
        nodes.push(text_heading(3, name));
    }
    let mut items: Vec<&TrelloCheckItem> = checklist.check_items.0.iter().collect();
    items.sort_by(|a, b| a.pos.total_cmp(&b.pos));
    let task_items: Vec<Value> = items
        .into_iter()
        .map(|item| {
            let checked = item.state.eq_ignore_ascii_case("complete");
            json!({
                "type": "taskItem",
                "attrs": {"checked": checked},
                "content": [text_paragraph(item.name.trim())],
            })
        })
        .collect();
    nodes.push(json!({"type": "taskList", "content": task_items}));
    Ok(())
}

fn append_attachments(
    nodes: &mut Vec<Value>,
    estimated_nodes: &mut usize,
    card_name: &str,
    attachments: &[TrelloAttachment],
    summary: &mut ImportSummary,
) -> Result<(), AppError> {
    let needed = attachments.len().saturating_mul(3).saturating_add(3);
    *estimated_nodes = estimated_nodes.saturating_add(needed);
    if *estimated_nodes > BODY_MAX_NODES {
        return Err(AppError::bad_request(format_args!(
            "Trello card body exceeds maximum of {BODY_MAX_NODES} nodes"
        )));
    }
    let mut items = Vec::new();
    for attachment in attachments {
        let url = attachment.url.trim();
        let name = attachment.name.trim();
        if url.is_empty() && name.is_empty() {
            continue;
        }
        // SSRF protection: external Trello attachment URLs are NEVER fetched by the server.
        // Only link absolute URLs with an allowed scheme; keep relative paths as plain text
        // so they do not turn into clickable internal fvoci paths.
        let (inline, kept_link) = if url.is_empty() {
            (json!({"type": "text", "text": name}), false)
        } else if has_url_scheme(url) {
            linked_text_node(name, url)
        } else {
            let shown = if name.is_empty() { url } else { name };
            (json!({"type": "text", "text": shown}), false)
        };
        if !url.is_empty() {
            if kept_link {
                summary.push_warning(
                    ImportWarningCode::ExternalAttachmentLinked,
                    card_name,
                    format!(
                        "external Trello attachment kept as link without fetching: {}",
                        truncate_chars(if name.is_empty() { url } else { name }, 120)
                    ),
                );
            } else {
                summary.push_warning(
                    ImportWarningCode::MarkdownDegraded,
                    card_name,
                    "attachment with disallowed URL scheme kept as plain text",
                );
            }
        }
        items.push(json!({
            "type": "listItem",
            "content": [{"type": "paragraph", "content": [inline]}],
        }));
    }
    if !items.is_empty() {
        nodes.push(text_heading(3, "Attachments"));
        nodes.push(json!({"type": "bulletList", "content": items}));
    }
    Ok(())
}

fn build_comments(
    board_id: &str,
    actions: Vec<TrelloAction>,
    card_id_map: &HashMap<String, String>,
    limits: &BundleLimits,
    body_budget: &mut BodyBudget,
    summary: &mut ImportSummary,
) -> Result<Vec<Block>, AppError> {
    let mut comments: Vec<(i64, TrelloAction)> = actions
        .into_iter()
        .filter(|action| action.action_type == "commentCard")
        .map(|action| {
            let ts = action
                .date
                .as_deref()
                .and_then(parse_iso_millis)
                .unwrap_or(0);
            (ts, action)
        })
        .collect();
    comments.sort_by_key(|(ts, _)| *ts);

    let mut blocks = Vec::new();
    for (created_at, action) in comments {
        let Some(card_ref) = action.data.card.as_ref() else {
            continue;
        };
        let Some(parent_id) = card_id_map.get(&card_ref.id) else {
            continue;
        };
        let text = action.data.text.as_deref().unwrap_or("").trim();
        let author = action
            .member_creator
            .as_ref()
            .and_then(|member| {
                member
                    .full_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        member
                            .username
                            .as_deref()
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                    })
            })
            .unwrap_or("알 수 없음");

        // Never link external authors to fvoci accounts (prevents author spoofing and
        // cross-workspace account enumeration); preserve the source display name as text
        // only while the DB author is always the authenticated importer.
        let safe_author = truncate_chars(author, 100);
        let mut nodes = vec![text_paragraph(&format!("가져옴: {safe_author}"))];
        if !text.is_empty() {
            let mut resolver = ExternalOnlyResolver;
            let converted = markdown_to_blocks(
                text,
                BodyContext::Comment,
                limits.max_markdown_depth,
                &mut resolver,
            )?;
            for notice in converted.notices {
                summary.push_warning(notice.code, author, notice.reason);
            }
            nodes.extend(converted.nodes);
        }
        let body = blocks_to_body(nodes)?;
        body_budget.charge(body.as_ref())?;
        blocks.push(Block {
            id: new_id(),
            container_id: board_id.to_owned(),
            parent_id: parent_id.clone(),
            block_type: BlockType::Comment,
            body,
            created_at,
            ..Block::default()
        });
    }
    Ok(blocks)
}

fn build_board_view(
    board_id: &str,
    status_prop_id: &str,
    visible_groups: &[String],
    shown_properties: &[String],
    card_order: &[String],
) -> Block {
    let fields = json_object(json!({
        "viewType": "board",
        "groupBy": status_prop_id,
        "visibleGroups": visible_groups,
        "hiddenGroups": [],
        "collapsedGroups": [],
        "shownProperties": shown_properties,
        "cardOrder": card_order,
        "sort": [],
        "filter": {"operation": "and", "filters": []},
        "columnWidths": {},
        "columnCalculations": {},
        "groupCalculations": {},
        "defaultTemplateId": "",
    }));
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::View,
        title: "Board".to_owned(),
        fields,
        ..Block::default()
    }
}

fn trello_date_value(start: Option<&str>, due: Option<&str>) -> Option<JsonMap> {
    let start_day = start.and_then(parse_iso_day);
    let due_day = due.and_then(parse_iso_day);
    let mut map = Map::new();
    match (start_day, due_day) {
        (Some(from), Some(to)) if to > from => {
            map.insert("from".to_owned(), Value::String(from));
            map.insert("to".to_owned(), Value::String(to));
        }
        (_, Some(day)) | (Some(day), None) => {
            map.insert("from".to_owned(), Value::String(day));
        }
        (None, None) => return None,
    }
    check_date_value(&map).ok()?;
    Some(map)
}

fn parse_iso_day(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if let Ok(ts) = trimmed.parse::<jiff::Timestamp>() {
        let day = jiff::tz::TimeZone::UTC.to_datetime(ts).date().to_string();
        return Some(day);
    }
    let prefix = trimmed.get(..10)?;
    let mut check = Map::new();
    check.insert("from".to_owned(), Value::String(prefix.to_owned()));
    check_date_value(&check).ok()?;
    Some(prefix.to_owned())
}

fn parse_iso_millis(raw: &str) -> Option<i64> {
    raw.trim()
        .parse::<jiff::Timestamp>()
        .ok()
        .map(jiff::Timestamp::as_millisecond)
        .filter(|&ms| ms > 0)
}

fn normalize_title(raw: &str, fallback: &str, summary: &mut ImportSummary) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return fallback.to_owned();
    }
    if trimmed.chars().count() > BLOCK_TITLE_MAX_CHARS {
        summary.push_warning(
            ImportWarningCode::ContentTruncated,
            truncate_chars(trimmed, 80),
            format!("title truncated to {BLOCK_TITLE_MAX_CHARS} characters"),
        );
        return truncate_chars(trimmed, BLOCK_TITLE_MAX_CHARS);
    }
    trimmed.to_owned()
}

fn json_object(value: Value) -> JsonMap {
    match value {
        Value::Object(map) => map,
        _ => JsonMap::new(),
    }
}
