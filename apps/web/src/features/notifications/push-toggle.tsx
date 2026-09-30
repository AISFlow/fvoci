import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { Label } from "@/components/ui/label";
import { pushSupported, rebindPushSession } from "@/features/notifications/push-browser";
import {
  liveSubscribed,
  refreshOwnSubscription,
  subscribePush,
  unsubscribePush,
} from "@/features/notifications/push-enable";
import { attempted, type PushAttempted, pushBlocker } from "@/features/notifications/push-subscription";
import { meQuery } from "@/lib/queries";
import { publicInstanceQuery } from "@/lib/queries/admin";

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
      await refreshOwnSubscription(workspaceId, publicKey, userId);
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
          await subscribePush(workspaceId, publicKey, userId);
        } else {
          await unsubscribePush(userId);
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
