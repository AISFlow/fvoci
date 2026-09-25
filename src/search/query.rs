//! Workspace search query, PG hydrate, and snippets.
//!
//! Ports the lexical path of `packages/core/src/search.ts` and
//! `packages/search/src/snippet.ts` at source SHA
//! `393795261322b916e588043cf94feca999175843`.
//!
//! Scope uses `project_permission` / `document_permission`. The Meili filter
//! is recall only; hydrate re-checks the current DB state.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::db::context::{session_is_live, set_tenant};
use crate::db::documents::{document_permission, membership_role, workspace_is_live};
use crate::db::group_grants::guest_wiki_document_ids_select_sql;
use crate::db::projects::{project_permission, LockedProject};
use crate::db::workspace::{list_workspaces_for_user, WorkspaceRole};
use crate::display_id::format_display_id;
use crate::projects::ProjectPermission;
use crate::search::meili::{
    search_meili, MeiliConfig, MeiliError, MeiliHit, MeiliSearchInput, MeiliSearchScope,
    SearchSourceKind, MEILI_MAX_TOTAL_HITS,
};
use crate::search::text::{is_chosung_query, stem_text};

pub const CHOSUNG_MIN_LENGTH: usize = 2;
const MEILI_PAGE: u32 = 50;
const MAX_CANDIDATES: u32 = 400;
const MAX_MEILI_PAGES: u32 = 8;
const MAX_SEARCH_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SearchTypeFilter {
    All,
    Document,
    Task,
    Attachment,
    Comment,
}

impl SearchTypeFilter {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Document => "document",
            Self::Task => "task",
            Self::Attachment => "attachment",
            Self::Comment => "comment",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "document" => Some(Self::Document),
            "task" => Some(Self::Task),
            "attachment" => Some(Self::Attachment),
            "comment" => Some(Self::Comment),
            _ => None,
        }
    }

    fn meili_kind(self) -> Option<SearchSourceKind> {
        match self {
            Self::All => None,
            Self::Document => Some(SearchSourceKind::Document),
            Self::Task => Some(SearchSourceKind::Task),
            Self::Attachment => Some(SearchSourceKind::Attachment),
            Self::Comment => Some(SearchSourceKind::Comment),
        }
    }

    fn matches_kind(self, kind: SearchSourceKind) -> bool {
        self == Self::All
            || matches!(
                (self, kind),
                (Self::Document, SearchSourceKind::Document)
                    | (Self::Task, SearchSourceKind::Task)
                    | (Self::Attachment, SearchSourceKind::Attachment)
                    | (Self::Comment, SearchSourceKind::Comment)
            )
    }
}

#[derive(Debug)]
enum ScanError {
    Meili(MeiliError),
    Db(sqlx::Error),
}

