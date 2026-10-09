//! Named push fan-out and payload projections on the existing operation lease.
use super::backend::OperationTx;
use super::codec::Cell;
use crate::push::db::PUSH_SUBSCRIPTIONS_PER_USER;
use uuid::Uuid;

#[derive(Clone, Copy)]
pub(crate) struct PushBinding {
    pub subscription: Uuid,
    pub user: Uuid,
}

fn decode_binding(cells: &[Cell], user: Uuid) -> Result<Option<PushBinding>, sqlx::Error> {
    let [subscription, owner, session, session_owner] = cells else {
        return Err(sqlx::Error::Protocol(
            "invalid push binding projection width".into(),
        ));
    };
    let subscription = subscription.id()?;
    if owner.id()? != user {
        return Err(sqlx::Error::Protocol(
            "foreign push subscription owner".into(),
        ));
    }
    let Some(_) = session.optional(Cell::id)? else {
        return Ok(None);
    };
    if session_owner.id()? != user {
        return Err(sqlx::Error::Protocol("foreign push session owner".into()));
    }
    Ok(Some(PushBinding { subscription, user }))
}

impl OperationTx<'_, '_> {
    pub(crate) async fn push_user_is_active(&mut self, user: Uuid) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM fvoci.users WHERE id=$1 AND deleted_at IS NULL AND suspended_at IS NULL)")
                .bind(user).fetch_one(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                let rows=tx.query("SELECT 1 FROM users WHERE id=?1 AND deleted_at IS NULL AND suspended_at IS NULL", &[Cell::uuid(user)]).await?;
                Ok(!rows.is_empty())
            }
        }
    }

    pub(crate) async fn enqueue_push_deliveries(
        &mut self,
        event: Uuid,
        workspace: Uuid,
        users: &[Uuid],
    ) -> Result<u64, sqlx::Error> {
        match self {
            Self::Postgres(tx) => Ok(sqlx::query(
                "INSERT INTO fvoci.push_deliveries(event_id,workspace_id,user_id,subscription_id) \
                 SELECT $1,$2,s.user_id,s.id FROM fvoci.push_subscriptions s \
                 JOIN fvoci.sessions se ON se.id=s.session_id \
                 WHERE s.user_id=ANY($3) AND se.revoked_at IS NULL AND se.expires_at>clock_timestamp() \
                 ORDER BY s.user_id,s.id ON CONFLICT(event_id,subscription_id) DO NOTHING")
                .bind(event).bind(workspace).bind(users).execute(&mut ***tx).await?.rows_affected()),
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                tx.require_tenant(workspace)?;
                let mut queued = 0;
                for user in users {
                    let rows=tx.query(
                        "SELECT s.id,s.user_id,s.session_id,se.user_id FROM push_subscriptions s \
                         JOIN sessions se ON se.id=s.session_id AND se.user_id=s.user_id \
                         WHERE s.user_id=?1 AND se.revoked_at IS NULL \
                         AND se.expires_at>unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000 \
                         ORDER BY s.updated_at DESC,s.id DESC LIMIT ?2", &[Cell::uuid(*user),Cell::Integer(PUSH_SUBSCRIPTIONS_PER_USER)]).await?;
                    for row in rows {
                        let cells = (0..4).map(|i| row.cell(i)).collect::<Result<Vec<_>,_>>()?;
                        let Some(binding) = decode_binding(&cells, *user)? else { continue };
                        queued += tx.execute(
                            "INSERT INTO push_deliveries(id,event_id,workspace_id,user_id,subscription_id) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(event_id,subscription_id) DO NOTHING",
                            &[Cell::uuid(Uuid::now_v7()),Cell::uuid(event),Cell::uuid(workspace),Cell::uuid(binding.user),Cell::uuid(binding.subscription)]).await?;
                    }
                }
                Ok(queued)
            }
        }
    }

    pub(crate) async fn push_workspace_label(
        &mut self,
        workspace: Uuid,
    ) -> Result<Option<(String, String)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                sqlx::query_as(
                    "SELECT slug,name FROM fvoci.workspaces WHERE id=$1 AND deleted_at IS NULL",
                )
                .bind(workspace)
                .fetch_optional(&mut ***tx)
                .await
            }
            Self::SqliteFamily(tx) => {
                tx.require_tenant(workspace)?;
                let rows = tx
                    .query(
                        "SELECT slug,name FROM workspaces WHERE id=?1 AND deleted_at IS NULL",
                        &[Cell::uuid(workspace)],
                    )
                    .await?;
                rows.first()
                    .map(|row| Ok((row.cell(0)?.string()?, row.cell(1)?.string()?)))
                    .transpose()
            }
        }
    }

    pub(crate) async fn push_actor_label(
        &mut self,
        actor: Uuid,
    ) -> Result<Option<(String, Option<String>, String)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as("SELECT given_name,family_name,locale FROM fvoci.users WHERE id=$1 AND deleted_at IS NULL")
                .bind(actor).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_system_context()?;
                let rows=tx.query("SELECT given_name,family_name,locale FROM users WHERE id=?1 AND deleted_at IS NULL",&[Cell::uuid(actor)]).await?;
                rows.first().map(|row|Ok((row.cell(0)?.string()?,row.cell(1)?.optional(Cell::string)?,row.cell(2)?.string()?))).transpose()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binding_codec_rejects_wrong_account_type_width_and_accepts_null_session() {
        let user = Uuid::now_v7();
        let cells = [
            Cell::uuid(Uuid::now_v7()),
            Cell::uuid(user),
            Cell::uuid(Uuid::now_v7()),
            Cell::uuid(user),
        ];
        assert!(decode_binding(&cells, user).unwrap().is_some());
        assert!(decode_binding(&cells, Uuid::now_v7()).is_err());
        assert!(decode_binding(&cells[..3], user).is_err());
        for (column, bad) in [
            (0, Cell::text("wrong")),
            (0, Cell::Blob(vec![0; 15])),
            (1, Cell::Null),
            (2, Cell::Integer(1)),
            (3, Cell::uuid(Uuid::now_v7())),
        ] {
            let mut damaged = cells.clone();
            damaged[column] = bad;
            assert!(decode_binding(&damaged, user).is_err(), "column {column}");
        }
        let mut absent = cells.clone();
        absent[2] = Cell::Null;
        assert!(decode_binding(&absent, user).unwrap().is_none());
    }
}

