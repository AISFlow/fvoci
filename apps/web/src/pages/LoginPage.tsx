import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { Navigate, useNavigate, useSearchParams } from "react-router-dom";
import { LoginForm } from "@/features/auth/login";
import { MfaStep } from "@/features/auth/mfa";
import { api, ensureOk } from "@/lib/api";
import { oidcErrorMessage, takeMfaFragment } from "@/lib/oidc";
import { meQuery, providersQuery, setupStatusQuery } from "@/lib/queries";

export function LoginPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [searchParams] = useSearchParams();
  const setupQuery = useQuery(setupStatusQuery);
  const meQueryState = useQuery(meQuery);
  const providers = useQuery(providersQuery);
  const resetNotice = searchParams.get("reset") === "1";
  const withdrawnNotice = searchParams.get("withdrawn") === "1";
  // The OIDC callback hands the pending MFA token over in the fragment: read
  // it once and drop it from the address bar.
  const [mfaToken, setMfaToken] = useState<string | null>(takeMfaFragment);
  const brandingName = setupQuery.data?.branding.name;

  async function enterApp(): Promise<void> {
    await queryClient.invalidateQueries();
    await navigate("/", { replace: true });
  }

  if (meQueryState.data) {
    return <Navigate to="/" replace />;
  }

  if (mfaToken !== null) {
    return (
      <MfaStep
        mfaToken={mfaToken}
        brandingName={brandingName}
        onBack={() => setMfaToken(null)}
        onVerified={enterApp}
      />
    );
  }

  return (
    <LoginForm
      brandingName={brandingName}
      unavailableNotice={null}
      mailEnabled={setupQuery.data?.mailEnabled === true}
      resetNotice={resetNotice}
      withdrawnNotice={withdrawnNotice}
      notice={oidcErrorMessage(searchParams.get("error"))}
      magicLink={providers.data?.magicLink}
      providers={providers.data?.providers}
      providersLoading={providers.isLoading}
      workspaceSso={providers.data?.workspaceSso}
      onMagicLink={async (email) => {
        await ensureOk(
          await api.POST("/api/v1/auth/magic-link", {
            body: { email },
          }),
        );
      }}
      onPasswordReset={async (email) => {
        await ensureOk(
          await api.POST("/api/v1/auth/password-reset", {
            body: { email },
          }),
        );
      }}
      onSubmit={async (input) => {
        const result = await ensureOk(
          await api.POST("/api/v1/auth/login", {
            body: input,
          }),
        );
        if (result.mfaToken) {
          setMfaToken(result.mfaToken);
          return;
        }
        await enterApp();
      }}
    />
  );
}
