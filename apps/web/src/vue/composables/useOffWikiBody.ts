import { t } from "@fvoci/i18n";
import { computed, inject, markRaw, onScopeDispose, ref, shallowRef, watch } from "vue";
import * as Y from "yjs";
import {
  OffWikiDraft,
  adoptMovedDraft,
  decodeUpdate,
  listRetainedDrafts,
  loadBody,
  ownerKey,
  readRetainedDraft,
  serverContainsLive,
  type OffWikiOwner,
  type DraftDestination,
  type RetainedOffDraft,
} from "@/features/documents/off-wiki-draft";
import { createDocumentFromDraft } from "@/features/documents/document-api";
import { readVersionedBody, saveVersionedBody } from "@/features/documents/versioned-body-api";
import { ProblemError } from "@/lib/api";
import { shareDownloadName, triggerDownload } from "@/lib/share";
import { copyText } from "../features/settings/clipboard";
import { sourceDraftAuthRetiredKey } from "./useSourceDraftGuard";

// Slots the tab storage refused (quota, blocked), kept in this page's memory
// under the same key: a denial, a retry, regained access or a remount in this
// page still restores them. Leaving, closing or reloading the page loses them.
const unstoredSlots = new Map<string, string>();
type TabSlots = Pick<Storage, "length" | "key" | "getItem" | "setItem" | "removeItem">;
/** This tab's draft slots: sessionStorage, then this page's memory for writes it refused. */
function tabStorage(): TabSlots {
  let storage: Storage | null;
  try {
    storage = window.sessionStorage;
  } catch {
    storage = null;
  }
  const keys = () => {
    const all = new Set(unstoredSlots.keys());
    for (let index = 0; storage && index < storage.length; index++) {
      const key = storage.key(index);
      if (key !== null) all.add(key);
    }
    return [...all];
  };
  return {
    get length() {
      return keys().length;
    },
    key: (index) => keys()[index] ?? null,
    // Memory is written only by a refused write, so it is never the older copy.
    getItem: (key) => unstoredSlots.get(key) ?? storage?.getItem(key) ?? null,
    setItem(key, value) {
      try {
        if (!storage) throw new Error("Tab storage unavailable");
        storage.setItem(key, value);
        unstoredSlots.delete(key);
      } catch (error) {
        // The draft still reports the failure, so its warning stays.
        unstoredSlots.set(key, value);
        throw error;
      }
    },
    removeItem(key) {
      unstoredSlots.delete(key);
      storage?.removeItem(key);
    },
  };
}
/** The draft as this page holds it, marked when only its memory does. */
const held = (draft: RetainedOffDraft | null): RetainedOffDraft | null =>
  draft && unstoredSlots.has(ownerKey(draft.owner)) ? { ...draft, unstored: true } : draft;
const denied = (failure: unknown) =>
  failure instanceof ProblemError && [401, 403, 404].includes(failure.status);

/** This tab's retained drafts of one actor and credential, read locally only. */
export function listRetainedOffDrafts(actorId: string, credentialId: string): RetainedOffDraft[] {
  return listRetainedDrafts(tabStorage(), actorId, credentialId).map(
    (draft) => held(draft) ?? draft,
  );
}

/** Local copy/download of a retained draft; neither path reaches the server. */
export function useRetainedDraftExport() {
  const note = ref<string | null>(null);
  async function run(draft: RetainedOffDraft | null, how: "copy" | "download", title?: string) {
    note.value = null;
    if (!draft) return;
    try {
      if (how === "copy") {
        await copyText(draft.markdown);
        note.value = t("doc.off.copyDraftDone");
      } else
        triggerDownload(
          shareDownloadName(title?.trim() || draft.excerpt || t("doc.off.mine"), "md"),
          new Blob([draft.markdown], { type: "text/markdown;charset=utf-8" }),
        );
    } catch {
      note.value = t("doc.off.copyDraftFailed");
    }
  }
  return { note, run };
}

/** An authorized wiki page owns one local native history document. Identity
 * changes retire both its pending HTTP callbacks and its editor, including ABA.
 * SSE notifications remain owned by the workspace shell. */
