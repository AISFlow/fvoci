import { useQuery } from "@tanstack/vue-query";
import { toValue, watch, type MaybeRefOrGetter } from "vue";
import { rebindPushSession } from "@/features/notifications/push-browser";
import { meQuery } from "@/lib/queries";
import { publicInstanceQuery } from "@/lib/queries/instance";

/**
 * Once per session, re-binds this browser's own Web Push subscription to the
 * current session (push-browser.ts rebindPushSession, as the React workspace
 * shell does): the server only sends to a subscription bound to a live
 * session. Runs again when the workspace, the key, the user or the session
 * changes; another account's subscription is left alone.
 */
export function usePushSessionRebind(workspaceId: MaybeRefOrGetter<string>): void {
  const instance = useQuery(publicInstanceQuery);
  const me = useQuery(meQuery);
  watch(
    [
      () => toValue(workspaceId),
      () => instance.data.value?.values.webPushPublicKey ?? null,
      () => me.data.value?.userId ?? null,
      () => me.data.value?.sessionId ?? null,
    ],
    ([workspaceId, publicKey, userId, sessionId]) => {
      rebindPushSession({ workspaceId, publicKey, userId, sessionId });
    },
    { immediate: true },
  );
}