impl From<MeiliError> for ScanError {
    fn from(error: MeiliError) -> Self {
        Self::Meili(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchQueryError {
    NotFound,
    Forbidden,
    InvalidCursor,
    MeiliUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetPiece {
    pub text: String,
    pub r#match: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResultItem {
    pub r#type: SearchTypeFilter,
    pub id: Uuid,
    pub title: String,
    pub display_id: Option<String>,
    pub extract_status: Option<String>,
    pub chunk_no: Option<i64>,
    pub snippet: Option<Vec<SnippetPiece>>,
    pub project_id: Option<Uuid>,
    pub document_id: Option<Uuid>,
    pub task_id: Option<Uuid>,
    pub score: f64,
    pub updated_at: DateTime<Utc>,
    pub workspace_id: Uuid,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchResultPage {
    pub items: Vec<SearchResultItem>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceSearchRequest<'a> {
    pub workspace_id: Uuid,
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub q: &'a str,
    pub r#type: SearchTypeFilter,
    pub project_id: Option<Uuid>,
    pub tag: Option<Uuid>,
    pub cursor: Option<&'a str>,
    pub limit: u32,
    pub meili: &'a MeiliConfig,
}

#[derive(Debug, Clone)]
pub struct GlobalSearchRequest<'a> {
    pub actor_user_id: Uuid,
    pub session_id: Uuid,
    pub q: &'a str,
    pub r#type: SearchTypeFilter,
    pub tag: Option<Uuid>,
    pub cursor: Option<&'a str>,
    pub limit: u32,
    pub meili: &'a MeiliConfig,
}

#[derive(Debug, Clone)]
struct VisibleWorkspaceAcl {
    workspace_id: Uuid,
    acl: SearchAcl,
}

#[derive(Debug, Clone)]
struct SearchAcl {
    project_ids: Vec<Uuid>,
    include_wiki: bool,
    wiki_document_ids: Vec<Uuid>,
}

#[derive(Debug, Clone)]
struct PreparedQuery {
    q: String,
    stem: String,
    chosung: bool,
    title_prefix: Option<String>,
    r#type: SearchTypeFilter,
    limit: u32,
    offset: u32,
}

#[derive(Debug, Clone)]
struct HydratedRow {
    r#type: SearchTypeFilter,
    id: Uuid,
    title: String,
    body: String,
    project_id: Option<Uuid>,
    document_id: Option<Uuid>,
    task_id: Option<Uuid>,
    number: Option<i32>,
    project_key: Option<String>,
    extract_status: Option<String>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize, Serialize)]
struct CursorPayload {
    off: u32,
    f: CursorFingerprint,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
struct CursorFingerprint {
    qh: String,
    r#type: SearchTypeFilter,
    #[serde(skip_serializing_if = "Option::is_none")]
    ws: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pj: Option<String>,
    mode: String,
    sh: String,
}

pub async fn query_workspace_search(
    pool: &PgPool,
    input: WorkspaceSearchRequest<'_>,
) -> Result<Result<SearchResultPage, SearchQueryError>, sqlx::Error> {
    let prepared = match prepare_query(&input) {
        Ok(Some(prepared)) => prepared,
        Ok(None) => {
            return Ok(Ok(SearchResultPage {
                items: Vec::new(),
                next_cursor: None,
            }));
        }
        Err(err) => return Ok(Err(err)),
    };

    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, input.workspace_id).await?;
    if !session_is_live(&mut tx, input.actor_user_id, input.session_id).await? {
        tx.rollback().await?;
        return Ok(Err(SearchQueryError::Forbidden));
    }
    if !workspace_is_live(&mut tx, input.workspace_id).await? {
        tx.rollback().await?;
        return Ok(Err(SearchQueryError::NotFound));
    }
    let role = membership_role(&mut tx, input.workspace_id, input.actor_user_id).await?;
    let Some(role) = role else {
        tx.rollback().await?;
        return Ok(Err(SearchQueryError::NotFound));
    };
    let acl = load_search_acl(
        &mut tx,
        input.workspace_id,
        input.actor_user_id,
        role,
        input.project_id,
    )
    .await?;
    tx.commit().await?;

    if input.project_id.is_some() && acl.project_ids.is_empty() {
        return Ok(Ok(SearchResultPage {
            items: Vec::new(),
            next_cursor: None,
        }));
    }

    let filters = search_filters(&input, &acl);
    if let Some(cursor) = input.cursor {
        if !cursor_matches(cursor, &filters) {
            return Ok(Err(SearchQueryError::InvalidCursor));
        }
    }

    let scanned = match scan_lexical(pool, input.meili, &input, &acl, &prepared).await {
        Ok(page) => page,
        // Meili failures are the 503 problem; a database failure stays an error.
        Err(ScanError::Meili(error)) => {
            tracing::warn!(error = %error, "meili search unavailable");
            return Ok(Err(SearchQueryError::MeiliUnavailable));
        }
        Err(ScanError::Db(error)) => return Err(error),
    };
    let items = finish_items(input.workspace_id, &prepared, scanned.items);
    let next_cursor = scanned.next_off.map(|off| encode_cursor(off, &filters));
    Ok(Ok(SearchResultPage { items, next_cursor }))
}

pub async fn query_global_search(
    pool: &PgPool,
    input: GlobalSearchRequest<'_>,
) -> Result<Result<SearchResultPage, SearchQueryError>, sqlx::Error> {
    let prepared = match prepare_global_query(&input) {
        Ok(Some(prepared)) => prepared,
        Ok(None) => {
            return Ok(Ok(SearchResultPage {
                items: Vec::new(),
                next_cursor: None,
            }));
        }
        Err(err) => return Ok(Err(err)),
    };

    let visible = load_visible_acls(pool, input.actor_user_id, input.session_id).await?;
    if visible.is_empty() {
        return Ok(Ok(SearchResultPage {
            items: Vec::new(),
            next_cursor: None,
        }));
    }

    let filters = global_search_filters(&input, &visible);
    if let Some(cursor) = input.cursor {
        if !cursor_matches(cursor, &filters) {
            return Ok(Err(SearchQueryError::InvalidCursor));
        }
    }

    let scanned = match scan_lexical_global(pool, input.meili, &input, &visible, &prepared).await {
        Ok(page) => page,
        Err(ScanError::Meili(error)) => {
            tracing::warn!(error = %error, "meili search unavailable");
            return Ok(Err(SearchQueryError::MeiliUnavailable));
        }
        Err(ScanError::Db(error)) => return Err(error),
    };
    let items = finish_items_global(&prepared, scanned.items);
    let next_cursor = scanned.next_off.map(|off| encode_cursor(off, &filters));
    Ok(Ok(SearchResultPage { items, next_cursor }))
}

fn prepare_global_query(
    input: &GlobalSearchRequest<'_>,
) -> Result<Option<PreparedQuery>, SearchQueryError> {
    let limit = input.limit.clamp(1, 50);
    let raw = input.q.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let raw: String = raw.chars().take(200).collect();
    let title_prefix = raw
        .strip_prefix('^')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(200).collect::<String>());
    let q = title_prefix.clone().unwrap_or(raw);
    if q.is_empty() {
        return Ok(None);
    }
    let chosung = is_chosung_query(&q);
    if chosung && q.chars().filter(|c| !c.is_whitespace()).count() < CHOSUNG_MIN_LENGTH {
        return Ok(None);
    }
    if chosung && input.r#type == SearchTypeFilter::Attachment {
        return Ok(None);
    }
    let stem = if chosung {
        String::new()
    } else {
        stem_text(&q)
    };
    let offset = match input.cursor {
        None => 0,
        Some(cursor) => decode_cursor_offset(cursor)?,
    };
    Ok(Some(PreparedQuery {
        q,
        stem,
        chosung,
        title_prefix,
        r#type: input.r#type,
        limit,
        offset,
    }))
}

fn prepare_query(
    input: &WorkspaceSearchRequest<'_>,
) -> Result<Option<PreparedQuery>, SearchQueryError> {
    let limit = input.limit.clamp(1, 50);
    let raw = input.q.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let raw: String = raw.chars().take(200).collect();
    let title_prefix = raw
        .strip_prefix('^')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(200).collect::<String>());
    let q = title_prefix.clone().unwrap_or(raw);
    if q.is_empty() {
        return Ok(None);
    }
    let chosung = is_chosung_query(&q);
    if chosung && q.chars().filter(|c| !c.is_whitespace()).count() < CHOSUNG_MIN_LENGTH {
        return Ok(None);
    }
    if chosung && input.r#type == SearchTypeFilter::Attachment {
        return Ok(None);
    }
    let stem = if chosung {
        String::new()
    } else {
        stem_text(&q)
    };
    let offset = match input.cursor {
        None => 0,
        Some(cursor) => decode_cursor_offset(cursor)?,
    };
    Ok(Some(PreparedQuery {
        q,
        stem,
        chosung,
        title_prefix,
        r#type: input.r#type,
        limit,
        offset,
    }))
}