export function useOffWikiBody(owner: () => OffWikiOwner | null, enabled: () => boolean) {
  const authRetired = inject(
    sourceDraftAuthRetiredKey,
    computed(() => false),
  );
  // Read the current injected authority after awaits; never retain its narrowed value.
  const isAuthRetired = () => authRetired.value;
  const draft = shallowRef<OffWikiDraft | null>(null);
  const retained = shallowRef<RetainedOffDraft | null>(null);
  // The private draft of an owner the view dropped, for retainedFor only.
  const lost = shallowRef<RetainedOffDraft | null>(null);
  // The draft this target had before a move, left at its old key because the
  // tab storage refused the new one: export only until an open with room.
  const unmoved = shallowRef<RetainedOffDraft | null>(null);
  const error = shallowRef<unknown>(null);
  const loading = ref(false);
  const revision = ref(0);
  const generation = ref(0);
  let lifetime = 0;
  let abort: AbortController | null = null;
  const changed = () => {
    revision.value++;
  };
  function retire() {
    lifetime++;
    abort?.abort();
    abort = null;
    draft.value?.retire();
    draft.value = null;
    retained.value = null;
    unmoved.value = null;
    loading.value = false;
    error.value = null;
    generation.value++;
  }
  // Only private state is a draft; a clean body is the server's, not an export.
  function exportOf(current: OffWikiDraft | null): RetainedOffDraft | null {
    if (!current?.active || !current.hasPrivateState) return null;
    try {
      return current.exportDraft();
    } catch {
      // Never block retirement; the tab slot still holds the draft.
      return null;
    }
  }
  /** Lost access keeps the owned draft in its tab slot and switches it to
   * export only; nothing sends it again before a fresh authorized read. */
  function deny(current: OffWikiDraft) {
    const kept = exportOf(current);
    current.retire();
    draft.value = null;
    retained.value = held(kept ?? readRetainedDraft(tabStorage(), current.owner));
    generation.value++;
  }
  async function load() {
    const scope = owner();
    if (!scope || !enabled() || isAuthRetired()) return;
    const started = ++lifetime;
    abort?.abort();
    abort = new AbortController();
    loading.value = true;
    error.value = null;
    try {
      const current = await readVersionedBody(
        scope.workspaceId,
        scope.targetId,
        abort.signal,
        scope.projectId,
        scope.kind,
      );
      const currentOwner = owner();
      if (
        started !== lifetime ||
        !enabled() ||
        !currentOwner ||
        ownerKey(currentOwner) !== ownerKey(scope) ||
        isAuthRetired()
      )
        return;
      // Storage that refuses a write leaves that slot in this page's memory.
      const storage = tabStorage();
      retained.value = null;
      const mounted = draft.value;
      if (
        mounted?.active &&
        ownerKey(mounted.owner) === ownerKey(scope) &&
        mounted.hasPrivateState
      ) {
        // Preserve the actual owned in-memory draft, including edits made after
        // the read started. Reconstructing from storage can lose a quota-denied
        // edit or an unavailable-storage source buffer/unknown command.
        mounted.observeAuthorizedBody(current);
      } else {
        mounted?.retire();
        // A target moved to another project component brings its draft along
        // once the tab storage takes it under the new key.
        unmoved.value = null;
        adoptMovedDraft(storage, scope, current, (left) => (unmoved.value = held(left)));
        const next = new OffWikiDraft(scope, current, storage, changed);
        // A slot only memory holds is offered back like any other. Writing it
        // again lets the storage take it now, or keeps its warning.
        if (unstoredSlots.has(ownerKey(scope))) next.persist();
        draft.value = markRaw(next);
        generation.value++;
      }
    } catch (failure) {
      if (started === lifetime) {
        if (denied(failure)) {
          const mounted = draft.value;
          const kept =
            mounted && ownerKey(mounted.owner) === ownerKey(scope) ? exportOf(mounted) : null;
          retire();
          retained.value = held(kept ?? readRetainedDraft(tabStorage(), scope));
        }
        error.value = failure;
      }
    } finally {
      if (started === lifetime) loading.value = false;
    }
  }
  watch(
    () => {
      const currentOwner = owner();
      return JSON.stringify([
        enabled(),
        isAuthRetired(),
        currentOwner ? ownerKey(currentOwner) : null,
      ]);
    },
    () => {
      // An owner the view dropped (its document was refused) keeps this page's
      // private draft for export even when storage refused it; others never do.
      const kept = owner() || isAuthRetired() ? null : (exportOf(draft.value) ?? retained.value);
      retire();
      lost.value = held(kept);
      return load();
    },
    { immediate: true, flush: "sync" },
  );
  onScopeDispose(retire);
  function beforeUnload(event: Event) {
    const current = draft.value;
    if (
      (current?.storageError && (current.dirty || current.sourceBuffer || current.distinct)) ||
      retained.value?.unstored ||
      lost.value?.unstored ||
      unmoved.value?.unstored
    )
      event.preventDefault();
  }
  if (typeof window !== "undefined") window.addEventListener("beforeunload", beforeUnload);
  onScopeDispose(() => {
    if (typeof window !== "undefined") window.removeEventListener("beforeunload", beforeUnload);
  });
  const value = <T, F>(read: (current: OffWikiDraft) => T, fallback: F) =>
    computed(() => {
      const state = { draft: draft.value, revision: revision.value };
      return state.draft ? read(state.draft) : fallback;
    });
  async function save(): Promise<boolean> {
    const current = draft.value;
    if (!current || isAuthRetired() || !enabled()) return false;
    const isActive = () => current.active;
    error.value = null;
    try {
      return await current.save((command) =>
        saveVersionedBody(
          current.owner.workspaceId,
          current.owner.targetId,
          command,
          current.owner.projectId,
          current.owner.kind,
        ),
      );
    } catch (failure) {
      if (draft.value !== current || !isActive()) return false;
      error.value = failure;
      if (denied(failure)) deny(current);
      else if (failure instanceof ProblemError && failure.status === 409) {
        // This is a fresh authorized read, not an oracle for an uncertain commit.
        // A confirmed 409 has already rolled back the rejected writer.
        const started = lifetime;
        try {
          const latest = await readVersionedBody(
            current.owner.workspaceId,
            current.owner.targetId,
            undefined,
            current.owner.projectId,
            current.owner.kind,
          );
          if (started === lifetime && draft.value === current && isActive())
            current.conflict(latest);
        } catch (readFailure) {
          if (started === lifetime) {
            if (denied(readFailure)) deny(current);
            error.value = readFailure;
          }
        }
      }
      return false;
    }
  }
  async function createDistinct(destination: DraftDestination) {
    const current = draft.value;
    const started = lifetime;
    if (!current || isAuthRetired() || !enabled()) return null;
    error.value = null;
    try {
      const result = await current.createDistinct(destination, (body, projectId) =>
        createDocumentFromDraft(current.owner.workspaceId, projectId, body, abort?.signal),
      );
      if (started !== lifetime || draft.value !== current || !current.active || isAuthRetired())
        return null;
      return result;
    } catch (failure) {
      if (started !== lifetime || draft.value !== current || !current.active) return null;
      error.value = failure;
      if (
        failure instanceof ProblemError &&
        ((failure.status === 400 &&
          [
            "invalid_input",
            "invalid_document_body",
            "tree_depth_limit",
            "document_affiliation_mismatch",
          ].includes(failure.code ?? "")) ||
          (failure.status === 413 &&
            failure.code === "document_body_exceeds_document_max_body_bytes"))
      ) {
        const commandId = current.distinct?.body.commandId;
        if (commandId) current.distinctRefused(commandId);
      }
      if (denied(failure)) deny(current);
      // Unknown finish has no fresh-observer recovery: retry the frozen command.
      return null;
    }
  }
  function editCurrent() {
    draft.value?.editCurrent();
    generation.value++;
    error.value = null;
  }
  /** The user's explicit discard: drop the tab slot, then read the current body. */
  async function discard(): Promise<boolean> {
    const current = draft.value;
    if (!current?.discard()) return false;
    draft.value = null;
    generation.value++;
    error.value = null;
    await load();
    return true;
  }
  /** A refused owner's export-only draft: this page's own copy first, else
   * that owner's slot in the tab or, when storage refused it, in memory. */
  function retainedFor(scope: OffWikiOwner): RetainedOffDraft | null {
    const key = ownerKey(scope);
    for (const kept of [retained.value, lost.value])
      if (kept && ownerKey(kept.owner) === key) return kept;
    return held(readRetainedDraft(tabStorage(), scope));
  }
  async function verifyCommitted(): Promise<boolean> {
    const current = draft.value;
    const started = lifetime;
    if (!current || !current.durable || isAuthRetired()) return false;
    const isDurable = () => current.durable;
    try {
      const fresh = await readVersionedBody(
        current.owner.workspaceId,
        current.owner.targetId,
        undefined,
        current.owner.projectId,
        current.owner.kind,
      );
      if (started !== lifetime || draft.value !== current || !current.active || isAuthRetired())
        return false;
      const reader = loadBody(fresh, current.owner.targetId);
      try {
        return (
          fresh.tailSeq === current.start.tailSeq &&
          isDurable() &&
          serverContainsLive(reader, Y.encodeStateAsUpdate(current.doc))
        );
      } finally {
        reader.destroy();
      }
    } catch (failure) {
      if (started === lifetime) {
        error.value = failure;
        if (denied(failure)) deny(current);
      }
      return false;
    }
  }
  return {
    draft,
    generation,
    error,
    loading,
    load,
    save,
    createDistinct,
    editCurrent,
    discard,
    verifyCommitted,
    retained: computed(() => retained.value),
    unmoved: computed(() => unmoved.value),
    retainedFor,
    restored: value((current) => current.restored && current.hasPrivateState, false),
    restoredComparison: value(
      (current) =>
        current.restored && current.hasPrivateState
          ? { start: current.start.contentJson, mine: current.mine }
          : null,
      null,
    ),
    doc: value((current) => current.doc, null),
    writable: value((current) => current.start.writable, false),
    dirty: value((current) => current.dirty, false),
    durable: value((current) => current.durable, false),
    saving: value((current) => current.saving, false),
    creating: value((current) => current.creating, false),
    pendingSave: value((current) => current.active && !!current.frozen, false),
    pendingDistinct: value((current) => current.distinct, null),
    conflict: value((current) => current.latest, null),
    comparison: value((current) => current.comparison, null),
    storageError: value((current) => current.storageError, null),
    sourceBuffer: value(
      (current) =>
        current.sourceBuffer
          ? { text: current.sourceBuffer.text, baseV1: decodeUpdate(current.sourceBuffer.baseV1) }
          : null,
      null,
    ),
    receiveSourceBuffer: (buffer: { text: string; baseV1: Uint8Array } | null) =>
      draft.value?.setSourceBuffer(buffer),
  };
}
