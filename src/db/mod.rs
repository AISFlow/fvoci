pub mod attachment_extract;
pub mod attachments;
pub mod collab;
pub mod collab_delivery;
pub mod context;
pub mod documents;
pub mod identity;
pub mod invitations;
pub mod lookup;
pub mod migrate;
pub mod pool;
pub mod projects;
pub mod quota;
pub mod revisions;
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
