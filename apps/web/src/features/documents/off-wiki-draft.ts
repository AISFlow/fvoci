import * as Y from "yjs";
import { yDocToTiptapJson } from "@fvoci/editor/collab-tiptap";
import { independentDraftBody } from "@fvoci/editor/extract";
import { tiptapDocToMd } from "@fvoci/editor/md";
import { createFvociExtensions } from "@fvoci/editor/tiptap-schema";
import { isTiptapDoc } from "@fvoci/editor/json";
import { z } from "zod";
import type { BodySaveCommand, BodySaveResult, VersionedBody } from "./versioned-body-api";
import type { OffDraftCreateBody, OffDraftCreateResponse } from "./document-api";

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
type FrozenSave = { command: BodySaveCommand; snapshot: string };
export type DraftDestination = {
  projectId: string | null;
  parentId: string | null;
  title: string;
  icon?: string | null;
};
type FrozenDistinct = { projectId: string | null; body: OffDraftCreateBody };
const liveUuid = (value: unknown) =>
  z.string().uuid().safeParse(value).success && value !== "00000000-0000-0000-0000-000000000000";
type StoredDraft = {
  format: 1;
  owner: OffWikiOwner;
  start: VersionedBody;
  mine: string;
  acknowledged: string;
  frozen: FrozenSave | null;
  distinct?: FrozenDistinct | null;
  conflictBackup?: StoredDraft | null;
  comparison?: { start: unknown; mine: unknown; current: unknown } | null;
  sourceBuffer?: { text: string; baseV1: string } | null;
};
const STORAGE_PREFIX = "fvoci:off-wiki:1:";
export function ownerKey(owner: OffWikiOwner): string {
  // sessionStorage is a tab-owned storage area. Neither another credential nor
  // another target can read this slot, including after A -> B -> A navigation.
  const identity = [owner.actorId, owner.credentialId, owner.workspaceId, owner.targetId];
  if (owner.kind === "task") identity.push("task");
  if (owner.projectId) identity.push(owner.projectId);
  return `${STORAGE_PREFIX}${JSON.stringify(identity)}`;
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
const markdownOf = (body: unknown) => (isTiptapDoc(body) ? tiptapDocToMd(body) : "");
/** The native body a stored draft, or the draft "edit current" set aside, holds. */
function storedBody(saved: Pick<StoredDraft, "start" | "mine">, targetId: string): unknown {
  const doc = loadBody(saved.start, targetId);
  try {
    Y.applyUpdate(doc, decodeUpdate(saved.mine));
    if (doc.store.pendingStructs || doc.store.pendingDs)
      throw new Error("Incomplete draft history");
    return yDocToTiptapJson(doc);
  } finally {
    doc.destroy();
  }
}
function backupBody(backup: StoredDraft | null | undefined, targetId: string): unknown {
  try {
    return backup ? storedBody(backup, targetId) : null;
  } catch {
    // An unreadable set-aside draft is left out; the working draft still exports.
    return null;
  }
}
function retainedExport(
  owner: OffWikiOwner,
  mine: unknown,
  sourceText: string | null,
  backup: unknown,
): RetainedOffDraft {
  // Unapplied Markdown is newer than the native body it was edited from. The
  // private draft that "edit current" set aside follows the working one.
  const parts = [sourceText ?? markdownOf(mine)];
  if (backup !== null) parts.push(markdownOf(backup));
  const markdown = parts.join("\n\n---\n\n");
  const line = markdown.split("\n").find((value) => value.trim()) ?? "";
  return { owner, markdown, excerpt: line.trim().slice(0, 80) };
}
function parseRetained(raw: string | null, key: string): RetainedOffDraft | null {
  if (!raw || raw.length > 40 * 1024 * 1024) return null;
  const saved = JSON.parse(raw) as Partial<Omit<StoredDraft, "format">> & { format: unknown };
  const source = saved.sourceBuffer;
  // Export only what the restoring constructor would accept for these fields.
  if (
    saved.format !== 1 ||
    !saved.owner ||
    !saved.start ||
    typeof saved.mine !== "string" ||
    typeof saved.acknowledged !== "string" ||
    ownerKey(saved.owner) !== key ||
    (source &&
      (typeof source.text !== "string" ||
        source.text.length > 2 * 1024 * 1024 ||
        typeof source.baseV1 !== "string")) ||
    (saved.conflictBackup && ownerKey(saved.conflictBackup.owner) !== key)
  )
    return null;
  const base = loadBody(saved.start, saved.owner.targetId);
  try {
    if (saved.acknowledged !== encodeUpdate(Y.encodeStateAsUpdate(base))) return null;
  } finally {
    base.destroy();
  }
  return retainedExport(
    saved.owner,
    storedBody({ start: saved.start, mine: saved.mine }, saved.owner.targetId),
    source ? source.text : null,
    backupBody(saved.conflictBackup, saved.owner.targetId),
  );
}
/** The body without editor mount normalization: block IDs and the empty
 * document's paragraph are written on mount, not typed by the user. Both
 * sides are Y projections so attr spelling cannot differ. */
function visibleBody(content: unknown): string {
  const strip = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(strip);
    if (typeof value !== "object" || value === null) return value;
    const entries: [string, unknown][] = Object.entries(value).sort(([a], [b]) =>
      a < b ? -1 : a > b ? 1 : 0,
    );
    const out: Record<string, unknown> = {};
    for (const [key, item] of entries) {
      if (key !== "attrs" || typeof item !== "object" || item === null) out[key] = strip(item);
      else {
        const attrs = Object.entries(item).filter(([name]) => name !== "id");
        if (attrs.length) out.attrs = strip(Object.fromEntries(attrs));
      }
    }
    return out;
  };
  const body = strip(content) as { content?: unknown };
  const blocks = Array.isArray(body.content) ? body.content : [];
  const empty =
    blocks.length === 0 ||
    (blocks.length === 1 && JSON.stringify(blocks[0]) === '{"type":"paragraph"}');
  return empty ? "" : JSON.stringify(body);
}
/** The exact slot of one owner, as the storage key scopes it. */
export function readRetainedDraft(
  storage: Pick<Storage, "getItem"> | null,
  owner: OffWikiOwner,
): RetainedOffDraft | null {
  try {
    return storage ? parseRetained(storage.getItem(ownerKey(owner)), ownerKey(owner)) : null;
  } catch {
    return null;
  }
}
/** This tab's retained slots of one actor and credential only. */
export function listRetainedDrafts(
  storage: Pick<Storage, "length" | "key" | "getItem"> | null,
  actorId: string,
  credentialId: string,
): RetainedOffDraft[] {
  const drafts: RetainedOffDraft[] = [];
  // The key spells the actor and credential first: other slots stay unparsed.
  const scope = `${STORAGE_PREFIX}${JSON.stringify([actorId, credentialId]).slice(0, -1)},`;
  try {
    for (let index = 0; storage && index < storage.length; index++) {
      const key = storage.key(index);
      if (!key?.startsWith(scope)) continue;
      let draft: RetainedOffDraft | null = null;
      try {
        draft = parseRetained(storage.getItem(key), key);
      } catch {
        /* A malformed slot is not exportable; it stays untouched. */
      }
      if (draft?.owner.actorId === actorId && draft.owner.credentialId === credentialId)
        drafts.push(draft);
    }
  } catch {
    /* Unavailable storage lists nothing. */
  }
  return drafts;
}
/** The retained drafts of one workspace a page can no longer reach: the task
 * whose own read was refused, any target of a project the fresh list no longer
 * grants, or the target last opened at the address that now names nothing
 * (trashed, deleted or moved). Without the list only the other two qualify. */
