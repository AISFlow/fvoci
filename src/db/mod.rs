pub mod api_tokens;
pub mod attachment_extract;
pub mod attachments;
pub mod collab;
pub mod collab_delivery;
pub mod comments;
pub mod context;
pub mod documents;
pub mod group_grants;
pub mod groups;
pub mod holidays;
pub mod ics;
pub mod identity;
pub mod invitations;
pub mod labels;
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
pub mod projects;
pub mod quota;
pub mod revisions;
pub mod search_index;
pub mod tasks;
pub mod workspace;

use sqlx::PgPool;

#[derive(Clone)]
pub struct Db {
    pub pool: PgPool,
}

impl Db {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
