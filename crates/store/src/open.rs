//! Opening and migrating a [`Store`] pool.

use sqlx::postgres::PgPoolOptions;
use sqlx::sqlite::SqlitePoolOptions;

use super::{
    Arc, DEFAULT_MAX_CONNECTIONS, DbType, Duration, Inner, Pool, SchemaCheck, Store, StoreError,
    Tenant, body_versions, clock, comment_versions, connect, db, file_versions, schema,
    server_lock, visibility,
};

impl Store {
    /// Brings the database's schema to this server's, when the connected role may
    /// change it: replaces the record of a database before the baseline with the
    /// baseline's, then applies the migrations the database does not have yet; nothing
    /// for a schema that is this server's.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::SchemaNotOwned`] if the schema is to be changed and the
    /// role may not change it (it does not own the tables), [`StoreError::Schema`] if
    /// the database is not this server's to open or the role may not read and write
    /// its tables, and [`StoreError::Database`] if a statement fails.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        self.migrate_with(&schema::STEPS).await
    }

    /// [`Store::migrate`] with the migrations `steps`: this server's, or a test's.
    pub(crate) async fn migrate_with(&self, steps: &[schema::Step]) -> Result<(), StoreError> {
        match self.re_anchor_with(steps).await? {
            SchemaCheck::Current => Ok(()),
            SchemaCheck::Behind => schema::migrate(self, steps).await,
            SchemaCheck::BeforeBaseline => Err(still_before_the_baseline()),
        }
    }

    /// Replaces the record of a database before the baseline with the baseline's, as
    /// [`Store::migrate`] does first, and gives what is left to do: the migrations
    /// after the baseline ([`SchemaCheck::Behind`]), which change the schema, or none.
    /// The schema itself is not changed, so that a server that serves the database
    /// goes on.
    ///
    /// # Errors
    ///
    /// As [`Store::migrate`].
    pub async fn re_anchor(&self) -> Result<SchemaCheck, StoreError> {
        self.re_anchor_with(&schema::STEPS).await
    }

    /// [`Store::re_anchor`] with the migrations `steps`.
    pub(crate) async fn re_anchor_with(
        &self,
        steps: &[schema::Step],
    ) -> Result<SchemaCheck, StoreError> {
        let change = self.schema_change(steps).await?;
        if change != SchemaCheck::BeforeBaseline {
            return Ok(change);
        }
        schema::re_anchor(self, steps).await?;
        match self.schema_change(steps).await? {
            SchemaCheck::BeforeBaseline => Err(still_before_the_baseline()),
            left => Ok(left),
        }
    }

    /// What [`Store::migrate`] would do to the database's schema, changing nothing.
    ///
    /// # Errors
    ///
    /// As [`Store::migrate`], for what it would refuse.
    pub async fn check_schema(&self) -> Result<SchemaCheck, StoreError> {
        self.with_conn(schema::refuse_unrecorded_schemas).await?;
        self.schema_change(&schema::STEPS).await
    }

    /// The change the schema is to have, if the role may make it.
    async fn schema_change(&self, steps: &[schema::Step]) -> Result<SchemaCheck, StoreError> {
        let state = schema::state(self, steps).await?;
        let may_change = schema::may_change(self).await?;
        let (change, reason) = match state {
            schema::SchemaState::Current => {
                if !may_change {
                    let ungranted = schema::ungranted_tables(self).await?;
                    if !ungranted.is_empty() {
                        let (role, schema) = schema::whereabouts(self).await?;
                        return Err(StoreError::Schema(format!(
                            "the role {role} may not read and write the tables {} of the \
                             schema {schema}: grant it SELECT, INSERT, UPDATE and DELETE on \
                             them (SELECT on _sqlx_migrations)",
                            ungranted.join(", ")
                        )));
                    }
                }
                return Ok(SchemaCheck::Current);
            }
            schema::SchemaState::Behind => (
                SchemaCheck::Behind,
                "migrations of this server are not applied yet".to_owned(),
            ),
            schema::SchemaState::BeforeBaseline => (
                SchemaCheck::BeforeBaseline,
                "its record of the schema migrations is the one before the baseline, to be \
                 replaced by the baseline's"
                    .to_owned(),
            ),
            schema::SchemaState::Foreign(reason) if may_change => {
                return Err(StoreError::Schema(reason));
            }
            schema::SchemaState::Foreign(reason) => (SchemaCheck::Current, reason),
        };
        if may_change && change != SchemaCheck::Current {
            return Ok(change);
        }
        let (role, schema) = schema::whereabouts(self).await?;
        Err(StoreError::SchemaNotOwned {
            role,
            schema,
            reason,
        })
    }

    /// Connects to the database and leaves its schema as it is. Nothing but
    /// [`Store::role_past_the_wall`] works on such a store before [`Store::migrate`].
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Config`] if `database_connection` cannot be used, and
    /// [`StoreError::Database`] if the database cannot be reached.
    pub async fn open(db_type: DbType, connection: &str) -> Result<Self, StoreError> {
        Self::open_with_max_connections(db_type, connection, DEFAULT_MAX_CONNECTIONS).await
    }

    /// Connects to the database with an explicit pool size limit (`max_connections`) and
    /// leaves its schema as it is.
    ///
    /// # Errors
    ///
    /// As [`Store::open`].
    pub async fn open_with_max_connections(
        db_type: DbType,
        connection: &str,
        max_connections: u32,
    ) -> Result<Self, StoreError> {
        let max_connections = max_connections.max(1);
        let mut lock_file = None;
        let longest_wait = match db_type {
            DbType::Sqlite => connect::sqlite_settings(connection)?.busy_timeout,
            DbType::Postgres => Duration::ZERO,
        };
        let pool = match db_type {
            DbType::Sqlite => {
                let settings = connect::sqlite_settings(connection)?;
                let database =
                    (!settings.in_memory).then(|| settings.options.get_filename().to_path_buf());
                let options = if settings.in_memory {
                    SqlitePoolOptions::new()
                        .max_connections(1)
                        .min_connections(1)
                        .idle_timeout(None)
                        .max_lifetime(None)
                } else {
                    SqlitePoolOptions::new().max_connections(max_connections)
                };
                let pool = options.connect_with(settings.options).await?;
                lock_file = database.as_deref().map(server_lock::file_beside);
                Pool::Sqlite(pool)
            }
            DbType::Postgres => {
                let settings = connect::postgres_settings(connection)?;
                let mut options = PgPoolOptions::new().max_connections(max_connections);
                if let Some(timeout) = settings.connect_timeout {
                    options = options.acquire_timeout(timeout.max(Duration::from_secs(1)));
                }
                Pool::Postgres(options.connect_with(settings.options).await?)
            }
        };

        Ok(Self {
            tenant: Tenant::Unset,
            noting: db::Noting::Named,
            agent: None,
            inner: Arc::new(Inner {
                pool,
                sqlite_writers: tokio::sync::Mutex::new(()),
                comment_versions_kept: std::sync::atomic::AtomicUsize::new(
                    usize::try_from(comment_versions::DEFAULT_COMMENT_VERSIONS_KEPT)
                        .unwrap_or(usize::MAX),
                ),
                search_outbox: std::sync::atomic::AtomicBool::new(false),
                body_versions_kept: std::sync::atomic::AtomicU32::new(
                    body_versions::DEFAULT_BODY_VERSIONS_KEPT,
                ),
                body_versions_max_bytes: std::sync::atomic::AtomicU64::new(
                    body_versions::DEFAULT_BODY_VERSIONS_MAX_BYTES,
                ),
                file_versions_kept: std::sync::atomic::AtomicU32::new(
                    file_versions::DEFAULT_FILE_VERSIONS_KEPT,
                ),
                observer: std::sync::OnceLock::new(),
                visibility: visibility::Visibility::settling_for(visibility::settle_after(
                    longest_wait,
                )),
                other_spellings: tokio::sync::OnceCell::new(),
                clock: clock::Clock::new(),
                lock_file,
            }),
        })
    }
}

/// The refusal of a record that is the one before the baseline after it was replaced.
fn still_before_the_baseline() -> StoreError {
    StoreError::Schema(
        "the record of the schema migrations is still the one before the baseline after it \
         was replaced: open the database again"
            .to_owned(),
    )
}
