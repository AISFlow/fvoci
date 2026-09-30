import { subscribeWhenActive } from "@/features/notifications/push-activation";
import {
  currentSubscription,
  localStore,
  storeSubscription,
} from "@/features/notifications/push-browser";
import {
  boundTo,
  decodeKey,
  PermissionBlocked,
  readPushOwner,
  SW_URL,
  writePushOwner,
} from "@/features/notifications/push-subscription";

// The settings toggle's Web Push steps, framework-neutral: enabling,
// disabling and reading this browser's subscription for the signed-in user.

/** Subscribes this browser and stores it for the signed-in user. */
export async function subscribePush(
  workspaceId: string,
  publicKey: string,
  userId: string,
): Promise<void> {
  if ((await Notification.requestPermission()) !== "granted") {
    throw new PermissionBlocked();
  }
  const registration = await navigator.serviceWorker.register(SW_URL);
  // A subscription left by another account (or of unknown owner) is replaced,
  // never re-bound: subscribe() would return the same endpoint otherwise.
  const leftover = await registration.pushManager.getSubscription();
  if (leftover && readPushOwner(localStore()) !== userId) {
    await leftover.unsubscribe();
  }
  // A first registration is still installing; subscribing before it is
  // active fails, and nothing is stored until it succeeds.
  const subscription = await subscribeWhenActive(registration, {
    userVisibleOnly: true,
    applicationServerKey: decodeKey(publicKey),
  });
  try {
    await storeSubscription(workspaceId, subscription, userId);
  } catch (err) {
    // Not stored on the server: do not leave a browser subscription that
    // shows the toggle as on while nothing can be delivered.
    await subscription.unsubscribe().catch(() => false);
    throw err;
  }
}

/**
 * Disabling only unsubscribes the browser: there is no delete route, the
 * sender drops the endpoint when the push service answers 404/410. Another
 * account's subscription is left alone.
 */
export async function unsubscribePush(userId: string): Promise<void> {
  if (readPushOwner(localStore()) !== userId) return;
  writePushOwner(localStore(), null);
  await (await currentSubscription())?.unsubscribe();
}

/**
 * Whether this browser holds a subscription made with the live public key by
 * the signed-in user. Another account's leftover subscription is not "on".
 */
export async function liveSubscribed(publicKey: string, userId: string): Promise<boolean> {
  const subscription = await currentSubscription();
  return (
    subscription !== null &&
    boundTo(subscription.options.applicationServerKey, publicKey) &&
    readPushOwner(localStore()) === userId
  );
}

/**
 * On opening the toggle: this user's own subscription is re-bound to this
 * session, or replaced when VAPID was rotated (the old one can never deliver
 * again). A subscription left by another account (or of unknown owner) is
 * not touched until this user enables push.
 */
export async function refreshOwnSubscription(
  workspaceId: string,
  publicKey: string,
  userId: string,
): Promise<void> {
  const subscription = await currentSubscription();
  if (!subscription || readPushOwner(localStore()) !== userId) return;
  if (!boundTo(subscription.options.applicationServerKey, publicKey)) {
    await subscription.unsubscribe();
    await subscribePush(workspaceId, publicKey, userId);
  } else {
    // Sends need a live bound session, and its logout disconnects this browser.
    await storeSubscription(workspaceId, subscription, userId);
  }
}
