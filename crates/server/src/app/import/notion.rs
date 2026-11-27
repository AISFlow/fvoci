//! Notion export ZIP converter.
//!
//! Converts a validated [`SafeBundle`] from a Notion export into one or more fvoci
//! containers inside a [`StagedArchive`]:
//!
//! - Markdown page hierarchies become a `ContainerKind::Wiki` with parent/child pages,
//!   ordering (`PAGE_ORDER_FIELD`), internal links rewritten to `reference` nodes, B1
//!   `table` nodes, and bundle-local images/files attached to the wiki.
//! - CSV databases plus their companion Markdown row files become `ContainerKind::Board`
//!   containers with inferred property types (`select`, `multiSelect`, `date`, `checkbox`,
//!   `number`, `url`, `text`), default views, card bodies, and bundle-local assets.
//! - External URLs are never fetched (SSRF blocked); paths outside the bundle are refused.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;

use fvoci_model::{
    BLOCK_TITLE_MAX_CHARS, Block, BlockError, BlockType, Body, Container, ContainerKind,
    ContainerVisibility, JsonMap, PAGE_ORDER_FIELD, PageLimits, new_id, order_between,
};
use fvoci_store::StoreError;
use serde_json::{Value, json};

use super::bundle::{BundleEntry, SafeBundle, resolve_relative_bundle_path};
use super::markdown::{
    AssetResolver, BodyContext, MarkdownNotice, ResolvedTarget, blocks_to_body, markdown_to_blocks,
};
use super::markdown_nodes::BodyBudget;
use super::notion_csv::{ParsedCsvDatabase, ParsedCsvRow, parse_notion_csv_bounded};
use super::staged::{StagedArchive, StagedContainer, StagedFile};
use super::summary::{
    BundleLimits, ImportFormat, ImportSummary, ImportWarningCode, truncate_chars,
};
use crate::app::files::stored_extension;
use crate::app::pages::refused_without_member;
use crate::error::{AppError, Reason};

/// File extensions treated as inline `image` blocks rather than `file` download blocks.
const IMAGE_EXTENSIONS: [&str; 9] = [
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif",
];

/// A discovered CSV database entry and its companion directory prefix.
struct CsvDatabaseSpec<'a> {
    entry: &'a BundleEntry,
    title: String,
    companion_dir: String,
}

/// A Markdown entry mapped to its logical path (after stripping an optional root wrapper).
#[derive(Clone)]
struct LogicalEntry<'a> {
    logical_path: String,
    entry: &'a BundleEntry,
}

/// Shared owned lookup maps passed across `spawn_blocking` for off-runtime Markdown conversion.
struct AssetLookup {
    by_logical: HashMap<String, BundleEntry>,
    md_to_block_id: HashMap<String, String>,
}

/// Pre-parsed Notion CSV databases along with claimed companion Markdown paths and item count.
struct PlannedCsvDatabases<'a> {
    parsed_dbs: Vec<(CsvDatabaseSpec<'a>, ParsedCsvDatabase, Vec<Option<String>>)>,
    card_ids_per_db: Vec<Vec<String>>,
    claimed_mds: HashSet<String>,
    md_to_block_id: HashMap<String, String>,
    planned_items: u64,
}

async fn plan_csv_databases<'a>(
    csv_specs: Vec<CsvDatabaseSpec<'a>>,
    logical_entries: &[LogicalEntry<'a>],
    bundle: &SafeBundle,
    limits: &BundleLimits,
) -> Result<PlannedCsvDatabases<'a>, AppError> {
    let mut claimed_mds: HashSet<String> = HashSet::new();
    let mut parsed_dbs = Vec::with_capacity(csv_specs.len());
    let mut md_to_block_id: HashMap<String, String> = HashMap::new();
    let mut card_ids_per_db = Vec::with_capacity(csv_specs.len());
    let mut planned_items: u64 = 0;

    for spec in csv_specs {
        planned_items = planned_items.saturating_add(2);
        if planned_items > limits.max_entries {
            return Err(too_many_notion_items(limits.max_entries));
        }
        let remaining =
            usize::try_from(limits.max_entries.saturating_sub(planned_items)).unwrap_or(usize::MAX);
        let csv_text = bundle
            .read_text(spec.entry, limits.max_text_entry_bytes)
            .await?;
        let limits_copy = *limits;
        let parsed = tokio::task::spawn_blocking(move || {
            parse_notion_csv_bounded(&csv_text, &limits_copy, remaining)
        })
        .await
        .map_err(|error| AppError::internal(format_args!("CSV parse task failed: {error}")))??;

        planned_items =
            planned_items.saturating_add(u64::try_from(parsed.rows.len()).unwrap_or(u64::MAX));
        if planned_items > limits.max_entries {
            return Err(too_many_notion_items(limits.max_entries));
        }

        let row_mds = match_row_markdowns(&spec.companion_dir, &parsed, logical_entries);
        let mut card_ids = Vec::with_capacity(parsed.rows.len());
        for matched_md in &row_mds {
            let card_id = new_id();
            if let Some(md_path) = matched_md {
                claimed_mds.insert(md_path.clone());
                md_to_block_id.insert(md_path.clone(), card_id.clone());
            }
            card_ids.push(card_id);
        }
        card_ids_per_db.push(card_ids);
        parsed_dbs.push((spec, parsed, row_mds));
    }

    Ok(PlannedCsvDatabases {
        parsed_dbs,
        card_ids_per_db,
        claimed_mds,
        md_to_block_id,
        planned_items,
    })
}

