import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Label } from "@/components/ui/label";
import {
  attempted,
  boundTo,
  decodeKey,
  PermissionBlocked,
  type PushAttempted,
  pushBlocker,
  readPushOwner,
  SW_URL,
  subscriptionBody,
  writePushOwner,
} from "@/features/notifications/push-subscription";
import { api, ensureOk } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import { publicInstanceQuery } from "@/lib/queries/admin";

function localStore(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function pushSupported(): boolean {
  return (
    typeof navigator !== "undefined" &&
    "serviceWorker" in navigator &&
    typeof window !== "undefined" &&
    "PushManager" in window &&
    "Notification" in window
  );
}

async function currentSubscription(): Promise<PushSubscription | null> {
  const registration = await navigator.serviceWorker.getRegistration(SW_URL);
  return (await registration?.pushManager.getSubscription()) ?? null;
}

/** Stores (or re-binds to this session) the subscription for the signed-in user. */
async function storeSubscription(
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

/** Subscribes this browser and stores it for the signed-in user. */
async function subscribe(workspaceId: string, publicKey: string, userId: string): Promise<void> {
  if ((await Notification.requestPermission()) !== "granted") {
    throw new PermissionBlocked();
  }
  const registration = await navigator.serviceWorker.register(SW_URL);
  const subscription = await registration.pushManager.subscribe({
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
 * Whether this browser holds a subscription made with the live public key by
 * the signed-in user. Another account's leftover subscription is not "on".
 */
async function liveSubscribed(publicKey: string, userId: string): Promise<boolean> {
  const subscription = await currentSubscription();
  return (
    subscription !== null &&
    boundTo(subscription.options.applicationServerKey, publicKey) &&
    readPushOwner(localStore()) === userId
  );
}

/**
 * Per-browser push opt-in. Disabling only unsubscribes the browser: there is
 * no delete route, the sender drops the endpoint when the push service
 * answers 404/410. Logout disconnects this browser (`push-logout.ts`).
 */
export function PushToggle({ workspaceId }: { workspaceId: string }) {
  const instance = useQuery(publicInstanceQuery);
  const me = useQuery(meQuery);
  const userId = me.data?.userId ?? null;
  const publicKey = instance.data?.values.webPushPublicKey ?? null;
  const [enabled, setEnabled] = useState(false);
  const [failure, setFailure] = useState<PushAttempted | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!pushSupported() || publicKey === null || userId === null) return;
    let live = true;
    void (async () => {
      const subscription = await currentSubscription();
      if (subscription) {
        if (readPushOwner(localStore()) !== userId) {
          // Left behind by another account (or an unknown owner) in this
          // browser profile: never deliver its pushes to the current user.
          await subscription.unsubscribe();
          writePushOwner(localStore(), null);
        } else if (!boundTo(subscription.options.applicationServerKey, publicKey)) {
          // VAPID was rotated: the old subscription can never deliver again.
          await subscription.unsubscribe();
          await subscribe(workspaceId, publicKey, userId);
        } else {
          // Re-bind to this session so its logout disconnects this browser.
          await storeSubscription(workspaceId, subscription, userId);
        }
      }
      if (live) setEnabled(await liveSubscribed(publicKey, userId));
    })().catch(async (err: unknown) => {
      if (!live) return;
      setFailure(attempted(err));
      setEnabled(await liveSubscribed(publicKey, userId).catch(() => false));
    });
    return () => {
      live = false;
    };
  }, [publicKey, userId, workspaceId]);

  const reason = pushBlocker({
    supported: pushSupported(),
    instanceLoaded: instance.isSuccess,
    publicKey,
    attempted: failure,
  });
  const unavailable = reason === "unsupported" || reason === "unavailable";

  const toggle = (next: boolean): void => {
    if (publicKey === null || userId === null) return;
    setBusy(true);
    setFailure(null);
    void (async () => {
      try {
        if (next) {
          await subscribe(workspaceId, publicKey, userId);
        } else {
          writePushOwner(localStore(), null);
          await (await currentSubscription())?.unsubscribe();
        }
      } catch (err) {
        setFailure(attempted(err));
      } finally {
        setEnabled(await liveSubscribed(publicKey, userId).catch(() => false));
        setBusy(false);
      }
    })();
  };

  return (
    <>
      <label className="settings-form__row">
        <input
          id="prefs-push"
          type="checkbox"
          checked={enabled}
          disabled={unavailable || busy || userId === null}
          onChange={(event) => {
            toggle(event.target.checked);
          }}
        />
        <Label htmlFor="prefs-push">{t("notif.prefs.push")}</Label>
      </label>
      <p className="settings-notice">{t("notif.prefs.pushHint")}</p>
      {reason ? (
        <p role="alert" className="settings-notice settings-notice--danger">
          {t(`notif.push.${reason}`)}
        </p>
      ) : null}
    </>
  );
}
