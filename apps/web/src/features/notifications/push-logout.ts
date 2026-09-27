import { api } from "@/lib/api";
import {
  logoutWithPushDisconnect,
  SW_URL,
  withTimeout,
  writePushOwner,
} from "@/features/notifications/push-subscription";

function localStore(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

async function browserSubscription(): Promise<PushSubscription | null> {
  if (typeof navigator === "undefined" || !("serviceWorker" in navigator)) return null;
  const registration = await navigator.serviceWorker.getRegistration(SW_URL);
  return (await registration?.pushManager.getSubscription()) ?? null;
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
    clearOwner: () => writePushOwner(localStore(), null),
  });
}