// S06 sender operations; the S05 module above remains an immutable input.
#[derive(Clone, Copy)]
pub(crate) struct PushClaim {
    pub id: Uuid,
    pub event_id: Uuid,
    pub user_id: Uuid,
    pub subscription_id: Uuid,
    pub attempt: i32,
}

fn decode_push_claim(cells: &[Cell]) -> Result<PushClaim, sqlx::Error> {
    let [id, event, user, subscription, attempt, lease, handoff] = cells else {
        return Err(sqlx::Error::Protocol(
            "invalid push claim projection width".into(),
        ));
    };
    let attempt = attempt.int32()?;
    if attempt <= 0 {
        return Err(sqlx::Error::Protocol("invalid push claim attempt".into()));
    }
    lease.datetime()?;
    handoff.optional(Cell::datetime)?;
    Ok(PushClaim {
        id: id.id()?,
        event_id: event.id()?,
        user_id: user.id()?,
        subscription_id: subscription.id()?,
        attempt,
    })
}

impl OperationTx<'_, '_> {
    pub(crate) async fn claim_push_deliveries(
        &mut self,
        batch: i64,
        lease: std::time::Duration,
    ) -> Result<Vec<PushClaim>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => {
                // Original PostgreSQL claim SQL and SKIP LOCKED semantics.
                let rows: Vec<(Uuid, Uuid, Uuid, Uuid, i32)> = sqlx::query_as(
                    r#"
            UPDATE fvoci.push_deliveries AS d
            SET claimed_until = now() + make_interval(secs => $2::double precision),
                attempt = d.attempt + 1
            WHERE d.id IN (
                SELECT id FROM fvoci.push_deliveries
                WHERE claimed_until IS NULL OR claimed_until < now()
                ORDER BY id
                LIMIT $1
                FOR UPDATE SKIP LOCKED
            )
            RETURNING d.id, d.event_id, d.user_id, d.subscription_id, d.attempt
            "#,
                )
                .bind(batch.max(1))
                .bind(lease.as_secs_f64())
                .fetch_all(&mut ***tx)
                .await?;
                Ok(rows
                    .into_iter()
                    .map(
                        |(id, event_id, user_id, subscription_id, attempt)| PushClaim {
                            id,
                            event_id,
                            user_id,
                            subscription_id,
                            attempt,
                        },
                    )
                    .collect())
            }
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                let now = tx.query("SELECT unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000",&[]).await?;
                let now = now
                    .first()
                    .ok_or(sqlx::Error::RowNotFound)?
                    .cell(0)?
                    .integer()?;
                let lease_us = i64::try_from(lease.as_micros())
                    .map_err(|_| sqlx::Error::Protocol("push lease overflow".into()))?;
                let until = now
                    .checked_add(lease_us)
                    .ok_or_else(|| sqlx::Error::Protocol("push lease overflow".into()))?;
                let rows = tx.query("SELECT id FROM push_deliveries WHERE claimed_until IS NULL OR claimed_until<?1 ORDER BY id LIMIT ?2",
                    &[Cell::Integer(now),Cell::Integer(batch.max(1))]).await?;
                let mut out = Vec::with_capacity(rows.len());
                for row in rows {
                    let id = row.cell(0)?.id()?;
                    let claimed = tx.query("UPDATE push_deliveries SET claimed_until=?2,attempt=attempt+1 WHERE id=?1 RETURNING id,event_id,user_id,subscription_id,attempt,claimed_until,handed_off_at",
                        &[Cell::uuid(id),Cell::Integer(until)]).await?;
                    let row = claimed.first().ok_or(sqlx::Error::RowNotFound)?;
                    let cells = (0..7).map(|i| row.cell(i)).collect::<Result<Vec<_>, _>>()?;
                    out.push(decode_push_claim(&cells)?);
                }
                Ok(out)
            }
        }
    }

    pub(crate) async fn push_claim_subscription(
        &mut self,
        claim: &PushClaim,
        workspace: Uuid,
    ) -> Result<Option<(String, String, String)>, sqlx::Error> {
        match self {
            Self::Postgres(tx) => sqlx::query_as("SELECT s.endpoint,s.p256dh,s.auth FROM fvoci.push_subscriptions s JOIN fvoci.sessions se ON se.id=s.session_id WHERE s.id=$1 AND s.user_id=$2 AND se.revoked_at IS NULL AND se.expires_at>clock_timestamp()")
                .bind(claim.subscription_id).bind(claim.user_id).fetch_optional(&mut ***tx).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?; tx.require_system_context()?; tx.require_tenant(workspace)?;
                let rows=tx.query("SELECT s.endpoint,s.p256dh,s.auth,s.session_id,se.user_id FROM push_deliveries d JOIN push_subscriptions s ON s.id=d.subscription_id AND s.user_id=d.user_id JOIN sessions se ON se.id=s.session_id AND se.user_id=s.user_id WHERE d.id=?1 AND d.attempt=?2 AND d.event_id=?3 AND d.user_id=?4 AND d.subscription_id=?5 AND d.workspace_id=?6 AND se.revoked_at IS NULL AND se.expires_at>unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000",
                    &[Cell::uuid(claim.id),Cell::Integer(claim.attempt.into()),Cell::uuid(claim.event_id),Cell::uuid(claim.user_id),Cell::uuid(claim.subscription_id),Cell::uuid(workspace)]).await?;
                rows.first().map(|r|{
                    r.cell(3)?.id()?;
                    if r.cell(4)?.id()?!=claim.user_id {return Err(sqlx::Error::Protocol("foreign push session owner".into()))}
                    Ok((r.cell(0)?.string()?,r.cell(1)?.string()?,r.cell(2)?.string()?))
                }).transpose()
            }
        }
    }

    pub(crate) async fn hand_off_push_claim(
        &mut self,
        claim: &PushClaim,
        workspace: Uuid,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => Ok(sqlx::query("UPDATE fvoci.push_deliveries SET handed_off_at=now() WHERE id=$1 AND attempt=$2 AND workspace_id=$3")
                .bind(claim.id).bind(claim.attempt).bind(workspace).execute(&mut ***tx).await?.rows_affected()==1),
            Self::SqliteFamily(tx) => {
                tx.require_writer()?; tx.require_system_context()?; tx.require_tenant(workspace)?;
                Ok(tx.execute("UPDATE push_deliveries SET handed_off_at=unixepoch()*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000 WHERE id=?1 AND attempt=?2 AND workspace_id=?3 AND event_id=?4 AND user_id=?5 AND subscription_id=?6",
                    &[Cell::uuid(claim.id),Cell::Integer(claim.attempt.into()),Cell::uuid(workspace),Cell::uuid(claim.event_id),Cell::uuid(claim.user_id),Cell::uuid(claim.subscription_id)]).await?==1)
            }
        }
    }

    pub(crate) async fn delete_push_claim(
        &mut self,
        claim: &PushClaim,
    ) -> Result<bool, sqlx::Error> {
        match self {
            Self::Postgres(tx) => Ok(sqlx::query(
                "DELETE FROM fvoci.push_deliveries WHERE id=$1 AND attempt=$2",
            )
            .bind(claim.id)
            .bind(claim.attempt)
            .execute(&mut ***tx)
            .await?
            .rows_affected()
                == 1),
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                Ok(tx.execute("DELETE FROM push_deliveries WHERE id=?1 AND attempt=?2 AND event_id=?3 AND user_id=?4 AND subscription_id=?5",
                    &[Cell::uuid(claim.id),Cell::Integer(claim.attempt.into()),Cell::uuid(claim.event_id),Cell::uuid(claim.user_id),Cell::uuid(claim.subscription_id)]).await?==1)
            }
        }
    }

    pub(crate) async fn remove_expired_push_endpoint(
        &mut self,
        endpoint: &str,
    ) -> Result<u64, sqlx::Error> {
        match self {
            Self::Postgres(tx) => crate::push::db::remove_by_endpoint(tx, endpoint).await,
            Self::SqliteFamily(tx) => {
                tx.require_writer()?;
                tx.require_system_context()?;
                // A genuine expired capability is removed globally, matching PG.
                tx.execute(
                    "DELETE FROM push_subscriptions WHERE endpoint=?1",
                    &[Cell::text(endpoint)],
                )
                .await
            }
        }
    }
}

#[cfg(test)]
mod sender_tests {
    use super::*;
    #[test]
    fn sender_projection_checks_uuid_attempt_nullable_and_lease_storage() {
        let cells = [
            Cell::uuid(Uuid::now_v7()),
            Cell::uuid(Uuid::now_v7()),
            Cell::uuid(Uuid::now_v7()),
            Cell::uuid(Uuid::now_v7()),
            Cell::Integer(1),
            Cell::Integer(1),
            Cell::Null,
        ];
        assert_eq!(decode_push_claim(&cells).unwrap().attempt, 1);
        assert!(decode_push_claim(&cells[..6]).is_err());
        for (column, bad) in [
            (0, Cell::Blob(vec![0; 15])),
            (1, Cell::text("wrong")),
            (2, Cell::Null),
            (3, Cell::Integer(1)),
            (4, Cell::Integer(0)),
            (4, Cell::Integer(i64::from(i32::MAX) + 1)),
            (5, Cell::Null),
            (5, Cell::text("wrong")),
            (6, Cell::text("wrong")),
        ] {
            let mut damaged = cells.clone();
            damaged[column] = bad;
            assert!(decode_push_claim(&damaged).is_err(), "column {column}");
        }
    }
}
