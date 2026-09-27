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
  SW_URL,
  subscriptionBody,
} from "@/features/notifications/push-subscription";
import { api, ensureOk } from "@/lib/api";
import { publicInstanceQuery } from "@/lib/queries/admin";

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

/** Subscribes this browser and stores it for the signed-in user. */
async function subscribe(workspaceId: string, publicKey: string): Promise<void> {
  if ((await Notification.requestPermission()) !== "granted") {
    throw new PermissionBlocked();
  }
  const registration = await navigator.serviceWorker.register(SW_URL);
  const subscription = await registration.pushManager.subscribe({
    userVisibleOnly: true,
    applicationServerKey: decodeKey(publicKey),
  });
  try {
    ensureOk(
      await api.PUT("/api/v1/workspaces/{workspace_id}/push-subscriptions", {
        params: { path: { workspace_id: workspaceId } },
        body: subscriptionBody(subscription.toJSON()),
      }),
    );
  } catch (err) {
    // Not stored on the server: do not leave a browser subscription that
    // shows the toggle as on while nothing can be delivered.
    await subscription.unsubscribe().catch(() => false);
    throw err;
  }
}

/** Whether this browser holds a subscription made with the live public key. */
async function liveSubscribed(publicKey: string): Promise<boolean> {
  const subscription = await currentSubscription();
  return subscription !== null && boundTo(subscription.options.applicationServerKey, publicKey);
}

/**
 * Per-browser push opt-in. Disabling only unsubscribes the browser: there is
 * no delete route, the sender drops the endpoint when the push service
 * answers 404/410.
 */
export function PushToggle({ workspaceId }: { workspaceId: string }) {
  const instance = useQuery(publicInstanceQuery);
  const publicKey = instance.data?.values.webPushPublicKey ?? null;
  const [enabled, setEnabled] = useState(false);
  const [failure, setFailure] = useState<PushAttempted | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!pushSupported() || publicKey === null) return;
    let live = true;
    void (async () => {
      const subscription = await currentSubscription();
      if (subscription && !boundTo(subscription.options.applicationServerKey, publicKey)) {
        // VAPID was rotated: the old subscription can never deliver again.
        await subscription.unsubscribe();
        await subscribe(workspaceId, publicKey);
      }
      if (live) setEnabled(await liveSubscribed(publicKey));
    })().catch(async (err: unknown) => {
      if (!live) return;
      setFailure(attempted(err));
      setEnabled(await liveSubscribed(publicKey).catch(() => false));
    });
    return () => {
      live = false;
    };
  }, [publicKey, workspaceId]);

  const reason = pushBlocker({
    supported: pushSupported(),
    instanceLoaded: instance.isSuccess,
    publicKey,
    attempted: failure,
  });
  const unavailable = reason === "unsupported" || reason === "unavailable";

  const toggle = (next: boolean): void => {
    if (publicKey === null) return;
    setBusy(true);
    setFailure(null);
    void (async () => {
      try {
        if (next) {
          await subscribe(workspaceId, publicKey);
        } else {
          await (await currentSubscription())?.unsubscribe();
        }
      } catch (err) {
        setFailure(attempted(err));
      } finally {
        setEnabled(await liveSubscribed(publicKey).catch(() => false));
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
          disabled={unavailable || busy}
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