async fn load_search_acl(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    role: WorkspaceRole,
    project_filter: Option<Uuid>,
) -> Result<SearchAcl, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            String,
            Uuid,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(
        r#"
        SELECT id, key, name, description, icon, visibility, root_document_id, status,
               created_by, created_at, updated_at
        FROM fvoci.projects
        WHERE workspace_id = $1 AND deleted_at IS NULL
        ORDER BY key COLLATE "C"
        "#,
    )
    .bind(workspace_id)
    .fetch_all(&mut **tx)
    .await?;

    let mut project_ids = Vec::new();
    for row in rows {
        let locked = LockedProject {
            id: row.0,
            key: row.1,
            name: row.2,
            description: row.3,
            icon: row.4,
            visibility: row.5,
            root_document_id: row.6,
            status: row.7,
            created_by: row.8,
            created_at: row.9,
            updated_at: row.10,
        };
        let permission = project_permission(tx, workspace_id, actor_user_id, &locked).await?;
        if permission.at_least(ProjectPermission::View) {
            project_ids.push(locked.id);
        }
    }
    let wiki_document_ids = if role == WorkspaceRole::Guest {
        sqlx::query_as::<_, (Uuid,)>(&guest_wiki_document_ids_select_sql(1, 2))
            .bind(workspace_id)
            .bind(actor_user_id)
            .fetch_all(&mut **tx)
            .await?
            .into_iter()
            .map(|(id,)| id)
            .collect()
    } else {
        Vec::new()
    };
    let include_wiki = role != WorkspaceRole::Guest;
    Ok(restrict_search_acl(
        SearchAcl {
            project_ids,
            include_wiki,
            wiki_document_ids,
        },
        project_filter,
    ))
}

fn restrict_search_acl(acl: SearchAcl, project_filter: Option<Uuid>) -> SearchAcl {
    match project_filter {
        None => acl,
        Some(project_id) if acl.project_ids.contains(&project_id) => SearchAcl {
            project_ids: vec![project_id],
            include_wiki: false,
            wiki_document_ids: Vec::new(),
        },
        Some(_) => SearchAcl {
            project_ids: Vec::new(),
            include_wiki: false,
            wiki_document_ids: Vec::new(),
        },
    }
}

fn scope_key(acl: &SearchAcl) -> String {
    let mut projects: Vec<String> = acl.project_ids.iter().map(ToString::to_string).collect();
    projects.sort();
    let mut wiki: Vec<String> = acl
        .wiki_document_ids
        .iter()
        .map(ToString::to_string)
        .collect();
    wiki.sort();
    format!(
        "{}:{}:{}",
        if acl.include_wiki { "1" } else { "0" },
        projects.join(","),
        wiki.join(",")
    )
}

fn global_scope_key(scopes: &[(Uuid, String)]) -> String {
    let mut sorted = scopes.to_vec();
    sorted.sort_by_key(|a| a.0);
    sorted
        .iter()
        .map(|(workspace_id, key)| format!("{workspace_id}:{key}"))
        .collect::<Vec<_>>()
        .join("|")
}

fn global_search_filters(
    input: &GlobalSearchRequest<'_>,
    visible: &[VisibleWorkspaceAcl],
) -> CursorFingerprint {
    let scope = global_scope_key(
        &visible
            .iter()
            .map(|entry| (entry.workspace_id, scope_key(&entry.acl)))
            .collect::<Vec<_>>(),
    );
    let qh_src = match input.tag {
        Some(tag) => format!("{}\0tag:{tag}", input.q),
        None => input.q.to_string(),
    };
    CursorFingerprint {
        qh: fnv1a(&qh_src),
        r#type: input.r#type,
        ws: None,
        pj: None,
        mode: "lexical".to_string(),
        sh: fnv1a(&scope),
    }
}

async fn load_visible_acls(
    pool: &PgPool,
    actor_user_id: Uuid,
    session_id: Uuid,
) -> Result<Vec<VisibleWorkspaceAcl>, sqlx::Error> {
    let mut listed = list_workspaces_for_user(pool, actor_user_id).await?;
    listed.sort_by(|a, b| a.slug.cmp(&b.slug).then_with(|| a.id.cmp(&b.id)));
    let mut visible = Vec::new();
    for workspace in listed {
        let mut tx = pool.begin().await?;
        set_tenant(&mut tx, workspace.id).await?;
        if !session_is_live(&mut tx, actor_user_id, session_id).await? {
            tx.rollback().await?;
            continue;
        }
        if !workspace_is_live(&mut tx, workspace.id).await? {
            tx.rollback().await?;
            continue;
        }
        let role = membership_role(&mut tx, workspace.id, actor_user_id).await?;
        let Some(role) = role else {
            tx.rollback().await?;
            continue;
        };
        let acl = load_search_acl(&mut tx, workspace.id, actor_user_id, role, None).await?;
        tx.commit().await?;
        visible.push(VisibleWorkspaceAcl {
            workspace_id: workspace.id,
            acl,
        });
    }
    Ok(visible)
}

fn search_filters(input: &WorkspaceSearchRequest<'_>, acl: &SearchAcl) -> CursorFingerprint {
    let qh_src = match input.tag {
        Some(tag) => format!("{}\0tag:{tag}", input.q),
        None => input.q.to_string(),
    };
    CursorFingerprint {
        qh: fnv1a(&qh_src),
        r#type: input.r#type,
        ws: Some(input.workspace_id.to_string()),
        pj: input.project_id.map(|id| id.to_string()),
        mode: "lexical".to_string(),
        sh: fnv1a(&scope_key(acl)),
    }
}

fn fnv1a(s: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for unit in s.encode_utf16() {
        h ^= u32::from(unit);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("{h:08x}")
}

fn encode_cursor(off: u32, filters: &CursorFingerprint) -> String {
    let payload = CursorPayload {
        off,
        f: filters.clone(),
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).expect("cursor json"))
}

fn decode_cursor_offset(cursor: &str) -> Result<u32, SearchQueryError> {
    if cursor.len() > 1024 {
        return Err(SearchQueryError::InvalidCursor);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor.as_bytes())
        .map_err(|_| SearchQueryError::InvalidCursor)?;
    let payload: CursorPayload =
        serde_json::from_slice(&bytes).map_err(|_| SearchQueryError::InvalidCursor)?;
    if payload.off > MEILI_MAX_TOTAL_HITS {
        return Err(SearchQueryError::InvalidCursor);
    }
    Ok(payload.off)
}

