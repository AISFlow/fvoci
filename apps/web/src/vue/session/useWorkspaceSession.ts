import { useQuery, useQueryClient } from "@tanstack/vue-query";
import {
  computed,
  onScopeDispose,
  ref,
  toValue,
  watch,
  watchEffect,
  type MaybeRefOrGetter,
} from "vue";
import { ProblemError } from "@/lib/api";
import { meQuery, setupStatusQuery, workspacesQuery } from "@/lib/queries";
import { watchWorkspaceAccess } from "@/lib/workspace-access-stream";
import { loginPath, redirectTo } from "./navigation";

export type SessionStatus = "loading" | "error" | "ready";

/** The browser pieces the session touches; tests pass their own. */
export interface SessionEnvironment {
  redirect(path: string): void;
  location(): Pick<Location, "pathname" | "search" | "hash">;
  watchAccess: typeof watchWorkspaceAccess;
}

const BROWSER: SessionEnvironment = {
  redirect: redirectTo,
  location: () => window.location,
  watchAccess: watchWorkspaceAccess,
};

/**
 * The React app's guards for a workspace page (SetupGuard, WorkspaceLayout,
 * useWorkspaceAccessWatch), for the Vue app: an instance that still needs
 * setup goes to /setup, a signed-out user to /login with this page as
 * returnTo, and a workspace the user cannot see (or loses while on the page)
 * to /?denied=workspace. A 428 consent_required answer is handled by the
 * shared API client (lib/api.ts consentGate).
 *
 * Unlike the React layout, only a 401 from /auth/me counts as signed out;
 * another failure (network, 5xx) shows a retry instead of the login page.
 */
export function useWorkspaceSession(
  slug: MaybeRefOrGetter<string>,
  env: SessionEnvironment = BROWSER,
) {
  const queryClient = useQueryClient();
  const setup = useQuery(setupStatusQuery);
  const me = useQuery(meQuery);
  const workspaces = useQuery(workspacesQuery);
  // Another tab can replace the cookie while this document is still mounted.
  // Private query keys do not include the actor: reenter through the existing
  // hard boundary before a new actor can consume the previous actor's cache.
  const actorChanged = ref(false);
  let actor: string | undefined;
  watch(
    () => me.data.value?.userId,
    (next) => {
      if (next === undefined) return;
      if (actor !== undefined && next !== actor) actorChanged.value = true;
      actor = next;
    },
    { immediate: true, flush: "sync" },
  );
  const workspace = computed(() =>
    workspaces.data.value?.items.find((item) => item.slug === toValue(slug)),
  );

  const signedOut = computed(
    () => me.error.value instanceof ProblemError && me.error.value.status === 401,
  );
  // Signed in (a cached `me` counts: a failed refetch keeps it) and a fresh
  // list without this workspace: the user cannot see it.
  const denied = computed(
    () =>
      me.data.value !== undefined && workspaces.isSuccess.value && workspace.value === undefined,
  );

  // Only a confirmed DELETE from the current page can own clean home. Keep
  // the captured identity after its workspace disappears from a fresh list;
  // metadata/access completions in the departing document are then expected.
  const deletedWorkspace = ref<{ id: string; slug: string; actor: string }>();
  let active = true;
  const leavingDeletedWorkspace = computed(() => {
    const deleted = deletedWorkspace.value;
    return (
      !!deleted &&
      active &&
      !actorChanged.value &&
      deleted.slug === toValue(slug) &&
      deleted.actor === me.data.value?.userId &&
      (workspace.value === undefined || workspace.value.id === deleted.id)
    );
  });
  watch(
    () => toValue(slug),
    () => {
      deletedWorkspace.value = undefined;
    },
    { flush: "sync" },
  );
  onScopeDispose(() => {
    active = false;
  });

  let requestedRedirect: string | undefined;
  watchEffect(() => {
    const path = setup.data.value?.needed
      ? "/setup"
      : signedOut.value
        ? loginPath(env.location())
        : actorChanged.value
          ? `${env.location().pathname}${env.location().search}${env.location().hash}`
          : leavingDeletedWorkspace.value
            ? "/"
            : denied.value
              ? "/?denied=workspace"
              : undefined;
    // Queries settle independently. Restarting the same pending navigation
    // (e.g. me 401, then setup completion) cancels its first document request.
    // A different destination still applies the guard priority above.
    if (path === undefined) {
      requestedRedirect = undefined;
      return;
    }
    if (path === requestedRedirect) return;
    requestedRedirect = path;
    env.redirect(path);
  });

  // The server closes the access stream when membership or the session may
  // have changed; only a fresh workspace list without this workspace evicts.
  watch(
    [() => workspace.value?.id, actorChanged, leavingDeletedWorkspace],
    ([workspaceId, changedActor, deleted], _previous, onCleanup) => {
      if (!workspaceId || changedActor || deleted) return;
      let active = true;
      const subscription = env.watchAccess(workspaceId, {
        onAccessChange: () => {
          if (!active) return;
          // The fresh list updates the reactive denied guard above. That guard
          // belongs to the current slug/scope and preserves setup/401 priority;
          // a retired subscription never redirects from its captured workspace.
          queryClient.query({ ...workspacesQuery, staleTime: 0 }).catch((error: unknown) => {
            // Query failures remain on the cache and must not evict the page.
            // Unexpected failures with no query owner remain observable.
            if (queryClient.getQueryState(workspacesQuery.queryKey)?.error !== error)
              reportError(error);
          });
        },
      });
      onCleanup(() => {
        active = false;
        subscription.close();
      });
    },
    { immediate: true, flush: "sync" },
  );

  const failedWithoutData = (query: { isError: { value: boolean }; data: { value: unknown } }) =>
    query.isError.value && query.data.value === undefined;

  const status = computed<SessionStatus>(() => {
    // Leaving for /setup, /login or home: keep showing "loading" until the page goes.
    if (
      setup.data.value?.needed ||
      signedOut.value ||
      actorChanged.value ||
      leavingDeletedWorkspace.value ||
      denied.value
    )
      return "loading";
    // A failed background refetch keeps the cached data (TanStack keeps `data`
    // with status "error"): only a query with nothing to show makes the page an
    // error, so one network blip during a stream reconnect does not unmount it.
    if (failedWithoutData(setup) || failedWithoutData(me) || failedWithoutData(workspaces)) {
      return "error";
    }
    if (!setup.data.value || !me.data.value || !workspace.value) return "loading";
    return "ready";
  });

  function retry(): void {
    if (setup.isError.value) setup.refetch().catch(reportError);
    if (me.isError.value) me.refetch().catch(reportError);
    if (workspaces.isError.value) workspaces.refetch().catch(reportError);
  }

  function leaveDeletedWorkspace(workspaceId: string): boolean {
    const userId = me.data.value?.userId;
    if (!active || status.value !== "ready" || !userId || workspace.value?.id !== workspaceId)
      return false;
    deletedWorkspace.value = { id: workspaceId, slug: toValue(slug), actor: userId };
    return true;
  }

  return { me: computed(() => me.data.value), workspace, status, retry, leaveDeletedWorkspace };
}
