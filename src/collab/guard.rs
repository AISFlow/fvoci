use sqlx::pool::PoolConnection;
use sqlx::postgres::PgConnection;
use sqlx::Postgres;
use uuid::Uuid;

use crate::collab::config::FamilyRoomTimings;
use crate::db::backend::Backend;
use crate::db::collab::COLLAB_ROOM_SESSION_LOCK_NAMESPACE;
use crate::db::collab::{
    activate_family_document_writer, append_collab_update_on_conn_timed,
    append_family_document_room_update_timed, claim_writer_and_load_kind_backend,
    release_family_document_room, renew_family_document_room, AppendCollabInput,
    AppendCollabResult, ClaimWriterResult, CollabDbError, CollabDbStageTimings, CollabKind,
    FamilyRoomDeliveryFence, FamilyRoomFence,
};
use crate::db::context::lock_key_from_uuid;

/// Detached session-level advisory lock for one document room (the room
/// fence). The lock lives with the dedicated connection: [`Self::release`]
/// unlocks it, and dropping the guard closes the connection, which releases it
/// too. The fence-lost path drops the guard for exactly that reason.
pub struct RoomGuard {
    conn: PgConnection,
    document_id: Uuid,
}

impl RoomGuard {
    /// Try to acquire the room fence without blocking. Returns None if another room holds it.
    /// Tests only: production takes the fence through [`Self::try_lock_pooled`]
    /// after a cancellable pool acquire.
    #[cfg(feature = "db-tests")]
    pub async fn try_acquire(
        pool: &sqlx::PgPool,
        document_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        let pooled = pool.acquire().await?;
        Self::try_lock_pooled(pooled, document_id).await
    }

    /// Finish the fence on a connection that has already been acquired.
    /// `pool.acquire()` may be cancelled at shutdown; this step must not be.
    pub async fn try_lock_pooled(
        pooled: PoolConnection<Postgres>,
        document_id: Uuid,
    ) -> Result<Option<Self>, sqlx::Error> {
        let mut conn = pooled.detach();
        let acquired: (bool,) = sqlx::query_as("SELECT pg_try_advisory_lock($1, $2)")
            .bind(COLLAB_ROOM_SESSION_LOCK_NAMESPACE)
            .bind(lock_key_from_uuid(document_id))
            .fetch_one(&mut conn)
            .await?;
        if acquired.0 {
            Ok(Some(Self { conn, document_id }))
        } else {
            Ok(None)
        }
    }

    pub fn connection_mut(&mut self) -> &mut PgConnection {
        &mut self.conn
    }

    pub async fn release(mut self) {
        let _ = sqlx::query("SELECT pg_advisory_unlock($1, $2)")
            .bind(COLLAB_ROOM_SESSION_LOCK_NAMESPACE)
            .bind(lock_key_from_uuid(self.document_id))
            .execute(&mut self.conn)
            .await;
    }

    async fn release_confirmed(mut self) -> Result<(), sqlx::Error> {
        let (released,): (bool,) = sqlx::query_as("SELECT pg_advisory_unlock($1,$2)")
            .bind(COLLAB_ROOM_SESSION_LOCK_NAMESPACE)
            .bind(lock_key_from_uuid(self.document_id))
            .fetch_one(&mut self.conn)
            .await?;
        if !released {
            return Err(sqlx::Error::Protocol(
                "room session guard was already lost".into(),
            ));
        }
        Ok(())
    }
}

/// Closed room ownership handle. PostgreSQL continues to hold its dedicated
/// session connection; a family room retains its opaque authoritative lease.
pub(crate) enum BackendRoomGuard {
    Postgres(RoomGuard),
    Family {
        backend: Backend,
        original: FamilyRoomFence,
        current: FamilyRoomFence,
        writer_owner: Uuid,
        timings: FamilyRoomTimings,
    },
}

impl BackendRoomGuard {
    pub(crate) fn family(
        backend: Backend,
        fence: FamilyRoomFence,
        timings: FamilyRoomTimings,
    ) -> Self {
        Self::Family {
            backend,
            original: fence,
            current: fence,
            writer_owner: Uuid::now_v7(),
            timings,
        }
    }