fn cursor_matches(cursor: &str, filters: &CursorFingerprint) -> bool {
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(cursor.as_bytes()) else {
        return false;
    };
    let Ok(payload) = serde_json::from_slice::<CursorPayload>(&bytes) else {
        return false;
    };
    payload.f == *filters
}

fn meili_scope(workspace_id: Uuid, acl: &SearchAcl) -> MeiliSearchScope {
    MeiliSearchScope {
        workspace_id: workspace_id.to_string(),
        project_ids: acl.project_ids.iter().map(ToString::to_string).collect(),
        include_wiki: acl.include_wiki,
        wiki_document_ids: acl
            .wiki_document_ids
            .iter()
            .map(ToString::to_string)
            .collect(),
    }
}

struct ScannedPage {
    items: Vec<(MeiliHit, HydratedRow)>,
    next_off: Option<u32>,
}

struct ScannedGlobalPage {
    items: Vec<(MeiliHit, HydratedRow)>,
    next_off: Option<u32>,
}

async fn scan_lexical_global(
    pool: &PgPool,
    meili: &MeiliConfig,
    input: &GlobalSearchRequest<'_>,
    visible: &[VisibleWorkspaceAcl],
    prepared: &PreparedQuery,
) -> Result<ScannedGlobalPage, ScanError> {
    let workspace_ids: HashSet<String> = visible
        .iter()
        .map(|entry| entry.workspace_id.to_string())
        .collect();
    let scopes: Vec<MeiliSearchScope> = visible
        .iter()
        .map(|entry| meili_scope(entry.workspace_id, &entry.acl))
        .collect();
    let acl_by_ws: HashMap<Uuid, SearchAcl> = visible
        .iter()
        .map(|entry| (entry.workspace_id, entry.acl.clone()))
        .collect();
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut offset = prepared.offset;
    let mut scanned = 0u32;
    let mut pages = 0u32;
    let t0 = Instant::now();
    let mut meili_exhausted = false;

    while items.len() < prepared.limit as usize
        && scanned < MAX_CANDIDATES
        && pages < MAX_MEILI_PAGES
        && t0.elapsed().as_millis() < u128::from(MAX_SEARCH_MS)
        && offset < MEILI_MAX_TOTAL_HITS
    {
        let want = MEILI_PAGE
            .min(MEILI_MAX_TOTAL_HITS.saturating_sub(offset))
            .min(MAX_CANDIDATES.saturating_sub(scanned));
        if want == 0 {
            break;
        }
        let res = search_meili(
            meili,
            &MeiliSearchInput {
                q: prepared.q.clone(),
                stem: prepared.stem.clone(),
                scopes: scopes.clone(),
                kind: prepared.r#type.meili_kind(),
                limit: want,
                offset,
            },
        )
        .await?;
        pages += 1;
        scanned += res.hits.len() as u32;
        if res.hits.is_empty() {
            meili_exhausted = true;
            break;
        }

        let mut unique = Vec::new();
        for (i, raw) in res.hits.iter().enumerate() {
            if !accept_hit(raw, &workspace_ids, prepared.r#type) {
                continue;
            }
            let key = format!(
                "{}:{}",
                raw.workspace_id,
                hit_key(raw.kind, &raw.resource_id)
            );
            if !seen.insert(key.clone()) {
                continue;
            }
            unique.push((key, raw.clone(), offset + i as u32));
        }

        let mut grouped: HashMap<Uuid, Vec<MeiliHit>> = HashMap::new();
        for (_, hit, _) in &unique {
            let Ok(workspace_id) = Uuid::parse_str(&hit.workspace_id) else {
                continue;
            };
            grouped.entry(workspace_id).or_default().push(hit.clone());
        }

        let mut by_key = HashMap::new();
        for (workspace_id, hits) in grouped {
            let Some(acl) = acl_by_ws.get(&workspace_id) else {
                continue;
            };
            let rows = hydrate_hits_for_workspace(
                pool,
                workspace_id,
                input.actor_user_id,
                input.session_id,
                None,
                acl,
                &hits,
            )
            .await
            .map_err(ScanError::Db)?;
            for row in rows {
                by_key.insert(
                    format!(
                        "{workspace_id}:{}",
                        hit_key(kind_of(row.r#type), &row.id.to_string())
                    ),
                    row,
                );
            }
        }

        let mut stopped_mid = false;
        let mut next_off = offset + res.hits.len() as u32;
        for (u, (key, hit, at)) in unique.iter().enumerate() {
            let Some(row) = by_key.get(key) else {
                continue;
            };
            if items.len() >= prepared.limit as usize {
                stopped_mid = true;
                next_off = *at;
                for rest in unique.iter().skip(u) {
                    seen.remove(&rest.0);
                }
                break;
            }
            items.push((hit.clone(), row.clone()));
        }
        if stopped_mid {
            return Ok(ScannedGlobalPage {
                items,
                next_off: Some(next_off),
            });
        }
        match res.next_offset {
            None => {
                meili_exhausted = true;
                break;
            }
            Some(next) => offset = next,
        }
    }

    let next_off = if items.len() == prepared.limit as usize && !meili_exhausted {
        Some(offset)
    } else {
        None
    };
    Ok(ScannedGlobalPage { items, next_off })
}

async fn scan_lexical(
    pool: &PgPool,
    meili: &MeiliConfig,
    input: &WorkspaceSearchRequest<'_>,
    acl: &SearchAcl,
    prepared: &PreparedQuery,
) -> Result<ScannedPage, ScanError> {
    let workspace_ids = HashSet::from([input.workspace_id.to_string()]);
    let scopes = [meili_scope(input.workspace_id, acl)];
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let mut offset = prepared.offset;
    let mut scanned = 0u32;
    let mut pages = 0u32;
    let t0 = Instant::now();
    let mut meili_exhausted = false;

    while items.len() < prepared.limit as usize
        && scanned < MAX_CANDIDATES
        && pages < MAX_MEILI_PAGES
        && t0.elapsed().as_millis() < u128::from(MAX_SEARCH_MS)
        && offset < MEILI_MAX_TOTAL_HITS
    {
        let want = MEILI_PAGE
            .min(MEILI_MAX_TOTAL_HITS.saturating_sub(offset))
            .min(MAX_CANDIDATES.saturating_sub(scanned));
        if want == 0 {
            break;
        }
        let res = search_meili(
            meili,
            &MeiliSearchInput {
                q: prepared.q.clone(),
                stem: prepared.stem.clone(),
                scopes: scopes.to_vec(),
                kind: prepared.r#type.meili_kind(),
                limit: want,
                offset,
            },
        )
        .await?;
        pages += 1;
        scanned += res.hits.len() as u32;
        if res.hits.is_empty() {
            meili_exhausted = true;
            break;
        }

        let mut unique = Vec::new();
        for (i, raw) in res.hits.iter().enumerate() {
            if !accept_hit(raw, &workspace_ids, prepared.r#type) {
                continue;
            }
            let key = hit_key(raw.kind, &raw.resource_id);
            if !seen.insert(key.clone()) {
                continue;
            }
            unique.push((key, raw.clone(), offset + i as u32));
        }

        let hits: Vec<MeiliHit> = unique.iter().map(|(_, hit, _)| hit.clone()).collect();
        let rows = hydrate_hits(pool, input, acl, &hits)
            .await
            .map_err(ScanError::Db)?;
        let mut by_key = HashMap::new();
        for row in rows {
            by_key.insert(hit_key(kind_of(row.r#type), &row.id.to_string()), row);
        }

        let mut stopped_mid = false;
        let mut next_off = offset + res.hits.len() as u32;
        for (u, (key, hit, at)) in unique.iter().enumerate() {
            let Some(row) = by_key.get(key) else {
                continue;
            };
            if items.len() >= prepared.limit as usize {
                stopped_mid = true;
                next_off = *at;
                for rest in unique.iter().skip(u) {
                    seen.remove(&rest.0);
                }
                break;
            }
            items.push((hit.clone(), row.clone()));
        }
        if stopped_mid {
            return Ok(ScannedPage {
                items,
                next_off: Some(next_off),
            });
        }
        match res.next_offset {
            None => {
                meili_exhausted = true;
                break;
            }
            Some(next) => offset = next,
        }
    }

    let next_off = if items.len() == prepared.limit as usize && !meili_exhausted {
        Some(offset)
    } else {
        None
    };
    Ok(ScannedPage { items, next_off })
}

fn accept_hit(hit: &MeiliHit, workspace_ids: &HashSet<String>, r#type: SearchTypeFilter) -> bool {
    workspace_ids.contains(&hit.workspace_id)
        && r#type.matches_kind(hit.kind)
        && Uuid::parse_str(&hit.resource_id).is_ok()
        && hit.score.is_finite()
}

fn hit_key(kind: SearchSourceKind, id: &str) -> String {
    format!("{}:{id}", kind.as_str())
}

fn kind_of(r#type: SearchTypeFilter) -> SearchSourceKind {
    match r#type {
        SearchTypeFilter::Document | SearchTypeFilter::All => SearchSourceKind::Document,
        SearchTypeFilter::Task => SearchSourceKind::Task,
        SearchTypeFilter::Attachment => SearchSourceKind::Attachment,
        SearchTypeFilter::Comment => SearchSourceKind::Comment,
    }
}

fn finish_items(
    workspace_id: Uuid,
    prepared: &PreparedQuery,
    rows: Vec<(MeiliHit, HydratedRow)>,
) -> Vec<SearchResultItem> {
    let prefix = prepared.title_prefix.as_ref().map(|s| s.to_lowercase());
    rows.into_iter()
        .filter(|(_, row)| match &prefix {
            None => true,
            Some(p) if p.is_empty() => true,
            Some(p) => row.title.to_lowercase().starts_with(p),
        })
        .map(|(hit, row)| {
            let snippet = if prepared.chosung {
                None
            } else {
                let html = highlight_snippet(&row.body, &prepared.q)
                    .or_else(|| highlight_snippet(&row.title, &prepared.q));
                html.and_then(|h| {
                    let pieces = parse_snippet(&h);
                    let joined: String = pieces.iter().map(|p| p.text.as_str()).collect();
                    if joined == row.title {
                        None
                    } else {
                        Some(pieces)
                    }
                })
            };
            let display_id = match (row.project_key.as_deref(), row.number, row.project_id) {
                (Some(key), Some(n), Some(_)) => Some(format_display_id(key, n)),
                (None, Some(n), None) => Some(format_display_id("WIKI", n)),
                _ => None,
            };
            SearchResultItem {
                r#type: row.r#type,
                id: row.id,
                title: row.title,
                display_id,
                extract_status: row.extract_status,
                chunk_no: if row.r#type == SearchTypeFilter::Attachment {
                    hit.chunk_no
                } else {
                    None
                },
                snippet,
                project_id: row.project_id,
                document_id: row.document_id,
                task_id: row.task_id,
                score: hit.score,
                updated_at: row.updated_at,
                workspace_id,
            }
        })
        .collect()
}

fn finish_items_global(
    prepared: &PreparedQuery,
    rows: Vec<(MeiliHit, HydratedRow)>,
) -> Vec<SearchResultItem> {
    rows.into_iter()
        .filter_map(|(hit, row)| {
            let workspace_id = Uuid::parse_str(&hit.workspace_id).ok()?;
            finish_items(workspace_id, prepared, vec![(hit, row)])
                .into_iter()
                .next()
        })
        .collect()
}

async fn hydrate_hits(
    pool: &PgPool,
    input: &WorkspaceSearchRequest<'_>,
    acl: &SearchAcl,
    hits: &[MeiliHit],
) -> Result<Vec<HydratedRow>, sqlx::Error> {
    hydrate_hits_for_workspace(
        pool,
        input.workspace_id,
        input.actor_user_id,
        input.session_id,
        input.project_id,
        acl,
        hits,
    )
    .await
}

async fn hydrate_hits_for_workspace(
    pool: &PgPool,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    session_id: Uuid,
    project_filter: Option<Uuid>,
    acl: &SearchAcl,
    hits: &[MeiliHit],
) -> Result<Vec<HydratedRow>, sqlx::Error> {
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    if !session_is_live(&mut tx, actor_user_id, session_id).await? {
        tx.rollback().await?;
        return Ok(Vec::new());
    }
    let rows = hydrate_in_tx(
        &mut tx,
        workspace_id,
        actor_user_id,
        project_filter,
        acl,
        hits,
    )
    .await?;
    tx.commit().await?;
    Ok(rows)
}

async fn hydrate_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project_filter: Option<Uuid>,
    acl: &SearchAcl,
    hits: &[MeiliHit],
) -> Result<Vec<HydratedRow>, sqlx::Error> {
    let mut loaded = HashMap::new();
    let doc_ids: Vec<Uuid> = hits
        .iter()
        .filter(|h| h.kind == SearchSourceKind::Document)
        .filter_map(|h| Uuid::parse_str(&h.resource_id).ok())
        .collect();
    if !doc_ids.is_empty() {
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                Option<Uuid>,
                i32,
                DateTime<Utc>,
                Option<String>,
            ),
        >(
            r#"
            SELECT d.id, d.title, d.text, d.project_id, d.number,
                   date_trunc('milliseconds', d.updated_at), p.key
            FROM fvoci.documents d
            LEFT JOIN fvoci.projects p
              ON p.workspace_id = d.workspace_id AND p.id = d.project_id AND p.deleted_at IS NULL
            WHERE d.workspace_id = $1
              AND d.deleted_at IS NULL
              AND d.status <> 'archived'
              AND d.id = ANY($2)
            "#,
        )
        .bind(workspace_id)
        .bind(&doc_ids)
        .fetch_all(&mut **tx)
        .await?;
        for (id, title, body, project_id, number, updated_at, project_key) in rows {
            if !visible_after_hydrate(
                tx,
                workspace_id,
                actor_user_id,
                project_filter,
                acl,
                project_id,
                Some(id),
            )
            .await?
            {
                continue;
            }
            loaded.insert(
                hit_key(SearchSourceKind::Document, &id.to_string()),
                HydratedRow {
                    r#type: SearchTypeFilter::Document,
                    id,
                    title,
                    body,
                    project_id,
                    document_id: Some(id),
                    task_id: None,
                    number: Some(number),
                    project_key,
                    extract_status: None,
                    updated_at,
                },
            );
        }
    }

    let task_ids: Vec<Uuid> = hits
        .iter()
        .filter(|h| h.kind == SearchSourceKind::Task)
        .filter_map(|h| Uuid::parse_str(&h.resource_id).ok())
        .collect();
    if !task_ids.is_empty() {
        let rows = sqlx::query_as::<_, (Uuid, String, Uuid, i32, DateTime<Utc>, Option<String>)>(
            r#"
            SELECT t.id, t.title, t.project_id, t.number,
                   date_trunc('milliseconds', t.updated_at), p.key
            FROM fvoci.tasks t
            LEFT JOIN fvoci.projects p
              ON p.workspace_id = t.workspace_id AND p.id = t.project_id AND p.deleted_at IS NULL
            WHERE t.workspace_id = $1
              AND t.deleted_at IS NULL
              AND t.archived_at IS NULL
              AND t.id = ANY($2)
            "#,
        )
        .bind(workspace_id)
        .bind(&task_ids)
        .fetch_all(&mut **tx)
        .await?;
        for (id, title, project_id, number, updated_at, project_key) in rows {
            if !visible_after_hydrate(
                tx,
                workspace_id,
                actor_user_id,
                project_filter,
                acl,
                Some(project_id),
                None,
            )
            .await?
            {
                continue;
            }
            loaded.insert(
                hit_key(SearchSourceKind::Task, &id.to_string()),
                HydratedRow {
                    r#type: SearchTypeFilter::Task,
                    id,
                    title: title.clone(),
                    body: title,
                    project_id: Some(project_id),
                    document_id: None,
                    task_id: Some(id),
                    number: Some(number),
                    project_key,
                    extract_status: None,
                    updated_at,
                },
            );
        }
    }

    let att_ids: Vec<Uuid> = hits
        .iter()
        .filter(|h| h.kind == SearchSourceKind::Attachment)
        .filter_map(|h| Uuid::parse_str(&h.resource_id).ok())
        .collect();
    if !att_ids.is_empty() {
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                String,
                Uuid,
                Option<Uuid>,
                i32,
                DateTime<Utc>,
                Option<String>,
            ),
        >(
            r#"
            SELECT a.id, a.name, a.extract_text, a.extract_status, a.document_id, d.project_id,
                   d.number, date_trunc('milliseconds', a.created_at), p.key
            FROM fvoci.attachments a
            JOIN fvoci.documents d
              ON d.workspace_id = a.workspace_id AND d.id = a.document_id
            LEFT JOIN fvoci.projects p
              ON p.workspace_id = d.workspace_id AND p.id = d.project_id AND p.deleted_at IS NULL
            WHERE a.workspace_id = $1
              AND a.status = 'stored'
              AND a.scan_status <> 'infected'
              AND d.deleted_at IS NULL
              AND d.status <> 'archived'
              AND a.id = ANY($2)
            "#,
        )
        .bind(workspace_id)
        .bind(&att_ids)
        .fetch_all(&mut **tx)
        .await?;
        for (
            id,
            title,
            body,
            extract_status,
            document_id,
            project_id,
            number,
            updated_at,
            project_key,
        ) in rows
        {
            if !visible_after_hydrate(
                tx,
                workspace_id,
                actor_user_id,
                project_filter,
                acl,
                project_id,
                Some(document_id),
            )
            .await?
            {
                continue;
            }
            loaded.insert(
                hit_key(SearchSourceKind::Attachment, &id.to_string()),
                HydratedRow {
                    r#type: SearchTypeFilter::Attachment,
                    id,
                    title,
                    body,
                    project_id,
                    document_id: Some(document_id),
                    task_id: None,
                    number: Some(number),
                    project_key,
                    extract_status: Some(extract_status),
                    updated_at,
                },
            );
        }
    }

    let comment_ids: Vec<Uuid> = hits
        .iter()
        .filter(|h| h.kind == SearchSourceKind::Comment)
        .filter_map(|h| Uuid::parse_str(&h.resource_id).ok())
        .collect();
    if !comment_ids.is_empty() {
        let rows = sqlx::query_as::<
            _,
            (
                Uuid,
                String,
                String,
                Option<Uuid>,
                Option<Uuid>,
                Option<Uuid>,
                i32,
                DateTime<Utc>,
                Option<String>,
            ),
        >(
            r#"
            SELECT c.id, d.title, c.body, d.project_id, c.document_id, NULL::uuid AS task_id,
                   d.number, date_trunc('milliseconds', c.updated_at), p.key
            FROM fvoci.comments c
            JOIN fvoci.documents d
              ON d.workspace_id = c.workspace_id AND d.id = c.document_id
            LEFT JOIN fvoci.projects p
              ON p.workspace_id = d.workspace_id AND p.id = d.project_id AND p.deleted_at IS NULL
            WHERE c.workspace_id = $1
              AND c.document_id IS NOT NULL
              AND d.deleted_at IS NULL
              AND d.status <> 'archived'
              AND c.id = ANY($2)
            UNION ALL
            SELECT c.id, t.title, c.body, t.project_id, NULL::uuid AS document_id, c.task_id,
                   t.number, date_trunc('milliseconds', c.updated_at), p.key
            FROM fvoci.comments c
            JOIN fvoci.tasks t
              ON t.workspace_id = c.workspace_id AND t.id = c.task_id
            LEFT JOIN fvoci.projects p
              ON p.workspace_id = t.workspace_id AND p.id = t.project_id AND p.deleted_at IS NULL
            WHERE c.workspace_id = $1
              AND c.task_id IS NOT NULL
              AND t.deleted_at IS NULL
              AND t.archived_at IS NULL
              AND c.id = ANY($2)
            "#,
        )
        .bind(workspace_id)
        .bind(&comment_ids)
        .fetch_all(&mut **tx)
        .await?;
        for (id, title, body, project_id, document_id, task_id, number, updated_at, project_key) in
            rows
        {
            let visible = if document_id.is_some() {
                visible_after_hydrate(
                    tx,
                    workspace_id,
                    actor_user_id,
                    project_filter,
                    acl,
                    project_id,
                    document_id,
                )
                .await?
            } else {
                visible_after_hydrate(
                    tx,
                    workspace_id,
                    actor_user_id,
                    project_filter,
                    acl,
                    project_id,
                    None,
                )
                .await?
            };
            if !visible {
                continue;
            }
            loaded.insert(
                hit_key(SearchSourceKind::Comment, &id.to_string()),
                HydratedRow {
                    r#type: SearchTypeFilter::Comment,
                    id,
                    title,
                    body,
                    project_id,
                    document_id,
                    task_id,
                    number: Some(number),
                    project_key,
                    extract_status: None,
                    updated_at,
                },
            );
        }
    }

    let mut items = Vec::new();
    let mut seen = HashSet::new();
    for hit in hits {
        let key = hit_key(hit.kind, &hit.resource_id);
        if !seen.insert(key.clone()) {
            continue;
        }
        if let Some(row) = loaded.get(&key) {
            items.push(row.clone());
        }
    }
    Ok(items)
}