/// Converts a validated Notion [`SafeBundle`] into a [`StagedArchive`].
///
/// # Errors
///
/// Returns 400 or 413 if the bundle holds no Notion Markdown/CSV files, or if any
/// Markdown/CSV entry violates parsing, depth, or size limits.
pub(crate) async fn convert_notion_bundle(
    bundle: &SafeBundle,
    limits: &BundleLimits,
    page_limits: PageLimits,
) -> Result<StagedArchive, AppError> {
    let (wrapper_title, logical_entries) = strip_single_wrapper(bundle.entries());
    let mut summary = ImportSummary::new(ImportFormat::Notion);

    let csv_specs = collect_csv_databases(&logical_entries, &mut summary);
    let PlannedCsvDatabases {
        parsed_dbs,
        card_ids_per_db,
        claimed_mds,
        mut md_to_block_id,
        planned_items,
    } = plan_csv_databases(csv_specs, &logical_entries, bundle, limits).await?;

    let wiki_mds: Vec<&LogicalEntry<'_>> = logical_entries
        .iter()
        .filter(|item| {
            is_markdown_path(&item.logical_path) && !claimed_mds.contains(&item.logical_path)
        })
        .collect();

    if wiki_mds.is_empty() && parsed_dbs.is_empty() {
        return Err(AppError::bad_request(
            "Notion export contains no Markdown pages or CSV databases",
        ));
    }

    let wiki_plan = build_wiki_plan(
        &wiki_mds,
        &mut md_to_block_id,
        &mut summary,
        planned_items,
        limits.max_entries,
        page_limits,
    )?;

    let lookup = Arc::new(AssetLookup {
        by_logical: logical_entries
            .iter()
            .map(|item| (item.logical_path.clone(), item.entry.clone()))
            .collect(),
        md_to_block_id,
    });
    let mut containers = Vec::new();
    let mut body_budget = BodyBudget::default();
    let mut conversion = Conversion {
        bundle,
        lookup: &lookup,
        limits,
        body_budget: &mut body_budget,
        summary: &mut summary,
    };

    if !wiki_plan.pages.is_empty() {
        let wiki_title = wrapper_title.as_deref().unwrap_or("Notion Import");
        let wiki = build_wiki_container(wiki_title, &wiki_plan, &mut conversion).await?;
        containers.push(wiki);
    }

    for ((spec, parsed, row_mds), card_ids) in parsed_dbs.into_iter().zip(card_ids_per_db) {
        let board =
            build_board_container(&spec, parsed, &row_mds, &card_ids, &mut conversion).await?;
        containers.push(board);
    }

    Ok(StagedArchive {
        containers,
        summary,
    })
}

fn too_many_notion_items(limit: u64) -> AppError {
    AppError::too_large(
        format_args!("Notion export exceeds maximum of {limit} items"),
        limit,
    )
    .with_reason(Reason::ArchiveTooLarge)
}

