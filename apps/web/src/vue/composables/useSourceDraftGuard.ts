import type { FvociEditor } from "@fvoci/editor/vue";
import {
  computed,
  getCurrentInstance,
  inject,
  type InjectionKey,
  onScopeDispose,
  type Ref,
  ref,
  shallowRef,
  watch,
} from "vue";

export interface SourceDraftAuthScope {
  actorId: string | null;
  credentialId: string | null;
  workspaceId: string;
  lifetime: number;
}
export const sourceDraftAuthRetiredKey: InjectionKey<Readonly<Ref<boolean>>> = Symbol(
  "source draft auth retirement",
);
function sameAuthScope(a: SourceDraftAuthScope, b: SourceDraftAuthScope): boolean {
  return (
    a.actorId === b.actorId &&
    a.credentialId === b.credentialId &&
    a.workspaceId === b.workspaceId &&
    a.lifetime === b.lifetime
  );
}
/** A successful logout can deny only the shell lifetime that initiated it. */
export function createSourceDraftRetirement(current: () => SourceDraftAuthScope) {
  const retired = shallowRef<SourceDraftAuthScope | null>(null);
  return {
    capture: () => ({ ...current() }),
    denied: computed(() => !!retired.value && sameAuthScope(current(), retired.value)),
    retire(scope: SourceDraftAuthScope): boolean {
      if (!sameAuthScope(current(), scope)) return false;
      retired.value = { ...scope };
      return true;
    },
  };
}

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
  const authRetired = getCurrentInstance()
    ? inject(
        sourceDraftAuthRetiredKey,
        computed(() => false),
      )
    : computed(() => false);
  const authorized = () => options.authorized() && !authRetired.value;
  const open = ref(false);
  const draft = shallowRef<SourceDraftState | null>(null);
  let owner: object | null = null;
  let ownerIdentity: string | null = null;
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
    if (!authorized() || state.scope !== options.scope()) return;
    if (state.phase === "activate") {
      if (options.editor()?.sourceDraftState.owner !== state.owner) return;
      if (owner !== state.owner) finish(false);
      owner = state.owner;
      ownerIdentity = options.identity();
      draft.value = state;
    } else if (owner === state.owner) {
      if (state.phase === "retire") {
        // A same-actor renderer retirement is not the user's permission to
        // discard. Definitive auth/identity retirement is handled separately.
        draft.value = state;
        finish(false);
      } else draft.value = state;
    }
  }
  function protectedDraft(): boolean {
    const live = options.editor()?.sourceDraftState;
    const current = live?.owner === owner ? live : draft.value;
    return (
      authorized() &&
      ownerIdentity === options.identity() &&
      !!current &&
      current.owner === owner &&
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
      !authorized() ||
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
      // A caller's synchronous identity watcher may advance the capture scope
      // before this helper's identity watcher runs. Auth retirement wins in
      // either registration order, while ordinary capture changes cancel.
      if (!authorized() || (ownerIdentity !== null && ownerIdentity !== options.identity())) {
        owner = null;
        ownerIdentity = null;
        draft.value = null;
        finish(true);
        return;
      }
      // Reconnect/readonly capture invalidation cancels a pending user exit.
      // Retain the actual draft until its owner reports the current scope.
      finish(false);
    },
    { flush: "sync" },
  );
  watch(
    [options.identity, authorized],
    () => {
      owner = null;
      ownerIdentity = null;
      draft.value = null;
      finish(true);
    },
    { flush: "sync" },
  );
  function beforeUnload(event: Event): void {
    if (!protectedDraft()) return;
    event.preventDefault();
  }
  if (typeof window !== "undefined") window.addEventListener("beforeunload", beforeUnload);
  onScopeDispose(() => {
    if (typeof window !== "undefined") window.removeEventListener("beforeunload", beforeUnload);
    owner = null;
    ownerIdentity = null;
    draft.value = null;
    finish(true);
  });
  return {
    open,
    draft,
    authRetired,
    receive,
    requestLeave,
    keepEditing,
    discardAndLeave,
    beforeUnload,
  };
}