async fn visible_after_hydrate(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    actor_user_id: Uuid,
    project_filter: Option<Uuid>,
    acl: &SearchAcl,
    project_id: Option<Uuid>,
    document_id: Option<Uuid>,
) -> Result<bool, sqlx::Error> {
    if let Some(requested) = project_filter {
        match project_id {
            Some(current) if current == requested => {}
            _ => return Ok(false),
        }
    }
    match project_id {
        Some(pid) => {
            if !acl.project_ids.contains(&pid) {
                return Ok(false);
            }
            let Some(locked) = load_live_project(tx, workspace_id, pid).await? else {
                return Ok(false);
            };
            let permission = project_permission(tx, workspace_id, actor_user_id, &locked).await?;
            Ok(permission.at_least(ProjectPermission::View))
        }
        None => {
            let Some(document_id) = document_id else {
                return Ok(false);
            };
            if !acl.include_wiki && !acl.wiki_document_ids.contains(&document_id) {
                return Ok(false);
            }
            let permission =
                document_permission(tx, workspace_id, actor_user_id, document_id, true).await?;
            Ok(permission.at_least(ProjectPermission::View))
        }
    }
}

async fn load_live_project(
    tx: &mut Transaction<'_, Postgres>,
    workspace_id: Uuid,
    project_id: Uuid,
) -> Result<Option<LockedProject>, sqlx::Error> {
    let row = sqlx::query_as::<
        _,
        (
            Uuid,
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            Option<Uuid>,
            String,
            Uuid,
            DateTime<Utc>,
            DateTime<Utc>,
        ),
    >(
        r#"
        SELECT id, key, name, description, icon, visibility, root_document_id, status,
               created_by, created_at, updated_at
        FROM fvoci.projects
        WHERE workspace_id = $1 AND id = $2 AND deleted_at IS NULL
        "#,
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.map(
        |(
            id,
            key,
            name,
            description,
            icon,
            visibility,
            root_document_id,
            status,
            created_by,
            created_at,
            updated_at,
        )| LockedProject {
            id,
            key,
            name,
            description,
            icon,
            visibility,
            root_document_id,
            status,
            created_by,
            created_at,
            updated_at,
        },
    ))
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        if let Some(end) = after.find(';') {
            let ent = &after[..end];
            if let Some(hex) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                if let Ok(cp) = u32::from_str_radix(hex, 16) {
                    if let Some(ch) = char::from_u32(cp) {
                        out.push(ch);
                        rest = &after[end + 1..];
                        continue;
                    }
                }
            } else if let Some(dec) = ent.strip_prefix('#') {
                if let Ok(cp) = dec.parse::<u32>() {
                    if let Some(ch) = char::from_u32(cp) {
                        out.push(ch);
                        rest = &after[end + 1..];
                        continue;
                    }
                }
            } else {
                let decoded = match ent {
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "amp" => Some('&'),
                    _ => None,
                };
                if let Some(ch) = decoded {
                    out.push(ch);
                    rest = &after[end + 1..];
                    continue;
                }
            }
        }
        out.push('&');
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Source `highlightSnippet`: NFKC, first token, ±40 window, `span.keyword`.
pub fn highlight_snippet(text: &str, q: &str) -> Option<String> {
    highlight_snippet_window(text, q, 200)
}

