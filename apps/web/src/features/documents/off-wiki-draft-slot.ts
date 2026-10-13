// An OFF draft's tab slot by owner and key, and the export-only draft read
// back from it. Nothing here imports the editor, so a screen without one can
// name and find slots; reading a slot's body needs the editor (off-wiki-draft).

export type OffWikiOwner = {
  actorId: string;
  credentialId: string;
  workspaceId: string;
  targetId: string;
  projectId?: string | null;
  kind?: "document" | "task";
  /** The address the target was opened at (WIKI-12, KEY-3). Stored in the slot
   * outside its key, so a page that no longer finds the target can still name
   * this draft locally. It is never sent to the server. */
  displayId?: string;
};
export const STORAGE_PREFIX = "fvoci:off-wiki:1:";
export function ownerKey(owner: OffWikiOwner): string {
  // sessionStorage is a tab-owned storage area. Neither another credential nor
  // another target can read this slot, including after A -> B -> A navigation.
  const identity = [owner.actorId, owner.credentialId, owner.workspaceId, owner.targetId];
  if (owner.kind === "task") identity.push("task");
  if (owner.projectId) identity.push(owner.projectId);
  return `${STORAGE_PREFIX}${JSON.stringify(identity)}`;
}
/** The key prefix of one actor's and credential's slots: the key spells them first. */
export function retainedScope(actorId: string, credentialId: string): string {
  return `${STORAGE_PREFIX}${JSON.stringify([actorId, credentialId]).slice(0, -1)},`;
}
/** A retired private draft read back for local copy/download only. It is
 * never a live draft or a save base, and nothing here can send it. `unstored`:
 * the tab storage refused it, so only this page's memory holds it. */
export type RetainedOffDraft = {
  owner: OffWikiOwner;
  markdown: string;
  excerpt: string;
  unstored?: true;
};
