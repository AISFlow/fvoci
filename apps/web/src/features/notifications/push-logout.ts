import { api } from "@/lib/api";
import { currentSubscription, localStore } from "@/features/notifications/push-browser";
import {
  logoutWithPushDisconnect,
  withTimeout,
  writePushOwner,
} from "@/features/notifications/push-subscription";

async function browserSubscription(): Promise<PushSubscription | null> {
  if (typeof navigator === "undefined" || !("serviceWorker" in navigator)) return null;
  return currentSubscription();
}

/** `POST /auth/logout` that also disconnects this browser's Web Push. */
export function logout() {
  return logoutWithPushDisconnect({
    currentEndpoint: () =>
      withTimeout(
        browserSubscription().then((subscription) => subscription?.endpoint ?? null),
        1000,
        null,
      ),
    postLogout: (pushEndpoint) =>
      api.POST("/api/v1/auth/logout", { body: pushEndpoint ? { pushEndpoint } : {} }),
    isOk: (result) => result.response.ok,
    unsubscribe: async () => {
      await withTimeout(
        browserSubscription().then((subscription) => subscription?.unsubscribe()),
        2000,
        undefined,
      );
    },
    clearOwner: () => {
      writePushOwner(localStore(), null);
    },
  });
}
