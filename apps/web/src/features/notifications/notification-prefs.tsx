import { t } from "@fvoci/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { Label } from "@/components/ui/label";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { notificationPrefsQuery } from "@/lib/queries";
import "@/features/settings/settings-shell.css";

export function NotificationPrefsSection({ workspaceId }: { workspaceId: string }) {
  const queryClient = useQueryClient();
  const prefsQuery = useQuery(notificationPrefsQuery(workspaceId));
  const save = useMutation({
    mutationFn: async (body: { inApp: boolean; mailImmediate: boolean; mailDigest: boolean }) =>
      ensureOk(
        await api.PUT("/api/v1/workspaces/{workspace_id}/notification-prefs", {
          params: { path: { workspace_id: workspaceId } },
          body,
        }),
      ),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["notification-prefs", workspaceId] });
      await queryClient.invalidateQueries({ queryKey: ["notifications", workspaceId] });
      await queryClient.invalidateQueries({ queryKey: ["notifications-unread", workspaceId] });
    },
  });
  const prefs = prefsQuery.data;

  if (prefsQuery.isLoading) return <QueryLoading />;
  if (prefsQuery.isError) {
    return (
      <QueryError
        message={loadErrorMessage(prefsQuery.error)}
        onRetry={() => {
          void prefsQuery.refetch();
        }}
      />
    );
  }
  if (!prefs) return null;

  return (
    <section className="settings-section">
      <h2 className="settings-section__title">{t("settings.notifications.title")}</h2>
      <label className="settings-form__row">
        <input
          id="prefs-in-app"
          type="checkbox"
          checked={prefs.inApp}
          onChange={(event) => {
            save.mutate({ ...prefs, inApp: event.target.checked });
          }}
        />
        <Label htmlFor="prefs-in-app">{t("notif.prefs.inApp")}</Label>
      </label>
      <label className="settings-form__row">
        <input
          id="prefs-mail-immediate"
          type="checkbox"
          checked={prefs.mailImmediate}
          onChange={(event) => {
            save.mutate({ ...prefs, mailImmediate: event.target.checked });
          }}
        />
        <Label htmlFor="prefs-mail-immediate">{t("notif.prefs.mailImmediate")}</Label>
      </label>
      <label className="settings-form__row">
        <input
          id="prefs-mail-digest"
          type="checkbox"
          checked={prefs.mailDigest}
          onChange={(event) => {
            save.mutate({ ...prefs, mailDigest: event.target.checked });
          }}
        />
        <Label htmlFor="prefs-mail-digest">{t("notif.prefs.mailDigest")}</Label>
      </label>
      {save.error ? (
        <p role="alert" className="settings-notice">
          {save.error instanceof ProblemError ? save.error.title : t("settings.save.failed")}
        </p>
      ) : null}
    </section>
  );
}
