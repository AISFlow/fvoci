import { t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, Navigate, useSearchParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import {
  changePassword,
  disableMfa,
  downloadMeExport,
  enableMfa,
  requestEmailChange,
  saveProfileName,
  sendEmailVerification,
  setUpMfa,
  unlinkIdentity,
  withdrawAccount,
} from "@/features/settings/account-requests";
import { AccountSettingsView } from "@/features/settings/settings-account";
import { AuthenticatedLegalNav } from "@/features/legal/operator-info";
import { MfaSection } from "@/features/settings/settings-account-mfa";
import { oidcErrorMessage } from "@/lib/oidc";
import { identitiesQuery, meQuery, mfaStatusQuery, providersQuery } from "@/lib/queries";

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
              onSaveName={(input) => saveProfileName(queryClient, input)}
              onSendVerification={sendEmailVerification}
              onChangeEmail={requestEmailChange}
              onChangePassword={(input) => changePassword(queryClient, input)}
              onWithdraw={async (input) => {
                window.location.assign(await withdrawAccount(input));
              }}
              onExport={downloadMeExport}
              onUnlink={(provider) => unlinkIdentity(queryClient, provider)}
              mfa={
                <MfaSection
                  status={mfa.data}
                  hasPassword={me.data.hasPassword}
                  onSetup={setUpMfa}
                  onEnable={(code) => enableMfa(queryClient, code)}
                  onDisable={(input) => disableMfa(queryClient, input)}
                />
              }
            />
          )}
        </div>
      </main>
      <footer className="border-t border-border px-4 py-3">
        <AuthenticatedLegalNav />
      </footer>
    </div>
  );
}
