import { t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, Navigate, useSearchParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { AccountSettingsView } from "@/features/settings/settings-account";
import { api, ensureOk } from "@/lib/api";
import { erasureRecoveryHash } from "@/lib/erasure-hash";
import { identitiesQuery, meQuery, providersQuery } from "@/lib/queries";

async function downloadMeExport(): Promise<void> {
  const blob = await ensureOk(await api.GET("/api/v1/me/export", { parseAs: "blob" }));
  const href = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = href;
  link.download = "fvoci-export.zip";
  link.rel = "noopener";
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(href);
}

export function AccountSettingsPage() {
  const queryClient = useQueryClient();
  const [searchParams] = useSearchParams();
  const me = useQuery(meQuery);
  const identities = useQuery(identitiesQuery);
  const providers = useQuery(providersQuery);
  const successNotice =
    searchParams.get("email_changed") === "1" ? t("auth.account.emailChangedNotice") : null;

  if (me.isError) {
    return <Navigate to="/login" replace />;
  }

  const failed = identities.isError || providers.isError;
  const ready = me.data && identities.data && providers.data;

  return (
    <div className="app-shell">
      <header className="app-shell__header">
        <Link to="/" className="text-ui underline underline-offset-2">
          {t("nav.backHome")}
        </Link>
      </header>
      <main className="app-shell__main">
        <div className="settings-page">
          {failed ? (
            <div>
              <p role="alert" className="text-muted-foreground">
                {t("load.failed")}
              </p>
              <Button
                type="button"
                size="sm"
                className="mt-2"
                onClick={() => {
                  void identities.refetch();
                  void providers.refetch();
                }}
              >
                {t("load.retry")}
              </Button>
            </div>
          ) : !ready ? (
            <p role="status" className="text-muted-foreground">
              {t("load.loading")}
            </p>
          ) : (
            <AccountSettingsView
              me={me.data}
              identities={identities.data.items}
              providers={providers.data.providers}
              magicLink={providers.data.magicLink}
              successNotice={successNotice}
              onSaveName={async (input) => {
                await ensureOk(await api.PATCH("/api/v1/auth/me", { body: input }));
                await queryClient.invalidateQueries({ queryKey: ["auth", "me"] });
              }}
              onSendVerification={async (email) => {
                await ensureOk(await api.POST("/api/v1/auth/magic-link", { body: { email } }));
              }}
              onChangeEmail={async (newEmail) => {
                await ensureOk(await api.PATCH("/api/v1/auth/email", { body: { newEmail } }));
              }}
              onChangePassword={async (input) => {
                await ensureOk(await api.PATCH("/api/v1/auth/password", { body: input }));
                await queryClient.invalidateQueries({ queryKey: ["auth", "me"] });
              }}
              onWithdraw={async (input) => {
                const result = await ensureOk(
                  await api.POST("/api/v1/auth/withdraw", { body: input }),
                );
                // The response already cleared the session cookie; a full load
                // drops every cached query of the withdrawn account.
                window.location.assign(
                  `/cancel-withdraw#${erasureRecoveryHash({
                    token: result.cancelToken,
                    eraseAt: result.eraseAt,
                    mailSent: result.mailSent,
                  })}`,
                );
              }}
              onExport={downloadMeExport}
            />
          )}
        </div>
      </main>
    </div>
  );
}
