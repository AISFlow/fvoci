import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useParams } from "react-router-dom";
import { InviteAcceptForm, InviteLoadError } from "@/features/auth/invite";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { invitationPublicQuery, setupStatusQuery } from "@/lib/queries";

export function InvitePage() {
  const { token = "" } = useParams<{ token: string }>();
  const invitation = useQuery(invitationPublicQuery(token));
  const setup = useQuery(setupStatusQuery);

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
      brandingName={setup.data?.branding.name}
      onSubmit={async (input) => {
        await ensureOk(
          await api.POST("/api/v1/invitations/{token}/accept", {
            params: { path: { token } },
            body: input,
          }),
        );
        window.location.assign("/");
      }}
    />
  );
}
