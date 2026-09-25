pub mod account;
pub mod admin;
pub mod api_tokens;
pub mod attachment_extract;
pub mod attachments;
pub mod collab;
pub mod collab_delivery;
pub mod collection_query;
pub mod collections;
pub mod comments;
pub mod context;
pub mod dashboard;
pub mod document_tags;
pub mod documents;
pub mod group_grants;
pub mod groups;
pub mod holidays;
pub mod ics;
pub mod identity;
pub mod import_jobs;
pub mod integrations;
pub mod invitations;
pub mod labels;
pub mod legal;
pub mod lookup;
pub mod magic;
pub mod migrate;
pub mod milestones;
pub mod notifications;
pub mod outbox;
pub mod outbox_recover;
pub mod pool;
pub mod project_clone;
pub mod project_documents;
pub mod project_views;
pub mod projects;
pub mod quota;
pub mod revisions;
pub mod search_index;
pub mod share;
pub mod stars;
pub mod task_activity;
pub mod tasks;
pub mod user_export;
pub mod view_query;
pub mod workspace;

use sqlx::PgPool;

#[derive(Clone)]
pub struct Db {
    pub pool: PgPool,
    /// Instance settings as first resolved by this process (restart badges).
    pub settings_boot: crate::settings::SettingsBoot,
}

impl Db {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            settings_boot: crate::settings::SettingsBoot::default(),
        }
    }
}
