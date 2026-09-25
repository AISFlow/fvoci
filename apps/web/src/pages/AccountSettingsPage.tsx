import { t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, Navigate, useSearchParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { AccountSettingsView } from "@/features/settings/settings-account";
import { MfaSection } from "@/features/settings/settings-account-mfa";
import { api, ensureOk } from "@/lib/api";
import { erasureRecoveryHash } from "@/lib/erasure-hash";
import { oidcErrorMessage } from "@/lib/oidc";
import { identitiesQuery, meQuery, mfaStatusQuery, providersQuery } from "@/lib/queries";

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
  const mfa = useQuery(mfaStatusQuery);
  const successNotice =
    searchParams.get("linked") === "1"
      ? t("auth.account.linkedNotice")
      : searchParams.get("email_changed") === "1"
        ? t("auth.account.emailChangedNotice")
        : null;
  const errorNotice = oidcErrorMessage(searchParams.get("error"));

  if (me.isError) {
    return <Navigate to="/login" replace />;
  }

  const failed = identities.isError || providers.isError || mfa.isError;
  const ready = me.data && identities.data && providers.data && mfa.data;

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
                  void mfa.refetch();
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
              errorNotice={errorNotice}
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
              onUnlink={async (provider) => {
                await ensureOk(
                  await api.POST("/api/v1/auth/oidc/{provider}/unlink", {
                    params: { path: { provider } },
                  }),
                );
                await queryClient.invalidateQueries({ queryKey: identitiesQuery.queryKey });
              }}
              mfa={
                <MfaSection
                  status={mfa.data}
                  hasPassword={me.data.hasPassword}
                  onSetup={async (input) =>
                    ensureOk(await api.POST("/api/v1/auth/mfa/setup", { body: input }))
                  }
                  onEnable={async (code) => {
                    await ensureOk(await api.POST("/api/v1/auth/mfa/enable", { body: { code } }));
                    await queryClient.invalidateQueries({ queryKey: mfaStatusQuery.queryKey });
                  }}
                  onDisable={async (input) => {
                    await ensureOk(await api.POST("/api/v1/auth/mfa/disable", { body: input }));
                    await queryClient.invalidateQueries({ queryKey: mfaStatusQuery.queryKey });
                  }}
                />
              }
            />
          )}
        </div>
      </main>
    </div>
  );
}