/// Strips a single redundant top-level directory wrapper if every entry in the bundle
/// resides inside it (e.g. `Export/Home.md`, `Export/Tasks.csv`).
fn strip_single_wrapper(entries: &[BundleEntry]) -> (Option<String>, Vec<LogicalEntry<'_>>) {
    let mut root: Option<&str> = None;
    let mut all_under_one = !entries.is_empty();
    for entry in entries {
        let Some((first, _)) = entry.path.split_once('/') else {
            all_under_one = false;
            break;
        };
        match root {
            None => root = Some(first),
            Some(prev) if prev == first => {}
            Some(_) => {
                all_under_one = false;
                break;
            }
        }
    }
    if all_under_one && let Some(prefix) = root {
        let title = strip_notion_id(prefix);
        let stripped = entries
            .iter()
            .filter_map(|entry| {
                let rest = entry.path.strip_prefix(prefix)?.strip_prefix('/')?;
                Some(LogicalEntry {
                    logical_path: rest.to_owned(),
                    entry,
                })
            })
            .collect();
        return (Some(title), stripped);
    }
    let mapped = entries
        .iter()
        .map(|entry| LogicalEntry {
            logical_path: entry.path.clone(),
            entry,
        })
        .collect();
    (None, mapped)
}

/// Collects CSV databases, preferring `<name>_all.csv` when Notion exports both a default
/// view `.csv` and a complete `_all.csv` alongside each other.
fn collect_csv_databases<'a>(
    entries: &[LogicalEntry<'a>],
    summary: &mut ImportSummary,
) -> Vec<CsvDatabaseSpec<'a>> {
    let mut by_key: BTreeMap<String, (bool, &LogicalEntry<'a>)> = BTreeMap::new();
    for item in entries {
        if !is_csv_path(&item.logical_path) {
            continue;
        }
        let stem_no_ext = strip_extension(&item.logical_path);
        let (key, is_all) = match stem_no_ext.strip_suffix("_all") {
            Some(base) => (base.to_owned(), true),
            None => (stem_no_ext.to_owned(), false),
        };
        match by_key.get(&key) {
            Some((existing_is_all, existing_item)) => {
                if is_all && !existing_is_all {
                    summary.push_warning(
                        ImportWarningCode::UnsupportedEntrySkipped,
                        &existing_item.logical_path,
                        "skipped partial CSV view in favor of complete `_all.csv`",
                    );
                    by_key.insert(key, (true, item));
                } else {
                    summary.push_warning(
                        ImportWarningCode::UnsupportedEntrySkipped,
                        &item.logical_path,
                        "skipped duplicate CSV export for same database",
                    );
                }
            }
            None => {
                by_key.insert(key, (is_all, item));
            }
        }
    }

    by_key
        .into_iter()
        .map(|(companion_dir, (_, item))| {
            let file_stem = companion_dir.rsplit('/').next().unwrap_or(&companion_dir);
            let title = strip_notion_id(file_stem);
            CsvDatabaseSpec {
                entry: item.entry,
                title,
                companion_dir,
            }
        })
        .collect()
}

/// Matches each CSV row to a companion `.md` file inside `companion_dir/` by cleaned title
/// in amortized O(rows + entries) time.
fn match_row_markdowns(
    companion_dir: &str,
    db: &ParsedCsvDatabase,
    entries: &[LogicalEntry<'_>],
) -> Vec<Option<String>> {
    let prefix = format!("{companion_dir}/");
    let mut candidates: Vec<(String, String, String)> = entries
        .iter()
        .filter_map(|item| {
            let rest = item.logical_path.strip_prefix(&prefix)?;
            if !is_markdown_path(&item.logical_path) || rest.contains('/') {
                return None;
            }
            let stem = strip_extension(rest);
            let clean = strip_notion_id(stem);
            let norm = normalize_match_key(&clean);
            Some((item.logical_path.clone(), clean, norm))
        })
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0));

    let mut by_clean: HashMap<&str, VecDeque<usize>> = HashMap::with_capacity(candidates.len());
    let mut by_norm: HashMap<&str, VecDeque<usize>> = HashMap::with_capacity(candidates.len());
    for (idx, (_, clean, norm)) in candidates.iter().enumerate() {
        by_clean.entry(clean.as_str()).or_default().push_back(idx);
        if !norm.is_empty() {
            by_norm.entry(norm.as_str()).or_default().push_back(idx);
        }
    }

    let mut used = vec![false; candidates.len()];
    let mut matched = vec![None; db.rows.len()];
    match_exact_titles(
        &db.rows,
        &candidates,
        &mut by_clean,
        &mut used,
        &mut matched,
    );
    match_normalized_titles(&db.rows, &candidates, &mut by_norm, &mut used, &mut matched);
    matched
}