export function unreachableDrafts(
  drafts: readonly RetainedOffDraft[],
  workspaceId: string,
  grantedProjects: ReadonlySet<string> | null,
  refusedTaskId: string | null,
  missingDisplayId: string | null = null,
): RetainedOffDraft[] {
  const missing = missingDisplayId?.trim().toUpperCase();
  return drafts.filter(
    ({ owner }) =>
      owner.workspaceId === workspaceId &&
      ((owner.kind === "task" && owner.targetId === refusedTaskId) ||
        (!!grantedProjects && !!owner.projectId && !grantedProjects.has(owner.projectId)) ||
        (!!missing &&
          typeof owner.displayId === "string" &&
          owner.displayId.toUpperCase() === missing)),
  );
}
/** A target moved to another project component of its workspace (wiki
 * included) keeps its id: its draft follows it. Only this actor's and
 * credential's slots of the same workspace, target and kind are read, by key
 * prefix; no other slot is parsed. The restoring constructor checks the slot
 * for the new owner first; one it refuses stays where it was. The old slot is
 * removed only after the storage took the new key without refusing it. A
 * refused write is taken back even when a fallback kept it, so the old slot
 * stays the one copy: it goes to `unmoved` for export only and moves on a
 * later open with room. The slot moves; it is never discarded. */
