pub mod collab;
pub mod context;
pub mod documents;
pub mod identity;
pub mod migrate;
pub mod pool;
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