/// Gives each row the file (of `candidates`: its path, cleaned title and normalized
/// title) whose cleaned title is the row's. This goes first, so that a normalized
/// match on an earlier row cannot take the file that is the exact match of a later one.
fn match_exact_titles(
    rows: &[ParsedCsvRow],
    candidates: &[(String, String, String)],
    by_clean: &mut HashMap<&str, VecDeque<usize>>,
    used: &mut [bool],
    matched: &mut [Option<String>],
) {
    for (slot, row) in matched.iter_mut().zip(rows) {
        if let Some(idx) = pop_unused(by_clean, row.title.trim(), used) {
            *slot = Some(candidates[idx].0.clone());
        }
    }
}

/// Gives each row still without a file the one whose title is the row's but for case
/// and punctuation.
fn match_normalized_titles(
    rows: &[ParsedCsvRow],
    candidates: &[(String, String, String)],
    by_norm: &mut HashMap<&str, VecDeque<usize>>,
    used: &mut [bool],
    matched: &mut [Option<String>],
) {
    for (slot, row) in matched.iter_mut().zip(rows) {
        if slot.is_some() {
            continue;
        }
        let row_norm = normalize_match_key(row.title.trim());
        if !row_norm.is_empty()
            && let Some(idx) = pop_unused(by_norm, &row_norm, used)
        {
            *slot = Some(candidates[idx].0.clone());
        }
    }
}

fn pop_unused(
    index: &mut HashMap<&str, VecDeque<usize>>,
    key: &str,
    used: &mut [bool],
) -> Option<usize> {
    let queue = index.get_mut(key)?;
    while let Some(idx) = queue.pop_front() {
        if !used[idx] {
            used[idx] = true;
            return Some(idx);
        }
    }
    None
}

struct PlannedPage<'a> {
    id: String,
    parent_id: String,
    title: String,
    order: String,
    source: Option<&'a LogicalEntry<'a>>,
}

struct WikiPlan<'a> {
    pages: Vec<PlannedPage<'a>>,
}

fn check_planned_wiki_pages(
    page_count: usize,
    planned_items: u64,
    max_entries: u64,
    page_limits: PageLimits,
) -> Result<(), AppError> {
    if page_count == 0 {
        return Ok(());
    }
    let total_items = planned_items
        .saturating_add(1)
        .saturating_add(u64::try_from(page_count).unwrap_or(u64::MAX));
    if total_items > max_entries {
        return Err(too_many_notion_items(max_entries));
    }
    if usize::try_from(page_limits.pages).is_ok_and(|most| page_count > most) {
        return Err(refused_without_member(StoreError::InvalidBlock {
            id: String::new(),
            source: BlockError::TooManyPages {
                limit: page_limits.pages,
            },
        }));
    }
    Ok(())
}

