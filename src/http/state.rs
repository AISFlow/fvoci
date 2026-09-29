use std::sync::Arc;

use crate::attachments::{ObjectStorage, UploadLimits};
use crate::auth::AuthService;
use crate::collab::CollabHub;
use crate::documents::markdown_helper::MarkdownHelper;
use crate::http::rate_limit::RateLimiter;
use crate::search::meili::MeiliConfig;
use crate::streams::StreamHub;

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthService>,
    pub branding_name: String,
    /// The public origin as `guard::normalize_public_origin` returns it
    /// (scheme, host, non-default port; no trailing slash). Whoever builds
    /// the state normalizes it.
    pub public_origin: String,
    pub cookie_secure: bool,
    pub rate_limiter: RateLimiter,
    pub storage: ObjectStorage,
    pub upload: UploadLimits,
    pub collab: Option<Arc<CollabHub>>,
    pub meili: Option<MeiliConfig>,
    /// Query-time embedder for workspace `mode=hybrid` (`None` = lexical only).
    pub search_embedder: Option<crate::search::embed::Embedder>,
    /// This binary as the `--internal-markdown` child (Markdown <-> Tiptap,
    /// legal HTML, every document export); `None` = those conversions answer
    /// 500.
    pub markdown: Option<MarkdownHelper>,
    /// Wakes the async import runner; `None` = no runner in this process, so
    /// office-file and notion-zip imports fail as unavailable (source).
    pub import_wake: Option<Arc<tokio::sync::Notify>>,
    /// Whether the HWP/HWPX extractor is configured for office-file imports.
    pub import_extractor_available: bool,
    /// Request-time parse for `preview-html` of a not-yet-extracted
    /// attachment; `None` = such a request answers 413.
    pub preview_extract: Option<crate::attachments::PreviewExtractor>,
    /// Workspace storage and per-upload limits (source `QuotaProvider`).
    pub quota: crate::db::quota::StorageQuota,
    pub mailer: std::sync::Arc<crate::mail::Mailer>,
    pub streams: std::sync::Arc<StreamHub>,
}

impl AppState {
    pub fn fresh_streams() -> std::sync::Arc<StreamHub> {
        StreamHub::new_arc()
    }
}