export function adoptMovedDraft(
  storage: Pick<Storage, "length" | "key" | "getItem" | "setItem" | "removeItem"> | null,
  owner: OffWikiOwner,
  current: VersionedBody,
  unmoved: (draft: RetainedOffDraft) => void = () => {},
): boolean {
  const key = ownerKey(owner);
  if (!storage || storage.getItem(key) !== null) return false;
  const target = `${STORAGE_PREFIX}${JSON.stringify([
    owner.actorId,
    owner.credentialId,
    owner.workspaceId,
    owner.targetId,
  ]).slice(0, -1)}`;
  const candidates: string[] = [];
  for (let index = 0; index < storage.length; index++) {
    const candidate = storage.key(index);
    if (
      candidate &&
      candidate !== key &&
      [`${target}]`, `${target},`].some((at) => candidate.startsWith(at))
    )
      candidates.push(candidate);
  }
  for (const from of candidates) {
    let saved: Partial<StoredDraft> | null;
    try {
      saved = JSON.parse(storage.getItem(from) ?? "null") as Partial<StoredDraft> | null;
    } catch {
      continue;
    }
    // A pending save or copy stays bound to the address it was sent to; its old
    // address still offers it for export.
    if (
      !saved?.owner ||
      ownerKey(saved.owner) !== from ||
      (saved.owner.kind ?? "document") !== (owner.kind ?? "document") ||
      saved.frozen ||
      saved.distinct
    )
      continue;
    const moved = JSON.stringify({
      ...saved,
      owner,
      ...(saved.conflictBackup ? { conflictBackup: { ...saved.conflictBackup, owner } } : {}),
    });
    const trial = new OffWikiDraft(
      owner,
      current,
      { getItem: (at) => (at === key ? moved : null), setItem() {}, removeItem() {} },
      () => {},
    );
    const taken = trial.storageError === null && trial.hasPrivateState;
    trial.retire();
    if (!taken) continue;
    try {
      storage.setItem(key, moved);
    } catch {
      // A fallback that kept the refused write (page memory) is not the tab
      // storage. The new key was empty, so this removes only that copy.
      try {
        storage.removeItem(key);
      } catch {
        /* Nothing was stored there. */
      }
      try {
        const left = parseRetained(storage.getItem(from), from);
        if (left) unmoved(left);
      } catch {
        /* Not exportable; the old slot still stays where it was. */
      }
      return false;
    }
    if (storage.getItem(key) !== moved) return false;
    try {
      storage.removeItem(from);
    } catch {
      /* Left where it was: kept, not lost. */
    }
    return true;
  }
  return false;
}
export function encodeUpdate(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}
export function decodeUpdate(encoded: string): Uint8Array {
  if (encoded.length > Math.ceil((8 * 1024 * 1024) / 3) * 4)
    throw new Error("Body update exceeds limit");
  const binary = atob(encoded);
  return Uint8Array.from(binary, (byte) => byte.charCodeAt(0));
}
function tail(value: string): bigint {
  if (!/^(0|[1-9][0-9]*)$/.test(value)) throw new Error("Invalid body version");
  const parsed = BigInt(value);
  if (parsed > 9223372036854775807n) throw new Error("Invalid body version");
  return parsed;
}
/** True when `server` contains `liveUpdate`.
 * A yrs Load→Apply→Snapshot keeps the history and the delete set, not the
 * update bytes. Both checks are required: the server state vector must cover
 * every struct in the live update, and the live delete set must be included in
 * the server delete set. Covering the state vector alone would accept a
 * deletion that was never saved. */