/// Builds the Wiki page hierarchy from unclaimed Markdown entries, synthesizing folder
/// pages when a subfolder has no matching `<folder>.md` sibling, and enforcing item and
/// wiki page count limits before any Markdown body is parsed.
fn build_wiki_plan<'a>(
    wiki_mds: &[&'a LogicalEntry<'a>],
    md_to_block_id: &mut HashMap<String, String>,
    summary: &mut ImportSummary,
    planned_items: u64,
    max_entries: u64,
    page_limits: PageLimits,
) -> Result<WikiPlan<'a>, AppError> {
    let mut nodes_by_key: BTreeMap<String, (String, Option<&'a LogicalEntry<'a>>)> =
        BTreeMap::new();
    for item in wiki_mds {
        let key = strip_extension(&item.logical_path).to_owned();
        if nodes_by_key.contains_key(&key) {
            summary.push_skipped(
                ImportWarningCode::UnsupportedEntrySkipped,
                &item.logical_path,
                "skipped duplicate Markdown page sharing the same path stem",
            );
            continue;
        }
        check_planned_wiki_pages(
            nodes_by_key.len().saturating_add(1),
            planned_items,
            max_entries,
            page_limits,
        )?;
        let id = new_id();
        md_to_block_id.insert(item.logical_path.clone(), id.clone());
        nodes_by_key.insert(key, (id, Some(*item)));
    }
    // Ensure every ancestor directory has a page node so tree parent_ids always resolve.
    let existing_keys: Vec<String> = nodes_by_key.keys().cloned().collect();
    for key in existing_keys {
        let mut current = key.as_str();
        while let Some((parent, _)) = current.rsplit_once('/') {
            if !nodes_by_key.contains_key(parent) {
                check_planned_wiki_pages(
                    nodes_by_key.len().saturating_add(1),
                    planned_items,
                    max_entries,
                    page_limits,
                )?;
                nodes_by_key.insert(parent.to_owned(), (new_id(), None));
            }
            current = parent;
        }
    }

    let mut children_by_parent: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for key in nodes_by_key.keys() {
        let parent_key = key.rsplit_once('/').map_or("", |(p, _)| p).to_owned();
        children_by_parent
            .entry(parent_key)
            .or_default()
            .push(key.clone());
    }

    let mut pages = Vec::with_capacity(nodes_by_key.len());
    let mut queue = vec![String::new()];
    while let Some(parent_key) = queue.pop() {
        let Some(children) = children_by_parent.get(&parent_key) else {
            continue;
        };
        let parent_id = if parent_key.is_empty() {
            String::new()
        } else {
            nodes_by_key
                .get(&parent_key)
                .map_or_else(String::new, |(id, _)| id.clone())
        };
        let mut last_order: Option<String> = None;
        for child_key in children {
            let (id, source) = &nodes_by_key[child_key];
            let order =
                order_between(last_order.as_deref(), None).unwrap_or_else(|_| "V".to_owned());
            let stem = child_key.rsplit('/').next().unwrap_or(child_key);
            let title = strip_notion_id(stem);
            pages.push(PlannedPage {
                id: id.clone(),
                parent_id: parent_id.clone(),
                title,
                order: order.clone(),
                source: *source,
            });
            last_order = Some(order);
            queue.push(child_key.clone());
        }
    }
    Ok(WikiPlan { pages })
}

async fn convert_markdown_off_thread(
    md_text: String,
    base_dir: String,
    max_markdown_depth: usize,
    lookup: Arc<AssetLookup>,
    mut file_placeholders: HashMap<String, String>,
    mut staged_files: Vec<StagedFile>,
) -> Result<
    (
        Option<Body>,
        Vec<MarkdownNotice>,
        HashMap<String, String>,
        Vec<StagedFile>,
    ),
    AppError,
> {
    tokio::task::spawn_blocking(move || {
        let mut resolver = BundleAssetResolver {
            base_dir: &base_dir,
            lookup: &lookup,
            file_placeholders: &mut file_placeholders,
            staged_files: &mut staged_files,
        };
        let converted = markdown_to_blocks(
            &md_text,
            BodyContext::Document,
            max_markdown_depth,
            &mut resolver,
        )?;
        let body = blocks_to_body(converted.nodes)?;
        Ok((body, converted.notices, file_placeholders, staged_files))
    })
    .await
    .map_err(|error| AppError::internal(format_args!("markdown conversion task failed: {error}")))?
}

/// What the bodies of an export's pages are made with: the bundle and the assets in it,
/// the limits, and the budget of bodies and the summary they are counted in.
struct Conversion<'a> {
    bundle: &'a SafeBundle,
    lookup: &'a Arc<AssetLookup>,
    limits: &'a BundleLimits,
    body_budget: &'a mut BodyBudget,
    summary: &'a mut ImportSummary,
}

async fn build_wiki_container(
    wiki_title: &str,
    plan: &WikiPlan<'_>,
    conversion: &mut Conversion<'_>,
) -> Result<StagedContainer, AppError> {
    let (bundle, lookup, limits) = (conversion.bundle, conversion.lookup, conversion.limits);
    let wiki_id = new_id();
    let mut staged_files = Vec::new();
    let mut file_placeholders: HashMap<String, String> = HashMap::new();
    let mut blocks = Vec::with_capacity(plan.pages.len());

    for page in &plan.pages {
        let body = if let Some(source) = page.source {
            let md_text = bundle
                .read_text(source.entry, limits.max_text_entry_bytes)
                .await?;
            let base_dir = source
                .logical_path
                .rsplit_once('/')
                .map_or("", |(dir, _)| dir)
                .to_owned();
            let (body, notices, next_placeholders, next_files) = convert_markdown_off_thread(
                md_text,
                base_dir,
                limits.max_markdown_depth,
                Arc::clone(lookup),
                file_placeholders,
                staged_files,
            )
            .await?;
            conversion.body_budget.charge(body.as_ref())?;
            file_placeholders = next_placeholders;
            staged_files = next_files;
            for notice in notices {
                conversion
                    .summary
                    .push_warning(notice.code, &source.logical_path, notice.reason);
            }
            body
        } else {
            None
        };

        let mut fields = JsonMap::new();
        fields.insert(
            PAGE_ORDER_FIELD.to_owned(),
            Value::String(page.order.clone()),
        );
        blocks.push(Block {
            id: page.id.clone(),
            container_id: wiki_id.clone(),
            parent_id: page.parent_id.clone(),
            block_type: BlockType::Page,
            title: truncate_title(&page.title, conversion.summary),
            fields,
            body,
            ..Block::default()
        });
    }

    let container = Container {
        id: wiki_id,
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Wiki,
        title: truncate_title(wiki_title, conversion.summary),
        ..Container::default()
    };
    Ok(StagedContainer {
        container,
        blocks,
        files: staged_files,
    })
}

