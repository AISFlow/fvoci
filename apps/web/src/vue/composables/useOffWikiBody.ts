import { computed, inject, markRaw, onScopeDispose, ref, shallowRef, watch } from "vue";
import * as Y from "yjs";
import {
  OffWikiDraft,
  decodeUpdate,
  encodeUpdate,
  loadBody,
  ownerKey,
  type OffWikiOwner,
  type DraftDestination,
} from "@/features/documents/off-wiki-draft";
import { createDocumentFromDraft } from "@/features/documents/document-api";
import { readVersionedBody, saveVersionedBody } from "@/features/documents/versioned-body-api";
import { ProblemError } from "@/lib/api";
import { sourceDraftAuthRetiredKey } from "./useSourceDraftGuard";

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
    loading.value = false;
    error.value = null;
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
      let storage: Storage | null = null;
      try {
        storage = window.sessionStorage;
      } catch {
        /* The editor still retains its in-memory draft. */
      }
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
        draft.value = markRaw(new OffWikiDraft(scope, current, storage, changed));
        generation.value++;
      }
    } catch (failure) {
      if (started === lifetime) {
        if (failure instanceof ProblemError && [401, 403, 404].includes(failure.status)) retire();
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
      retire();
      return load();
    },
    { immediate: true, flush: "sync" },
  );
  onScopeDispose(retire);
  function beforeUnload(event: Event) {
    const current = draft.value;
    if (current?.storageError && (current.dirty || current.sourceBuffer || current.distinct))
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
      if (failure instanceof ProblemError && [401, 403, 404].includes(failure.status)) {
        current.retire();
        draft.value = null;
        generation.value++;
      } else if (failure instanceof ProblemError && failure.status === 409) {
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
            if (
              readFailure instanceof ProblemError &&
              [401, 403, 404].includes(readFailure.status)
            ) {
              current.retire();
              draft.value = null;
              generation.value++;
            }
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
      if (failure instanceof ProblemError && [401, 403, 404].includes(failure.status)) {
        current.retire();
        draft.value = null;
        generation.value++;
      }
      // Unknown finish has no fresh-observer recovery: retry the frozen command.
      return null;
    }
  }
  function editCurrent() {
    draft.value?.editCurrent();
    generation.value++;
    error.value = null;
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
          encodeUpdate(Y.encodeStateAsUpdate(reader)) ===
            encodeUpdate(Y.encodeStateAsUpdate(current.doc))
        );
      } finally {
        reader.destroy();
      }
    } catch (failure) {
      if (started === lifetime) {
        error.value = failure;
        if (failure instanceof ProblemError && [401, 403, 404].includes(failure.status)) {
          current.retire();
          draft.value = null;
          generation.value++;
        }
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
    verifyCommitted,
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
