import {
  boundTo,
  readPushOwner,
  SW_URL,
  subscriptionBody,
  writePushOwner,
} from "@/features/notifications/push-subscription";
import { api, ensureOk } from "@/lib/api";

// This browser's Web Push subscription, framework-neutral: the React
// settings toggle (push-toggle.tsx) and both web apps' workspace shells use
// it. push-subscription.ts keeps the parts that need no browser globals.

export function localStore(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

export function pushSupported(): boolean {
  return (
    typeof navigator !== "undefined" &&
    "serviceWorker" in navigator &&
    typeof window !== "undefined" &&
    "PushManager" in window &&
    "Notification" in window
  );
}

export async function currentSubscription(): Promise<PushSubscription | null> {
  const registration = await navigator.serviceWorker.getRegistration(SW_URL);
  return (await registration?.pushManager.getSubscription()) ?? null;
}

/** Stores (or re-binds to this session) the subscription for the signed-in user. */
export async function storeSubscription(
  workspaceId: string,
  subscription: PushSubscription,
  userId: string,
): Promise<void> {
  ensureOk(
    await api.PUT("/api/v1/workspaces/{workspace_id}/push-subscriptions", {
      params: { path: { workspace_id: workspaceId } },
      body: subscriptionBody(subscription.toJSON()),
    }),
  );
  writePushOwner(localStore(), userId);
}

/**
 * Once per session, re-binds this browser's own subscription to the current
 * session (sends need a live bound session). Another account's subscription
 * is left alone. Failures are ignored: the settings toggle shows the state.
 */
export function rebindPushSession(input: {
  workspaceId: string;
  publicKey: string | null;
  userId: string | null;
  sessionId: string | null;
}): void {
  const { workspaceId, publicKey, userId, sessionId } = input;
  if (!pushSupported() || publicKey === null || userId === null || sessionId === null) return;
  if (readPushOwner(localStore()) !== userId) return;
  const marker = `fvoci.push.rebound.${sessionId}`;
  try {
    if (sessionStorage.getItem(marker) !== null) return;
  } catch {
    /* fall through: re-binding twice is harmless */
  }
  void (async () => {
    const subscription = await currentSubscription();
    if (!subscription || !boundTo(subscription.options.applicationServerKey, publicKey)) return;
    await storeSubscription(workspaceId, subscription, userId);
    try {
      sessionStorage.setItem(marker, "1");
    } catch {
      /* ignore */
    }
  })().catch(() => undefined);
}