async fn build_board_container(
    spec: &CsvDatabaseSpec<'_>,
    parsed: ParsedCsvDatabase,
    row_mds: &[Option<String>],
    card_ids: &[String],
    conversion: &mut Conversion<'_>,
) -> Result<StagedContainer, AppError> {
    let (bundle, lookup, limits) = (conversion.bundle, conversion.lookup, conversion.limits);
    let board_id = new_id();
    let mut staged_files = Vec::new();
    let mut file_placeholders: HashMap<String, String> = HashMap::new();
    let mut blocks = Vec::with_capacity(1 + parsed.rows.len());

    let view = build_csv_view(
        &board_id,
        parsed.first_select.as_ref(),
        &parsed.all_property_ids,
        card_ids,
    );
    blocks.push(view);

    for ((row, card_id), matched_md) in parsed
        .rows
        .into_iter()
        .zip(card_ids.iter())
        .zip(row_mds.iter())
    {
        let body = if let Some(md_path) = matched_md
            && let Some(entry) = lookup.by_logical.get(md_path.as_str())
        {
            let md_text = bundle.read_text(entry, limits.max_text_entry_bytes).await?;
            let base_dir = md_path
                .rsplit_once('/')
                .map_or("", |(dir, _)| dir)
                .to_owned();
            let (body, notices, next_placeholders, next_files) = convert_markdown_off_thread(
                md_text,
                base_dir,
                limits.max_markdown_depth,
                Arc::clone(lookup),
                file_placeholders,
                staged_files,
            )
            .await?;
            conversion.body_budget.charge(body.as_ref())?;
            file_placeholders = next_placeholders;
            staged_files = next_files;
            for notice in notices {
                conversion
                    .summary
                    .push_warning(notice.code, md_path, notice.reason);
            }
            body
        } else {
            None
        };

        let mut fields = JsonMap::new();
        fields.insert("properties".to_owned(), Value::Object(row.properties));
        fields.insert("contentOrder".to_owned(), Value::Array(Vec::new()));
        blocks.push(Block {
            id: card_id.clone(),
            container_id: board_id.clone(),
            block_type: BlockType::Card,
            title: truncate_title(&row.title, conversion.summary),
            fields,
            body,
            ..Block::default()
        });
    }

    let container = Container {
        id: board_id,
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Board,
        title: truncate_title(&spec.title, conversion.summary),
        card_properties: parsed.card_properties,
        ..Container::default()
    };
    Ok(StagedContainer {
        container,
        blocks,
        files: staged_files,
    })
}

fn build_csv_view(
    board_id: &str,
    first_select: Option<&(String, Vec<String>)>,
    all_property_ids: &[String],
    card_ids: &[String],
) -> Block {
    let (title, fields) = match first_select {
        Some((group_id, option_ids)) => {
            let shown_props: Vec<&String> = all_property_ids
                .iter()
                .filter(|id| *id != group_id)
                .collect();
            (
                "Board",
                json!({
                    "viewType": "board",
                    "groupBy": group_id,
                    "visibleGroups": option_ids,
                    "hiddenGroups": [],
                    "collapsedGroups": [],
                    "shownProperties": shown_props,
                    "cardOrder": card_ids,
                    "sort": [],
                    "filter": {"operation": "and", "filters": []},
                    "columnWidths": {},
                    "columnCalculations": {},
                    "groupCalculations": {},
                    "defaultTemplateId": "",
                }),
            )
        }
        None => (
            "Table",
            json!({
                "viewType": "table",
                "visibleGroups": [],
                "hiddenGroups": [],
                "collapsedGroups": [],
                "shownProperties": all_property_ids,
                "cardOrder": card_ids,
                "sort": [],
                "filter": {"operation": "and", "filters": []},
                "columnWidths": {},
                "columnCalculations": {},
                "groupCalculations": {},
                "defaultTemplateId": "",
            }),
        ),
    };
    let fields = match fields {
        Value::Object(map) => map,
        _ => JsonMap::new(),
    };
    Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::View,
        title: title.to_owned(),
        fields,
        ..Block::default()
    }
}

