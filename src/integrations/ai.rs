//! Document "AI" actions (source `core/ai.ts`). They are local text
//! heuristics over the document's markdown, with no model or paid API:
//! summarize = first 400 characters, generate-tasks = up to 20 headings (h1–h3)
//! or else non-empty lines, suggest-links = other documents the user can see.
//! Source gates them behind `FVOCI_AI_ENABLED` + `FVOCI_AI_SECRET` (503
//! `ai_unavailable` otherwise, after the membership check). Markdown comes from
//! the document convert helper, so it is also required.

use std::sync::LazyLock;

use regex::Regex;
use sqlx::PgPool;
use uuid::Uuid;

use crate::db::context::{session_is_live, set_tenant};
use crate::db::documents::{document_permission, membership_role, workspace_is_live};
use crate::projects::ProjectPermission;

pub const SUMMARY_MAX_CHARS: usize = 400;
pub const TITLES_MAX: usize = 20;

static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^#{1,3}[ \t]+(.+)$").expect("heading regex"));

#[derive(Clone)]
pub struct AiConfig {
    secret: String,
}

impl std::fmt::Debug for AiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AiConfig")
            .field("secret", &"<redacted>")
            .finish()
    }
}

impl AiConfig {
    /// Source: enabled only when `FVOCI_AI_ENABLED` is `1`/`true` and a
    /// non-empty `FVOCI_AI_SECRET` is set.
    pub fn from_env() -> Option<Self> {
        let enabled = std::env::var("FVOCI_AI_ENABLED")
            .map(|v| v.trim() == "1" || v.trim().eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let secret = std::env::var("FVOCI_AI_SECRET")
            .ok()
            .filter(|s| !s.trim().is_empty())?;
        enabled.then_some(Self { secret })
    }

    pub fn new(secret: &str) -> Self {
        Self {
            secret: secret.to_string(),
        }
    }

    /// Source logs `sha256(secret)` per request, never the secret.
    pub fn key_hash(&self) -> String {
        crate::auth::token::hash_token(&self.secret)
    }
}

pub fn summarize(markdown: &str, title: &str) -> String {
    let md = markdown.trim();
    let text = if md.is_empty() { title } else { md };
    text.chars().take(SUMMARY_MAX_CHARS).collect()
}

pub fn titles(markdown: &str, fallback: &str) -> Vec<String> {
    let headings: Vec<String> = HEADING_RE
        .captures_iter(markdown)
        .map(|c| c[1].trim().to_string())
        .filter(|t| !t.is_empty())
        .take(TITLES_MAX)
        .collect();
    if !headings.is_empty() {
        return headings;
    }
    let lines: Vec<String> = markdown
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(TITLES_MAX)
        .map(str::to_string)
        .collect();
    if !lines.is_empty() {
        return lines;
    }
    vec![fallback.to_string()]
}

/// Workspace membership (any role) with a live credential; source `admit`
/// checks it before revealing whether AI is on.
pub async fn is_member(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    session_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let ok = session_is_live(&mut tx, user_id, session_id).await?
        && workspace_is_live(&mut tx, workspace_id).await?
        && membership_role(&mut tx, workspace_id, user_id)
            .await?
            .is_some();
    tx.commit().await?;
    Ok(ok)
}

/// Source `suggestDocumentLinks` over `visibleTreeFor`: live documents other
/// than `document_id` that the user can view, in tree order.
pub async fn visible_document_ids(
    pool: &PgPool,
    workspace_id: Uuid,
    user_id: Uuid,
    document_id: Uuid,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    set_tenant(&mut tx, workspace_id).await?;
    let ids: Vec<Uuid> = sqlx::query_scalar(
        r#"
        SELECT id FROM fvoci.documents
        WHERE workspace_id = $1 AND deleted_at IS NULL AND id <> $2
        ORDER BY project_id NULLS FIRST, sort_key COLLATE "C", id
        "#,
    )
    .bind(workspace_id)
    .bind(document_id)
    .fetch_all(&mut *tx)
    .await?;
    let mut visible = Vec::new();
    for id in ids {
        if document_permission(&mut tx, workspace_id, user_id, id, true)
            .await?
            .at_least(ProjectPermission::View)
        {
            visible.push(id);
        }
    }
    tx.commit().await?;
    Ok(visible)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_clips_chars_and_falls_back_to_title() {
        assert_eq!(summarize("  \n", "제목"), "제목");
        let long = "가".repeat(500);
        assert_eq!(summarize(&long, "t").chars().count(), SUMMARY_MAX_CHARS);
    }

    #[test]
    fn titles_prefer_headings_then_lines_then_fallback() {
        assert_eq!(
            titles("# 하나\ntext\n### 셋\n#### 넷", "f"),
            vec!["하나".to_string(), "셋".to_string()]
        );
        assert_eq!(
            titles("a\n\n b ", "f"),
            vec!["a".to_string(), "b".to_string()]
        );
        assert_eq!(titles("", "f"), vec!["f".to_string()]);
        let many: String = (0..30).map(|i| format!("## h{i}\n")).collect();
        assert_eq!(titles(&many, "f").len(), TITLES_MAX);
    }
}