    pub(crate) fn delivery_fence(&self) -> Option<FamilyRoomDeliveryFence> {
        match self {
            Self::Postgres(_) => None,
            Self::Family {
                original,
                writer_owner,
                ..
            } => Some(FamilyRoomDeliveryFence {
                original: *original,
                writer_owner: *writer_owner,
            }),
        }
    }

    pub(crate) fn renew_interval(&self) -> Option<std::time::Duration> {
        match self {
            Self::Postgres(_) => None,
            Self::Family { timings, .. } => Some(timings.renew()),
        }
    }

    pub(crate) fn family_fence(&self) -> Option<FamilyRoomFence> {
        match self {
            Self::Postgres(_) => None,
            Self::Family { current, .. } => Some(*current),
        }
    }

    pub(crate) async fn claim_writer(
        &mut self,
        backend: &Backend,
        kind: CollabKind,
        workspace: Uuid,
        actor: Uuid,
        credential: Uuid,
        resource: Uuid,
    ) -> Result<Result<ClaimWriterResult, CollabDbError>, sqlx::Error> {
        match self {
            Self::Postgres(_) => {
                claim_writer_and_load_kind_backend(
                    backend, kind, workspace, actor, credential, resource,
                )
                .await
            }
            Self::Family {
                backend,
                original,
                current,
                writer_owner,
                ..
            } => {
                if kind != CollabKind::Document
                    || original.workspace_id != workspace
                    || original.document_id != resource
                {
                    return Ok(Err(CollabDbError::StaleWriter));
                }
                let claimed = activate_family_document_writer(
                    backend,
                    *original,
                    actor,
                    credential,
                    *writer_owner,
                )
                .await?;
                Ok(claimed.map(|claimed| {
                    *current = claimed.fence;
                    claimed.native
                }))
            }
        }
    }

    pub(crate) async fn append(
        &mut self,
        kind: CollabKind,
        input: AppendCollabInput<'_>,
    ) -> Result<
        (
            Result<AppendCollabResult, CollabDbError>,
            CollabDbStageTimings,
        ),
        sqlx::Error,
    > {
        match self {
            Self::Postgres(guard) => {
                append_collab_update_on_conn_timed(guard.connection_mut(), kind, input).await
            }
            Self::Family {
                backend, current, ..
            } => {
                if kind != CollabKind::Document {
                    return Err(sqlx::Error::Protocol(
                        "family task room ownership is pending".into(),
                    ));
                }
                append_family_document_room_update_timed(backend, *current, input).await
            }
        }
    }

    /// Reconcile only the two known tokens of this room's stable activation.
    /// An error is returned to the actor's fence-loss path; it is never treated
    /// as renewal or as permission to broadcast another native frame.
    pub(crate) async fn renew(&mut self) -> Result<bool, sqlx::Error> {
        let Self::Family {
            backend,
            original,
            current,
            writer_owner,
            timings,
        } = self
        else {
            return Ok(true);
        };
        if renew_family_document_room(backend, *current, timings.lease()).await? {
            return Ok(true);
        }
        let mut activated = *original;
        activated.owner_token = *writer_owner;
        if activated == *current {
            return Ok(false);
        }
        if renew_family_document_room(backend, activated, timings.lease()).await? {
            *current = activated;
            return Ok(true);
        }
        Ok(false)
    }

    pub(crate) async fn release(self) -> Result<(), sqlx::Error> {
        match self {
            Self::Postgres(guard) => guard.release_confirmed().await,
            Self::Family {
                backend,
                original,
                current,
                writer_owner,
                ..
            } => {
                if release_family_document_room(&backend, current).await? {
                    return Ok(());
                }
                let mut activated = original;
                activated.owner_token = writer_owner;
                if activated != current {
                    release_family_document_room(&backend, activated).await?;
                }
                Ok(())
            }
        }
    }
}
