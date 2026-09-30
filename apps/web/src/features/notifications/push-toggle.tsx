import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Label } from "@/components/ui/label";
import { subscribeWhenActive } from "@/features/notifications/push-activation";
import {
  currentSubscription,
  localStore,
  pushSupported,
  rebindPushSession,
  storeSubscription,
} from "@/features/notifications/push-browser";
import {
  attempted,
  boundTo,
  decodeKey,
  PermissionBlocked,
  type PushAttempted,
  pushBlocker,
  readPushOwner,
  SW_URL,
  writePushOwner,
} from "@/features/notifications/push-subscription";
import { meQuery } from "@/lib/queries";
import { publicInstanceQuery } from "@/lib/queries/admin";

/** Subscribes this browser and stores it for the signed-in user. */
async function subscribe(workspaceId: string, publicKey: string, userId: string): Promise<void> {
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
      // A subscription left by another account (or of unknown owner) shows as
      // off and is not touched until this user enables push.
      if (subscription && readPushOwner(localStore()) === userId) {
        if (!boundTo(subscription.options.applicationServerKey, publicKey)) {
          // VAPID was rotated: the old subscription can never deliver again.
          await subscription.unsubscribe();
          await subscribe(workspaceId, publicKey, userId);
        } else {
          // Re-bind to this session: sends need a live bound session, and its
          // logout disconnects this browser.
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
        } else if (readPushOwner(localStore()) === userId) {
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

/** {@link rebindPushSession} for the React workspace shell. */
export function usePushSessionRebind(workspaceId: string): void {
  const instance = useQuery(publicInstanceQuery);
  const me = useQuery(meQuery);
  const userId = me.data?.userId ?? null;
  const sessionId = me.data?.sessionId ?? null;
  const publicKey = instance.data?.values.webPushPublicKey ?? null;
  useEffect(() => {
    rebindPushSession({ workspaceId, publicKey, userId, sessionId });
  }, [publicKey, sessionId, userId, workspaceId]);
}