fn highlight_snippet_window(text: &str, q: &str, max_chars: usize) -> Option<String> {
    // Offsets follow the source exactly: JS strings index UTF-16 code units, so
    // indexOf/slice and the ±40 / 200 window are measured in UTF-16 units, not
    // bytes (byte offsets dropped most Korean snippets).
    let needle: String = q
        .nfkc()
        .collect::<String>()
        .split_whitespace()
        .next()?
        .to_string();
    if text.is_empty() {
        return None;
    }
    let hay: String = text.nfkc().collect();
    let hay16: Vec<u16> = hay.encode_utf16().collect();
    let lower16: Vec<u16> = hay.to_lowercase().encode_utf16().collect();
    let needle_lower16: Vec<u16> = needle.to_lowercase().encode_utf16().collect();
    let needle_len = needle.encode_utf16().count();
    let idx = index_of_u16(&lower16, &needle_lower16)?;
    let start = idx.saturating_sub(40.min(max_chars));
    let end = hay16.len().min(start + max_chars);
    let match_end = (idx + needle_len).min(end);
    let slice = |from: usize, to: usize| -> String {
        let from = from.min(hay16.len());
        let to = to.clamp(from, hay16.len());
        String::from_utf16_lossy(&hay16[from..to])
    };
    let before = escape_html(&slice(start, idx));
    let matched = escape_html(&slice(idx, match_end));
    let after = escape_html(&slice(match_end, end));
    Some(format!(
        "{before}<span class=\"keyword\">{matched}</span>{after}"
    ))
}