export function serverContainsLive(server: Y.Doc, liveUpdate: Uint8Array): boolean {
  if (server.store.pendingStructs || server.store.pendingDs) return false;
  return Y.snapshotContainsUpdate(Y.snapshot(server), liveUpdate);
}
export function loadBody(source: VersionedBody, targetId: string): Y.Doc {
  if (source.targetId !== targetId) throw new Error("Body target mismatch");
  tail(source.tailSeq);
  if (source.tailV1.length > 64) throw new Error("Body history exceeds limit");
  const doc = new Y.Doc({ gc: false });
  try {
    if (source.snapshotV1) Y.applyUpdate(doc, decodeUpdate(source.snapshotV1));
    for (const update of source.tailV1) Y.applyUpdate(doc, decodeUpdate(update));
    if (doc.store.pendingStructs || doc.store.pendingDs) throw new Error("Incomplete body history");
    return doc;
  } catch (error) {
    doc.destroy();
    throw error;
  }
}

/** One mounted OFF wiki owner. Only acknowledged snapshots become the next
 * save base. A pending command freezes exact bytes even when editing continues.
 * No server projection is ever reseeded into an existing native document. */
export class OffWikiDraft {
  doc: Y.Doc;
  start: VersionedBody;
  latest: VersionedBody | null = null;
  comparison: { start: unknown; mine: unknown; current: unknown } | null = null;
  private conflictBackup: StoredDraft | null = null;
  sourceBuffer: { text: string; baseV1: string } | null = null;
  frozen: FrozenSave | null = null;
  distinct: FrozenDistinct | null = null;
  private acknowledged: string;
  private retired = false;
  /** A restored slot the user changed: it is offered back, never hidden. */
  restored = false;
  saving = false;
  creating = false;
  storageError: unknown = null;
  constructor(
    readonly owner: OffWikiOwner,
    current: VersionedBody,
    private storage: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null,
    private changed: () => void,
    private commandId: () => string = () => crypto.randomUUID(),
  ) {
    this.start = current;
    this.doc = loadBody(current, owner.targetId);
    this.acknowledged = encodeUpdate(Y.encodeStateAsUpdate(this.doc));
    // Restore only after a fresh authorized read. Stale versions become an
    // explicit conflict; the server body never overwrites the owned draft.
    try {
      const raw = storage?.getItem(ownerKey(owner));
      if (raw && raw.length <= 40 * 1024 * 1024) {
        // Persisted JSON has not yet proved the version or optional command shapes.
        const saved = JSON.parse(raw) as Omit<StoredDraft, "format"> & { format: unknown };
        if (saved.format !== 1 || ownerKey(saved.owner) !== ownerKey(owner))
          throw new Error("Draft owner mismatch");
        if (typeof saved.mine !== "string" || typeof saved.acknowledged !== "string")
          throw new Error("Invalid stored draft");
        const old = loadBody(saved.start, owner.targetId);
        try {
          if (saved.acknowledged !== encodeUpdate(Y.encodeStateAsUpdate(old)))
            throw new Error("Invalid draft base");
          const startBody = yDocToTiptapJson(old);
          Y.applyUpdate(old, decodeUpdate(saved.mine));
          if (old.store.pendingStructs || old.store.pendingDs)
            throw new Error("Incomplete draft history");
          if (saved.frozen) {
            const command = saved.frozen.command as BodySaveCommand | undefined;
            if (
              typeof command?.commandId !== "string" ||
              !/^[0-9a-f-]{36}$/i.test(command.commandId) ||
              command.expectedTailSeq !== saved.start.tailSeq ||
              typeof command.updateV1 !== "string" ||
              typeof saved.frozen.snapshot !== "string"
            )
              throw new Error("Invalid pending draft command");
            const frozenDoc = loadBody(saved.start, owner.targetId);
            try {
              Y.applyUpdate(frozenDoc, decodeUpdate(command.updateV1));
              if (
                frozenDoc.store.pendingStructs ||
                frozenDoc.store.pendingDs ||
                encodeUpdate(Y.encodeStateAsUpdate(frozenDoc)) !== saved.frozen.snapshot
              )
                throw new Error("Pending draft command differs from its snapshot");
            } finally {
              frozenDoc.destroy();
            }
          }
          if (saved.sourceBuffer) {
            if (
              typeof saved.sourceBuffer.text !== "string" ||
              saved.sourceBuffer.text.length > 2 * 1024 * 1024 ||
              typeof saved.sourceBuffer.baseV1 !== "string"
            )
              throw new Error("Invalid Markdown draft");
            decodeUpdate(saved.sourceBuffer.baseV1);
          }
          if (saved.conflictBackup && ownerKey(saved.conflictBackup.owner) !== ownerKey(owner))
            throw new Error("Conflict draft owner mismatch");
          if (saved.distinct) {
            const request = saved.distinct.body as OffDraftCreateBody | undefined;
            if (
              !request ||
              !liveUuid(request.commandId) ||
              request.sourceId !== owner.targetId ||
              request.sourceKind !== (owner.kind ?? "document") ||
              (request.sourceProjectId ?? null) !==
                (owner.kind === "task" ? null : (owner.projectId ?? null)) ||
              !isTiptapDoc(request.contentJson) ||
              new TextEncoder().encode(JSON.stringify(request.contentJson)).length > 1024 * 1024 ||
              typeof request.title !== "string" ||
              (request.parentId !== null &&
                !z.string().uuid().safeParse(request.parentId).success) ||
              (saved.distinct.projectId !== null &&
                !z.string().uuid().safeParse(saved.distinct.projectId).success)
            )
              throw new Error("Distinct draft command owner/input mismatch");
          }
          // Replace the just-created reader, never an already edited document.
          this.doc.destroy();
          this.doc = old;
          this.start = { ...saved.start, writable: current.writable };
          this.acknowledged = saved.acknowledged;
          this.frozen = saved.frozen;
          this.distinct = saved.distinct ?? null;
          this.conflictBackup = saved.conflictBackup ?? null;
          this.comparison = saved.comparison ?? null;
          this.sourceBuffer = saved.sourceBuffer ?? null;
          // Every slot is restored; only one the user changed is offered back.
          this.restored =
            !!saved.frozen ||
            !!saved.distinct ||
            !!saved.conflictBackup ||
            !!saved.comparison ||
            !!saved.sourceBuffer ||
            saved.start.tailSeq !== current.tailSeq ||
            visibleBody(startBody) !== visibleBody(yDocToTiptapJson(old));
          if (saved.start.tailSeq !== current.tailSeq) {
            this.latest = current;
            this.comparison = {
              start: saved.start.contentJson,
              mine: yDocToTiptapJson(old),
              current: current.contentJson,
            };
          }
        } catch (error) {
          old.destroy();
          throw error;
        }
      }
    } catch (error) {
      this.storageError = error;
    }
    this.doc.on("update", this.onUpdate);
  }
  private onUpdate = () => {
    this.persist();
    this.changed();
  };
  get dirty(): boolean {
    return encodeUpdate(Y.encodeStateAsUpdate(this.doc)) !== this.acknowledged;
  }
  get mine() {
    return yDocToTiptapJson(this.doc);
  }
  get durable(): boolean {
    return !this.dirty && !this.frozen && !this.distinct && !this.latest && !this.sourceBuffer;
  }
  get active(): boolean {
    return !this.retired;
  }
  get hasPrivateState(): boolean {
    return (
      this.dirty ||
      !!this.frozen ||
      !!this.distinct ||
      !!this.latest ||
      !!this.conflictBackup ||
      !!this.sourceBuffer
    );
  }
  /** A confirmed operation may be followed by a fresh authorized read while
   * typing continues. Keep the live native draft even when storage refused it.
   * This read is not a receipt for any other unknown pending command. */
  observeAuthorizedBody(current: VersionedBody): void {
    if (this.retired || current.targetId !== this.owner.targetId) return;
    const reader = loadBody(current, this.owner.targetId);
    let contained: boolean;
    try {
      contained = serverContainsLive(reader, decodeUpdate(this.acknowledged));
    } finally {
      reader.destroy();
    }
    this.start = { ...this.start, writable: current.writable };
    if (current.tailSeq !== this.start.tailSeq || !contained) {
      this.latest = current;
      this.comparison = {
        start: this.start.contentJson,
        mine: this.mine,
        current: current.contentJson,
      };
    }
    this.persist();
    this.changed();
  }
  persist(): void {
    if (this.retired || !this.storage) return;
    try {
      if (
        !this.dirty &&
        !this.frozen &&
        !this.distinct &&
        !this.latest &&
        !this.conflictBackup &&
        !this.sourceBuffer
      )
        this.storage.removeItem(ownerKey(this.owner));
      else
        this.storage.setItem(
          ownerKey(this.owner),
          JSON.stringify({
            format: 1,
            owner: this.owner,
            start: this.start,
            mine: encodeUpdate(Y.encodeStateAsUpdate(this.doc)),
            acknowledged: this.acknowledged,
            frozen: this.frozen,
            distinct: this.distinct,
            conflictBackup: this.conflictBackup,
            comparison: this.comparison,
            sourceBuffer: this.sourceBuffer,
          } satisfies StoredDraft),
        );
      this.storageError = null;
    } catch (error) {
      this.storageError = error;
    }
  }
  async save(send: (command: BodySaveCommand) => Promise<BodySaveResult>): Promise<boolean> {
    if (this.retired || this.saving || this.creating || (this.latest && !this.frozen)) return false;
    if (!this.start.writable) return false;
    if (!this.dirty && !this.frozen) return this.durable;
    if (!this.frozen) {
      if (tail(this.start.tailSeq) === 9223372036854775807n)
        throw new Error("Body version exhausted");
      const base = loadBody(this.start, this.owner.targetId);
      try {
        this.frozen = {
          command: {
            commandId: this.commandId(),
            expectedTailSeq: this.start.tailSeq,
            updateV1: encodeUpdate(Y.encodeStateAsUpdate(this.doc, Y.encodeStateVector(base))),
          },
          snapshot: encodeUpdate(Y.encodeStateAsUpdate(this.doc)),
        };
      } finally {
        base.destroy();
      }
      this.persist();
    }
    const pending = this.frozen;
    this.saving = true;
    this.changed();
    try {
      const result = await send(pending.command);
      if (!this.active) return false;
      if (
        result.commandId !== pending.command.commandId ||
        result.targetId !== this.owner.targetId ||
        tail(result.tailSeq) !== tail(pending.command.expectedTailSeq) + 1n ||
        !result.revisionId
      ) {
        throw new Error("Unmatched body save acknowledgement");
      }
      // Matching command, target, tail and revision still gate this path.
      // The receipt settles that command. It does not replace a newer
      // authorized head, a definite in-flight conflict, or the live draft.
      const newerHead = this.latest !== null && tail(this.latest.tailSeq) > tail(result.tailSeq);
      if (this.frozen !== pending || newerHead) {
        if (this.frozen === pending) this.frozen = null;
        if (this.latest) {
          this.comparison = {
            start: this.start.contentJson,
            mine: this.mine,
            current: this.latest.contentJson,
          };
        }
        this.persist();
        return false;
      }
      const acknowledgedDoc = this.snapshotDoc(pending.snapshot);
      let contentJson;
      try {
        contentJson = yDocToTiptapJson(acknowledgedDoc);
      } finally {
        acknowledgedDoc.destroy();
      }
      this.start = {
        targetId: this.owner.targetId,
        tailSeq: result.tailSeq,
        snapshotV1: pending.snapshot,
        tailV1: [],
        contentJson,
        writable: this.start.writable,
      };
      this.acknowledged = pending.snapshot;
      this.frozen = null;
      this.latest = null;
      // The restored draft is now the acknowledged base, no longer an offer.
      this.restored = false;
      this.persist();
      return this.durable;
    } finally {
      this.saving = false;
      if (this.active) this.changed();
    }
  }
  async createDistinct(
    destination: DraftDestination,
    send: (body: OffDraftCreateBody, projectId: string | null) => Promise<OffDraftCreateResponse>,
  ): Promise<OffDraftCreateResponse | null> {
    if (this.retired || this.saving || this.creating || (this.sourceBuffer && !this.distinct))
      return null;
    if (!this.distinct) {
      const title = destination.title.trim();
      if (
        !title ||
        title.length > 300 ||
        (destination.icon != null && destination.icon.length > 50)
      )
        throw new Error("Invalid document title or icon");
      this.distinct = {
        projectId: destination.projectId,
        body: {
          commandId: this.commandId(),
          sourceKind: this.owner.kind ?? "document",
          sourceId: this.owner.targetId,
          sourceProjectId: this.owner.kind === "task" ? null : (this.owner.projectId ?? null),
          parentId: destination.parentId,
          title,
          icon: destination.icon,
          contentJson: independentDraftBody(this.mine, createFvociExtensions()),
        },
      };
      this.persist();
    }
    const pending = this.distinct;
    this.creating = true;
    this.changed();
    try {
      const result = await send(pending.body, pending.projectId);
      if (!this.active) return null;
      const document = result.document as OffDraftCreateResponse["document"] | undefined;
      if (
        result.commandId !== pending.body.commandId ||
        result.tailSeq !== "1" ||
        !document ||
        !liveUuid(document.id) ||
        document.id === this.owner.targetId ||
        document.workspaceId !== this.owner.workspaceId ||
        (document.projectId ?? null) !== pending.projectId ||
        document.parentId !== pending.body.parentId ||
        !liveUuid(result.revisionId)
      )
        throw new Error("Unmatched distinct document acknowledgment");
      // Copying mine never replaces, advances or clears the original draft.
      this.distinct = null;
      this.persist();
      return result;
    } finally {
      this.creating = false;
      if (this.active) this.changed();
    }
  }
  distinctRefused(commandId: string): void {
    if (this.retired || this.distinct?.body.commandId !== commandId) return;
    // Only a definite validated-input refusal supplied by the controller.
    // Mine/history remain private; the next deliberate submit is a new intent.
    this.distinct = null;
    this.persist();
    this.changed();
  }
  private snapshotDoc(snapshot: string): Y.Doc {
    const doc = new Y.Doc({ gc: false });
    Y.applyUpdate(doc, decodeUpdate(snapshot));
    return doc;
  }
  conflict(current: VersionedBody): void {
    if (this.retired || current.targetId !== this.owner.targetId) return;
    // Validate the actual native history, but do not apply it to mine.
    loadBody(current, this.owner.targetId).destroy();
    this.latest = current;
    this.comparison = {
      start: this.start.contentJson,
      mine: this.mine,
      current: current.contentJson,
    };
    // A 409 is a definite refusal, so this command cannot later become an ACK.
    this.frozen = null;
    this.persist();
    this.changed();
  }
  editCurrent(): void {
    if (this.retired || !this.latest || this.saving || this.creating) return;
    const fresh = loadBody(this.latest, this.owner.targetId);
    this.conflictBackup = {
      format: 1,
      owner: this.owner,
      start: this.start,
      mine: encodeUpdate(Y.encodeStateAsUpdate(this.doc)),
      acknowledged: this.acknowledged,
      frozen: this.frozen,
    };
    this.doc.off("update", this.onUpdate);
    this.doc.destroy();
    this.doc = fresh;
    this.start = this.latest;
    this.latest = null;
    this.frozen = null;
    this.acknowledged = encodeUpdate(Y.encodeStateAsUpdate(fresh));
    fresh.on("update", this.onUpdate);
    this.persist();
    this.changed();
  }
  setSourceBuffer(buffer: { text: string; baseV1: Uint8Array } | null): void {
    if (this.retired) return;
    this.sourceBuffer = buffer ? { text: buffer.text, baseV1: encodeUpdate(buffer.baseV1) } : null;
    this.persist();
    this.changed();
  }
  /** The live private state as a local export, before access loss retires it. */
  exportDraft(): RetainedOffDraft {
    return retainedExport(
      this.owner,
      this.mine,
      this.sourceBuffer?.text ?? null,
      backupBody(this.conflictBackup, this.owner.targetId),
    );
  }
  /** Only an explicit user choice discards private state, never an in-flight
   * request's. The slot goes before retirement so nothing rewrites it. */
  discard(): boolean {
    if (this.retired || this.saving || this.creating) return false;
    try {
      this.storage?.removeItem(ownerKey(this.owner));
    } catch (error) {
      this.storageError = error;
      this.changed();
      return false;
    }
    this.retired = true;
    this.doc.off("update", this.onUpdate);
    this.doc.destroy();
    return true;
  }
  retire(): void {
    this.persist();
    this.retired = true;
    this.doc.off("update", this.onUpdate);
    this.doc.destroy();
  }
}
