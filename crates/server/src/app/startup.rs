//! The store a server starts with: opened, looked at, brought up to date, and reaching
//! no organization until a request identifies one.

use std::time::Duration;

use anyhow::{Context as _, bail};
use fvoci_store::{DbType, SchemaCheck, Store, StoreError};

use crate::config::{Configuration, Settings};

/// Opens the database of the configuration, and looks at the role the server connects
/// as before anything is made as it: a start that is refused leaves the database as it
/// was. Nothing is changed in it until [`prepare_store`].
///
/// # Errors
///
/// Returns an error if the database cannot be reached, and if the role the server
/// connects as is one the wall between organizations does not hold while the
/// configuration takes in several organizations.
pub(crate) async fn open_store(settings: &Settings) -> anyhow::Result<Store> {
    let store = connect(settings.database_kind(), settings.config()).await?;
    check_the_wall_holds(&store, settings).await?;
    Ok(store)
}

/// Makes the opened database ready for the server, once it holds the lock of the
/// server: brings the schema up to date without choosing or creating an organization.
///
/// # Errors
///
/// Returns an error if the database is not this server's to open, or cannot be
/// brought up to date.
pub(crate) async fn prepare_store(store: Store) -> anyhow::Result<Store> {
    match store.migrate().await {
        Ok(()) => Ok(store),
        Err(error @ StoreError::SchemaNotOwned { .. }) => Err(anyhow::Error::from(error).context(
            "the server does not start: run `fvoci-server --migrate` first, with the \
                 database_connection of the role that owns the tables",
        )),
        Err(error) => Err(error).context("bringing the database's schema up to date"),
    }
}

/// Brings the database's schema up to date and ends: `fvoci-server --migrate`, run as
/// the role that owns the tables when the server connects as one that does not. A
/// schema that is up to date already is left as it is at once, while a server serves
/// it; of a database before the baseline the record is replaced first, which changes no
/// table. Migrations that are still to be applied then are applied under the lock of
/// the server, so that no server serves the database while its tables change: it waits
/// for a server that serves it to stop. It does not look at the wall: the owner that
/// migrates may be a superuser.
///
/// # Errors
///
/// Returns an error if the database cannot be reached, another process serves it for
/// longer than `server_lock_wait_seconds`, or the schema cannot be brought up to date.
pub async fn migrate_database(config: &Configuration) -> anyhow::Result<()> {
    let store = connect(config.database_kind.parse()?, config).await?;
    let migrated = migrate_under_the_lock(&store, config).await;
    store.close().await;
    let done = migrated?;
    tracing::info!(
        role = done.role,
        schema = done.schema,
        changed = done.changed,
        "the database's schema is up to date"
    );
    Ok(())
}

/// Says what [`migrate_database`] would do to the database's schema, and changes
/// nothing: `fvoci-server --migrate --check`, which an update runs with the new image
/// while the old one still serves the database, so that a database the new server would
/// refuse is found before anything stops.
///
/// # Errors
///
/// Returns an error if the database cannot be reached, or the migration would be
/// refused: why, as the migration would say it.
pub async fn check_database(config: &Configuration) -> anyhow::Result<()> {
    let store = connect(config.database_kind.parse()?, config).await?;
    let checked = store.check_schema().await;
    let whereabouts = store.role_and_schema().await;
    store.close().await;
    let change = checked.context("the database's schema cannot be brought up to date")?;
    let (role, schema) =
        whereabouts.context("reading the role and the schema of the connection")?;
    let would = match change {
        SchemaCheck::Current => "leave it: the schema is up to date",
        SchemaCheck::Behind => "apply this server's migrations, holding the lock of the server",
        SchemaCheck::BeforeBaseline => {
            "replace the record of the migrations before the baseline with the baseline's, \
             leaving the schema as it is"
        }
    };
    tracing::info!(role, schema, "the migration would {would}");
    Ok(())
}

/// What a migration did, for its log line.
struct Migrated {
    role: String,
    schema: String,
    /// Whether the schema was changed, or found up to date.
    changed: bool,
}

/// [`migrate_database`] on an open store, which the caller closes. The lock is let go
/// after the migration; the store is still open then, which only the end of a server
/// orders the other way.
async fn migrate_under_the_lock(store: &Store, config: &Configuration) -> anyhow::Result<Migrated> {
    let (role, schema) = store
        .role_and_schema()
        .await
        .context("reading the role and the schema of the connection")?;
    let mut change = store
        .check_schema()
        .await
        .context("reading the record of the database's migrations")?;
    let re_anchored = change == SchemaCheck::BeforeBaseline;
    if re_anchored {
        // Only the record changes: a server that serves the database goes on.
        change = store
            .re_anchor()
            .await
            .context("replacing the record of the migrations before the baseline")?;
    }
    match change {
        SchemaCheck::Current => {}
        // The migrations after the baseline change the tables, of a database before it
        // too.
        SchemaCheck::Behind => {
            let lock = super::take_server_lock(store, config).await.context(
                "the schema is to be changed while a server serves the database: stop it, \
                 then migrate",
            )?;
            let migrated = store.migrate().await;
            lock.release().await;
            migrated.context("bringing the database's schema up to date")?;
        }
        SchemaCheck::BeforeBaseline => anyhow::bail!(
            "the record of the migrations is still the one before the baseline after it was \
             replaced"
        ),
    }
    Ok(Migrated {
        role,
        schema,
        changed: re_anchored || change == SchemaCheck::Behind,
    })
}

/// Connects to the database of kind `db_type`, trying up to `database_connect_attempts`
/// times a second apart, so that a server started together with its database waits for
/// it.
async fn connect(db_type: DbType, config: &Configuration) -> anyhow::Result<Store> {
    let attempts = config.database_connect_attempts.max(1);
    let mut attempt = 1;
    loop {
        match Store::open_with_max_connections(
            db_type,
            &config.database_connection,
            config.database_max_connections,
        )
        .await
        {
            Ok(store) => return Ok(store),
            Err(fvoci_store::StoreError::Database(error)) if attempt < attempts => {
                tracing::warn!(%error, attempt, attempts, "the database is not reachable yet");
                attempt += 1;
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => return Err(error).context("connecting to the database"),
        }
    }
}

/// Looks at the role the server connects to PostgreSQL as: a superuser, or a role with
/// `BYPASSRLS`, passes the policies that keep the rows of the organizations apart, and
/// the safety net below the server's own checks is not there.
///
/// # Errors
///
/// Fails for such a role if `org_creation` is `open`: a server that takes in several
/// organizations does not start without the net. With the one organization of
/// `first_user_only` it starts, and says so (the default user of a database container
/// is a superuser, and a server set up that way goes on working).
async fn check_the_wall_holds(store: &Store, settings: &Settings) -> anyhow::Result<()> {
    let Some(role) = store
        .role_past_the_wall()
        .await
        .context("reading the role of the database connection")?
    else {
        return Ok(());
    };
    if settings.takes_several_organizations() {
        bail!(
            "the database role {role:?} is a superuser or has BYPASSRLS, so the row-level \
             security policies that keep organizations apart do not hold it; with \
             org_creation \"open\" the server connects as a role made for it (LOGIN, \
             NOSUPERUSER, NOBYPASSRLS) that owns its tables"
        );
    }
    tracing::warn!(
        role,
        "the database role is a superuser or has BYPASSRLS: the row-level security \
         policies that keep organizations apart do not hold it. The server works; the \
         safety net below its own checks is off"
    );
    Ok(())
}
