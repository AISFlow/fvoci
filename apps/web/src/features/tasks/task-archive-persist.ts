import type { CollabSession, CollabUser } from "@/features/documents/collab-session";

/**
 * Source task-detail-screen: `if (archived && session) await session.persistNow()`.
 * `pageEditable` is the task-detail page gate (!archived && can edit metadata), not collab badge state.
 * Never-editable / still-connecting: no initial `synced` yet — skip persist, allow archive.
 * After the body room synced once, `synced` stays true; disconnect then blocks archive until reconnect+persist.
 */
export async function persistTaskBodyBeforeArchive(input: {
  pageEditable: boolean;
  session: CollabSession | null;
  collabUser: CollabUser | null;
}): Promise<void> {
  if (!input.pageEditable) return;
  if (!input.collabUser || !input.session) return;
  if (!input.session.synced) return;
  if (input.session.status !== "connected") {
    throw new Error("collab disconnected");
  }
  await input.session.persistNow();
}

/** Persist (when required) then run the archive PATCH; used by UI and ordering tests. */
export async function runArchiveWithBodyPersist(input: {
  pageEditable: boolean;
  session: CollabSession | null;
  collabUser: CollabUser | null;
  archive: () => Promise<void>;
}): Promise<void> {
  await persistTaskBodyBeforeArchive({
    pageEditable: input.pageEditable,
    session: input.session,
    collabUser: input.collabUser,
  });
  await input.archive();
}
