//! VAPID singleton storage. PostgreSQL private access stays behind its existing
//! definers; SQLite-family private access requires system context and mutations
//! keep the same writer reservation through cleanup, event, audit and commit.
use super::backend::OperationTx;
use super::codec::Cell;

impl OperationTx<'_, '_> {
    pub(crate) async fn read_vapid_public_key(&mut self) -> Result<Option<String>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar("SELECT fvoci.app_vapid_public_key()")
                    .fetch_one(&mut ***tx)
                    .await
            }
            Self::SqliteFamily(tx) => {
                let rows = tx
                    .query(
                        "SELECT vapid_public_key FROM instance_config WHERE id=1",
                        &[],
                    )
                    .await?;
                rows.first()
                    .map(|row| row.cell(0)?.optional(Cell::string))
                    .transpose()
                    .map(Option::flatten)
            }
        }
    }

    pub(crate) async fn read_vapid_sealed_private(
        &mut self,
    ) -> Result<Option<String>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar("SELECT fvoci.app_vapid_private_key()")
                    .fetch_one(&mut ***tx)
                    .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                let rows = tx
                    .query(
                        "SELECT vapid_private_key FROM instance_config WHERE id=1",
                        &[],
                    )
                    .await?;
                rows.first()
                    .map(|row| row.cell(0)?.optional(Cell::string))
                    .transpose()
                    .map(Option::flatten)
            }
        }
    }

    /// Match PG040: initialize only when the sealed private column is NULL.
    /// A competing initializer cannot replace the winner's pair.
    pub(crate) async fn init_vapid_if_absent(
        &mut self,
        public: &str,
        sealed: &str,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_scalar("SELECT fvoci.app_init_vapid($1, $2)")
                    .bind(public)
                    .bind(sealed)
                    .fetch_one(&mut ***tx)
                    .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                let changed = tx.execute(
                    "UPDATE instance_config SET vapid_public_key=?1,vapid_private_key=?2 WHERE id=1 AND vapid_private_key IS NULL",
                    &[Cell::text(public), Cell::text(sealed)],
                ).await?;
                Ok(changed == 1)
            }
        }
    }

    /// Only the trusted secrets CLI calls this operation; no user HTTP surface
    /// can grant itself the operator's system context.
    pub(crate) async fn replace_vapid_pair(
        &mut self,
        public: &str,
        sealed: &str,
    ) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query("SELECT fvoci.app_set_vapid($1, $2)")
                    .bind(public)
                    .bind(sealed)
                    .execute(&mut ***tx)
                    .await?;
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                let changed = tx.execute(
                    "UPDATE instance_config SET vapid_public_key=?1,vapid_private_key=?2 WHERE id=1",
                    &[Cell::text(public), Cell::text(sealed)],
                ).await?;
                if changed != 1 {
                    return Err(sqlx::Error::RowNotFound);
                }
            }
        }
        Ok(())
    }

    /// Rotation invalidates subscriptions across every user/workspace. FK
    /// cascades also remove deliveries tied to those subscriptions.
    pub(crate) async fn revoke_vapid_subscriptions(&mut self) -> Result<u64, sqlx::Error> {
        match self {
            Self::Postgres(tx) => Ok(sqlx::query("DELETE FROM fvoci.push_subscriptions")
                .execute(&mut ***tx)
                .await?
                .rows_affected()),
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                tx.execute("DELETE FROM push_subscriptions", &[]).await
            }
        }
    }
}
