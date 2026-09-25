import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useParams } from "react-router-dom";
import { InviteAcceptForm, InviteLoadError } from "@/features/auth/invite";
import { MfaStep } from "@/features/auth/mfa";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { invitationPublicQuery, providersQuery, setupStatusQuery } from "@/lib/queries";

export function InvitePage() {
  const { token = "" } = useParams<{ token: string }>();
  const invitation = useQuery(invitationPublicQuery(token));
  const setup = useQuery(setupStatusQuery);
  const providers = useQuery(providersQuery);
  const [mfaToken, setMfaToken] = useState<string | null>(null);

  if (mfaToken !== null) {
    return (
      <MfaStep
        mfaToken={mfaToken}
        brandingName={setup.data?.branding.name}
        onBack={() => window.location.assign("/login")}
        onVerified={() => window.location.assign("/")}
      />
    );
  }

  if (invitation.isError) {
    return (
      <InviteLoadError
        message={
          invitation.error instanceof ProblemError
            ? invitation.error.title
            : t("auth.invite.failed")
        }
      />
    );
  }
  if (!invitation.data) {
    return (
      <p className="p-8 text-muted-foreground" role="status">
        {t("load.loading")}
      </p>
    );
  }

  return (
    <InviteAcceptForm
      invitation={invitation.data}
      token={token}
      brandingName={setup.data?.branding.name}
      providers={providers.data?.providers}
      onSubmit={async (input) => {
        const result = await ensureOk(
          await api.POST("/api/v1/invitations/{token}/accept", {
            params: { path: { token } },
            body: input,
          }),
        );
        if (result.mfaToken) {
          setMfaToken(result.mfaToken);
          return;
        }
        window.location.assign("/");
      }}
    />
  );
}