fn index_of_u16(hay: &[u16], needle: &[u16]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    hay.windows(needle.len())
        .position(|window| window == needle)
}

pub fn parse_snippet(html: &str) -> Vec<SnippetPiece> {
    let mut out = Vec::new();
    let mut last = 0usize;
    let open = "<span class=\"keyword\">";
    let close = "</span>";
    let mut rest = html;
    let mut abs = 0usize;
    while let Some(start) = rest.find(open) {
        let abs_start = abs + start;
        if abs_start > last {
            out.push(SnippetPiece {
                text: decode_entities(&html[last..abs_start]),
                r#match: false,
            });
        }
        let after_open = abs_start + open.len();
        let Some(rel_end) = html[after_open..].find(close) else {
            break;
        };
        let abs_end = after_open + rel_end;
        out.push(SnippetPiece {
            text: decode_entities(&html[after_open..abs_end]),
            r#match: true,
        });
        last = abs_end + close.len();
        abs = last;
        rest = &html[last..];
    }
    if last < html.len() {
        out.push(SnippetPiece {
            text: decode_entities(&html[last..]),
            r#match: false,
        });
    }
    out.retain(|p| !p.text.is_empty());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a_is_8_hex() {
        assert_eq!(fnv1a("").len(), 8);
        assert_eq!(fnv1a("검색"), fnv1a("검색"));
        assert_ne!(fnv1a("a"), fnv1a("b"));
    }

    #[test]
    fn highlight_snippet_uses_utf16_offsets_for_korean_and_emoji() {
        // Mirrors the source JS: indexOf/slice in UTF-16 code units.
        let text = format!("{}검색 결과 🙂 끝", "가".repeat(60));
        let html = highlight_snippet(&text, "검색").expect("korean snippet");
        assert!(
            html.contains("<span class=\"keyword\">검색</span>"),
            "{html}"
        );
        // 40 UTF-16 units of context before the match, as in the source.
        let before = html.split("<span").next().unwrap();
        assert_eq!(before.encode_utf16().count(), 40, "{html}");
        let emoji = highlight_snippet("앞 🙂 검색어 뒤", "검색어").expect("emoji snippet");
        assert!(emoji.starts_with("앞 🙂 "), "{emoji}");
    }

    #[test]
    fn snippet_highlights_first_token() {
        let html = highlight_snippet("hello search world", "search").unwrap();
        assert!(html.contains("<span class=\"keyword\">search</span>"));
        let pieces = parse_snippet(&html);
        assert!(pieces.iter().any(|p| p.r#match && p.text == "search"));
    }

    #[test]
    fn snippet_escapes_html() {
        let html = highlight_snippet("<script>alert(1)</script> needle", "needle").unwrap();
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;script&gt;"));
    }

    #[test]
    fn cursor_round_trip_rejects_mismatch() {
        let filters = CursorFingerprint {
            qh: fnv1a("q"),
            r#type: SearchTypeFilter::All,
            ws: Some("11111111-1111-4111-8111-111111111111".into()),
            pj: None,
            mode: "lexical".into(),
            sh: fnv1a("1::"),
        };
        let encoded = encode_cursor(20, &filters);
        assert!(cursor_matches(&encoded, &filters));
        let mut other = filters.clone();
        other.qh = fnv1a("other");
        assert!(!cursor_matches(&encoded, &other));
    }

    #[test]
    fn restrict_unknown_project_is_empty() {
        let acl = SearchAcl {
            project_ids: vec![Uuid::now_v7()],
            include_wiki: true,
            wiki_document_ids: Vec::new(),
        };
        let restricted = restrict_search_acl(acl, Some(Uuid::now_v7()));
        assert!(restricted.project_ids.is_empty());
        assert!(!restricted.include_wiki);
    }
}
