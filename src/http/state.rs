use std::sync::Arc;

use crate::attachments::{ObjectStorage, UploadLimits};
use crate::auth::AuthService;
use crate::collab::CollabHub;
use crate::documents::convert::ConvertClient;
use crate::documents::markdown_helper::MarkdownHelper;
use crate::http::rate_limit::RateLimiter;
use crate::search::meili::MeiliConfig;

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthService>,
    pub branding_name: String,
    pub public_origin: String,
    pub cookie_secure: bool,
    pub rate_limiter: RateLimiter,
    pub storage: ObjectStorage,
    pub upload: UploadLimits,
    pub collab: Option<Arc<CollabHub>>,
    pub meili: Option<MeiliConfig>,
    /// Query-time embedder for workspace `mode=hybrid` (`None` = lexical only).
    pub search_embedder: Option<crate::search::embed::Embedder>,
    pub document_convert: Option<ConvertClient>,
    /// This binary as the `--internal-markdown` child (Markdown <-> Tiptap,
    /// legal HTML); `None` = those conversions answer 500.
    pub markdown: Option<MarkdownHelper>,
    /// Wakes the async import runner; `None` = no runner in this process, so
    /// office-file and notion-zip imports fail as unavailable (source).
    pub import_wake: Option<Arc<tokio::sync::Notify>>,
    /// Whether the HWP/HWPX extractor is configured for office-file imports.
    pub import_extractor_available: bool,
    /// Workspace storage and per-upload limits (source `QuotaProvider`).
    pub quota: crate::db::quota::StorageQuota,
    pub mailer: std::sync::Arc<crate::mail::Mailer>,
}