/// Resolves Markdown image and file links strictly against entries inside the validated bundle.
struct BundleAssetResolver<'a> {
    base_dir: &'a str,
    lookup: &'a AssetLookup,
    file_placeholders: &'a mut HashMap<String, String>,
    staged_files: &'a mut Vec<StagedFile>,
}

impl BundleAssetResolver<'_> {
    fn register_local_asset(&mut self, logical_path: &str, entry: &BundleEntry) -> String {
        if let Some(existing) = self.file_placeholders.get(logical_path) {
            return existing.clone();
        }
        let original_name = logical_path
            .rsplit('/')
            .next()
            .unwrap_or(logical_path)
            .to_owned();
        let ext = stored_extension(&original_name);
        let placeholder = format!("asset{:05}{ext}", self.staged_files.len() + 1);
        self.staged_files.push(StagedFile {
            placeholder: placeholder.clone(),
            original_name,
            entry: entry.clone(),
        });
        self.file_placeholders
            .insert(logical_path.to_owned(), placeholder.clone());
        placeholder
    }
}

impl AssetResolver for BundleAssetResolver<'_> {
    fn resolve_image(&mut self, dest: &str) -> ResolvedTarget {
        if super::bundle::has_url_scheme(dest.trim()) {
            return ResolvedTarget::ExternalUrl;
        }
        let Some(resolved) = resolve_relative_bundle_path(self.base_dir, dest) else {
            return ResolvedTarget::MissingLocal;
        };
        let Some(entry) = self.lookup.by_logical.get(resolved.as_str()) else {
            return ResolvedTarget::MissingLocal;
        };
        let file_id = self.register_local_asset(&resolved, entry);
        if is_image_path(&resolved) {
            ResolvedTarget::LocalImage { file_id }
        } else {
            ResolvedTarget::LocalFile { file_id }
        }
    }

    fn resolve_link(&mut self, dest: &str) -> ResolvedTarget {
        if super::bundle::has_url_scheme(dest.trim()) {
            return ResolvedTarget::ExternalUrl;
        }
        let Some(resolved) = resolve_relative_bundle_path(self.base_dir, dest) else {
            return ResolvedTarget::MissingLocal;
        };
        if let Some(block_id) = self.lookup.md_to_block_id.get(&resolved) {
            return ResolvedTarget::InternalReference {
                block_id: block_id.clone(),
            };
        }
        let Some(entry) = self.lookup.by_logical.get(resolved.as_str()) else {
            return ResolvedTarget::MissingLocal;
        };
        if is_markdown_path(&resolved) || is_csv_path(&resolved) {
            return ResolvedTarget::MissingLocal;
        }
        let file_id = self.register_local_asset(&resolved, entry);
        if is_image_path(&resolved) {
            ResolvedTarget::LocalImage { file_id }
        } else {
            ResolvedTarget::LocalFile { file_id }
        }
    }
}

/// Strips Notion's trailing `" <32 lowercase hex chars>"` export suffix from a name.
pub(crate) fn strip_notion_id(name: &str) -> String {
    let trimmed = name.trim();
    if let Some((head, tail)) = trimmed.rsplit_once(' ')
        && tail.len() == 32
        && tail
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        && !head.trim().is_empty()
    {
        return head.trim().to_owned();
    }
    if trimmed.is_empty() {
        "Untitled".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn normalize_match_key(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                ch.to_ascii_lowercase()
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn strip_extension(path: &str) -> &str {
    match path.rsplit_once('.') {
        Some((stem, ext)) if !ext.contains('/') => stem,
        _ => path,
    }
}

fn is_markdown_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
}

fn is_csv_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("csv"))
}

fn is_image_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|ext| {
            IMAGE_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

fn truncate_title(raw: &str, summary: &mut ImportSummary) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return "Untitled".to_owned();
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
