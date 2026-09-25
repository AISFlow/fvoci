pub mod api_tokens;
pub mod attachment_extract;
pub mod attachments;
pub mod collab;
pub mod collab_delivery;
pub mod comments;
pub mod context;
pub mod documents;
pub mod groups;
pub mod identity;
pub mod invitations;
pub mod labels;
pub mod lookup;
pub mod migrate;
pub mod milestones;
pub mod notifications;
pub mod outbox;
pub mod outbox_recover;
pub mod pool;
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
