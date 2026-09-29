import { useQuery } from "@tanstack/vue-query";
import { toValue, watch, type MaybeRefOrGetter } from "vue";
import { rebindPushSession } from "@/features/notifications/push-browser";
import { meQuery } from "@/lib/queries";
import { publicInstanceQuery } from "@/lib/queries/instance";

/** {@link rebindPushSession} for the Vue workspace shell. */
export function usePushSessionRebind(workspaceId: MaybeRefOrGetter<string | undefined>): void {
  const instance = useQuery(publicInstanceQuery);
  const me = useQuery(meQuery);
  watch(
    () => ({
      workspaceId: toValue(workspaceId),
      publicKey: instance.data.value?.values.webPushPublicKey ?? null,
      userId: me.data.value?.userId ?? null,
      sessionId: me.data.value?.sessionId ?? null,
    }),
    (input) => {
      if (!input.workspaceId) return;
      rebindPushSession({
        workspaceId: input.workspaceId,
        publicKey: input.publicKey,
        userId: input.userId,
        sessionId: input.sessionId,
      });
    },
    { immediate: true },
  );
}
