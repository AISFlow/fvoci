import { computed, inject, markRaw, onScopeDispose, ref, shallowRef, watch } from "vue";
import * as Y from "yjs";
import {
  OffWikiDraft,
  decodeUpdate,
  encodeUpdate,
  loadBody,
  ownerKey,
  type OffWikiOwner,
} from "@/features/documents/off-wiki-draft";
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
    if (!scope || !enabled() || authRetired.value) return;
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
      if (
        started !== lifetime ||
        !enabled() ||
        !owner() ||
        ownerKey(owner()!) !== ownerKey(scope) ||
        authRetired.value
      )
        return;
      let storage: Storage | null = null;
      try {
        storage = window.sessionStorage;
      } catch {
        /* The editor still retains its in-memory draft. */
      }
      const mounted = draft.value;
      if (mounted?.active && ownerKey(mounted.owner) === ownerKey(scope) && mounted.hasPrivateState) {
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
    () => JSON.stringify([enabled(), authRetired.value, owner() ? ownerKey(owner()!) : null]),
    () => {
      retire();
      void load();
    },
    { immediate: true, flush: "sync" },
  );
  onScopeDispose(retire);
  function beforeUnload(event: Event) {
    const current = draft.value;
    if (current?.storageError && (current.dirty || current.sourceBuffer)) event.preventDefault();
  }
  if (typeof window !== "undefined") window.addEventListener("beforeunload", beforeUnload);
  onScopeDispose(() => {
    if (typeof window !== "undefined") window.removeEventListener("beforeunload", beforeUnload);
  });
  const value = <T, F>(read: (current: OffWikiDraft) => T, fallback: F) =>
    computed(() => {
      void revision.value;
      return draft.value ? read(draft.value) : fallback;
    });
  async function save(): Promise<boolean> {
    const current = draft.value;
    if (!current || authRetired.value || !enabled()) return false;
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
      if (draft.value !== current || !current.active) return false;
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
          if (started === lifetime && draft.value === current && current.active)
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
  function editCurrent() {
    draft.value?.editCurrent();
    generation.value++;
    error.value = null;
  }
  async function verifyCommitted(): Promise<boolean> {
    const current = draft.value;
    const started = lifetime;
    if (!current || !current.durable || authRetired.value) return false;
    try {
      const fresh = await readVersionedBody(
        current.owner.workspaceId,
        current.owner.targetId,
        undefined,
        current.owner.projectId,
        current.owner.kind,
      );
      if (started !== lifetime || draft.value !== current || !current.active || authRetired.value)
        return false;
      const reader = loadBody(fresh, current.owner.targetId);
      try {
        return (
          fresh.tailSeq === current.start.tailSeq &&
          current.durable &&
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
    editCurrent,
    verifyCommitted,
    doc: value((current) => current.doc, null),
    writable: value((current) => current.start.writable, false),
    dirty: value((current) => current.dirty, false),
    durable: value((current) => current.durable, false),
    saving: value((current) => current.saving, false),
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
