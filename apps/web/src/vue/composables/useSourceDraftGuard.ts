import type { FvociEditor } from "@fvoci/editor/vue";
import { onScopeDispose, ref, shallowRef, watch } from "vue";

type EditorInstance = InstanceType<typeof FvociEditor>;
export type SourceDraftState = EditorInstance["sourceDraftState"];
interface GuardOptions {
  scope: () => string | number | undefined;
  identity: () => string;
  authorized: () => boolean;
  editor: () => Pick<EditorInstance, "sourceDraftState" | "discardSourceDraft"> | null;
}

/** Protect only the current editor's transient source draft. Auth retirement
 * takes priority over a pending user navigation and never discards a new owner. */
export function useSourceDraftGuard(options: GuardOptions) {
  const open = ref(false);
  const draft = shallowRef<SourceDraftState | null>(null);
  let owner: object | null = null;
  let pending: {
    owner: object;
    scope: SourceDraftState["scope"];
    identity: string;
    promise: Promise<boolean>;
    resolve: (leave: boolean) => void;
  } | null = null;

  function finish(leave: boolean): void {
    const previous = pending;
    pending = null;
    open.value = false;
    previous?.resolve(leave);
  }
  function receive(state: SourceDraftState): void {
    if (!options.authorized() || state.scope !== options.scope()) return;
    if (state.phase === "activate") {
      if (options.editor()?.sourceDraftState.owner !== state.owner) return;
      if (owner !== state.owner) finish(true);
      owner = state.owner;
      draft.value = state;
    } else if (owner === state.owner) {
      if (state.phase === "retire") {
        owner = null;
        draft.value = null;
        finish(true);
      } else draft.value = state;
    }
  }
  function protectedDraft(): boolean {
    const current = draft.value;
    return (
      options.authorized() &&
      !!current &&
      current.owner === owner &&
      current.scope === options.scope() &&
      (current.dirty || current.composing)
    );
  }
  function requestLeave(): Promise<boolean> {
    if (!protectedDraft() || !owner) return Promise.resolve(true);
    if (pending) return pending.promise;
    let resolve: (leave: boolean) => void = () => {
      throw new Error("unbound navigation guard");
    };
    const promise = new Promise<boolean>((done) => {
      resolve = done;
    });
    pending = { owner, scope: options.scope(), identity: options.identity(), promise, resolve };
    open.value = true;
    return promise;
  }
  function keepEditing(): void {
    finish(false);
  }
  function discardAndLeave(): void {
    const instance = options.editor();
    const state = instance?.sourceDraftState;
    if (
      !pending ||
      !options.authorized() ||
      pending.owner !== owner ||
      pending.scope !== options.scope() ||
      pending.identity !== options.identity() ||
      !state ||
      state.owner !== pending.owner ||
      state.scope !== pending.scope ||
      state.composing
    )
      return;
    instance.discardSourceDraft();
    if (instance.sourceDraftState.dirty || instance.sourceDraftState.composing) return;
    draft.value = instance.sourceDraftState;
    finish(true);
  }
  watch(
    options.scope,
    () => {
      // A callback for the old scope cannot authorize navigation in the new one.
      draft.value = null;
      finish(true);
    },
    { flush: "sync" },
  );
  watch(
    [options.identity, options.authorized],
    () => {
      owner = null;
      draft.value = null;
      finish(true);
    },
    { flush: "sync" },
  );
  function beforeUnload(event: BeforeUnloadEvent): void {
    if (!protectedDraft()) return;
    event.preventDefault();
  }
  if (typeof window !== "undefined") window.addEventListener("beforeunload", beforeUnload);
  onScopeDispose(() => {
    if (typeof window !== "undefined") window.removeEventListener("beforeunload", beforeUnload);
    owner = null;
    draft.value = null;
    finish(true);
  });
  return { open, draft, receive, requestLeave, keepEditing, discardAndLeave };
}
