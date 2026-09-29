import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, toValue, watch, watchEffect, type MaybeRefOrGetter } from "vue";
import { ProblemError } from "@/lib/api";
import { meQuery, setupStatusQuery, workspacesQuery } from "@/lib/queries";
import { watchWorkspaceAccess } from "@/lib/workspace-access-stream";
import { loginPath, redirectTo } from "./navigation";

export type SessionStatus = "loading" | "error" | "ready";

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
export function useWorkspaceSession(slug: MaybeRefOrGetter<string>) {
  const queryClient = useQueryClient();
  const setup = useQuery(setupStatusQuery);
  const me = useQuery(meQuery);
  const workspaces = useQuery(workspacesQuery);
  const workspace = computed(() => workspaces.data.value?.items.find((item) => item.slug === toValue(slug)));

  const signedOut = computed(() => me.error.value instanceof ProblemError && me.error.value.status === 401);
  const denied = computed(() => me.isSuccess.value && workspaces.isSuccess.value && workspace.value === undefined);

  watchEffect(() => {
    if (setup.data.value?.needed) redirectTo("/setup");
    else if (signedOut.value) redirectTo(loginPath(window.location));
    else if (denied.value) redirectTo("/?denied=workspace");
  });

  // The server closes the access stream when membership or the session may
  // have changed; only a fresh workspace list without this workspace evicts.
  watch(
    () => workspace.value?.id,
    (workspaceId, _previous, onCleanup) => {
      if (!workspaceId) return;
      const subscription = watchWorkspaceAccess(workspaceId, {
        onAccessChange: async () => {
          try {
            const list = await queryClient.fetchQuery({ ...workspacesQuery, staleTime: 0 });
            if (!list.items.some((item) => item.id === workspaceId)) redirectTo("/?denied=workspace");
          } catch {
            // A transport or list failure alone must not evict the page.
          }
        },
      });
      onCleanup(() => subscription.close());
    },
    { immediate: true },
  );

  const status = computed<SessionStatus>(() => {
    if (setup.isError.value || (me.isError.value && !signedOut.value) || workspaces.isError.value) return "error";
    if (!setup.data.value || setup.data.value.needed || !me.data.value || !workspace.value) return "loading";
    return "ready";
  });

  function retry(): void {
    if (setup.isError.value) void setup.refetch();
    if (me.isError.value) void me.refetch();
    if (workspaces.isError.value) void workspaces.refetch();
  }

  return { me: computed(() => me.data.value), workspace, status, retry };
}
