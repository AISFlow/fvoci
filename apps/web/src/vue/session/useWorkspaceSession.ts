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
export function useWorkspaceSession(slug: MaybeRefOrGetter<string>, env: SessionEnvironment = BROWSER) {
  const queryClient = useQueryClient();
  const setup = useQuery(setupStatusQuery);
  const me = useQuery(meQuery);
  const workspaces = useQuery(workspacesQuery);
  const workspace = computed(() => workspaces.data.value?.items.find((item) => item.slug === toValue(slug)));

  const signedOut = computed(() => me.error.value instanceof ProblemError && me.error.value.status === 401);
  // Signed in (a cached `me` counts: a failed refetch keeps it) and a fresh
  // list without this workspace: the user cannot see it.
  const denied = computed(
    () => me.data.value !== undefined && workspaces.isSuccess.value && workspace.value === undefined,
  );

  watchEffect(() => {
    if (setup.data.value?.needed) env.redirect("/setup");
    else if (signedOut.value) env.redirect(loginPath(env.location()));
    else if (denied.value) env.redirect("/?denied=workspace");
  });

  // The server closes the access stream when membership or the session may
  // have changed; only a fresh workspace list without this workspace evicts.
  watch(
    () => workspace.value?.id,
    (workspaceId, _previous, onCleanup) => {
      if (!workspaceId) return;
      const subscription = env.watchAccess(workspaceId, {
        onAccessChange: async () => {
          try {
            const list = await queryClient.fetchQuery({ ...workspacesQuery, staleTime: 0 });
            if (!list.items.some((item) => item.id === workspaceId)) env.redirect("/?denied=workspace");
          } catch {
            // A transport or list failure alone must not evict the page.
          }
        },
      });
      onCleanup(() => subscription.close());
    },
    { immediate: true },
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
    if (setup.isError.value) void setup.refetch();
    if (me.isError.value) void me.refetch();
    if (workspaces.isError.value) void workspaces.refetch();
  }

  return { me: computed(() => me.data.value), workspace, status, retry };
}
