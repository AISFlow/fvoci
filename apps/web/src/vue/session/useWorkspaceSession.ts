import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, toValue, watch, watchEffect, type MaybeRefOrGetter } from "vue";
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

  let requestedRedirect: string | undefined;
  watchEffect(() => {
    const path = setup.data.value?.needed
      ? "/setup"
      : signedOut.value
        ? loginPath(env.location())
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
    () => workspace.value?.id,
    (workspaceId, _previous, onCleanup) => {
      if (!workspaceId) return;
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
    if (setup.data.value?.needed || signedOut.value || denied.value) return "loading";
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

  return { me: computed(() => me.data.value), workspace, status, retry };
}
