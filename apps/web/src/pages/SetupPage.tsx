import { t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Navigate, useNavigate } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { SetupForm } from "@/features/auth/setup";
import { api, ensureOk } from "@/lib/api";
import { setupStatusQuery } from "@/lib/queries";

export function SetupPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const statusQuery = useQuery(setupStatusQuery);

  if (statusQuery.isLoading) {
    return <p role="status">{t("load.loading")}</p>;
  }

  if (statusQuery.isError) {
    return (
      <div className="p-8">
        <p role="alert" className="text-muted-foreground">{t("load.failed")}</p>
        <Button type="button" size="sm" className="mt-2" onClick={() => void statusQuery.refetch()}>
          {t("load.retry")}
        </Button>
      </div>
    );
  }

  if (statusQuery.data && !statusQuery.data.needed) {
    return <Navigate to="/login" replace />;
  }

  return (
    <SetupForm
      brandingName={statusQuery.data?.branding.name}
      onSubmit={async (input) => {
        await ensureOk(
          await api.POST("/api/v1/setup", {
            body: {
              email: input.email,
              password: input.password,
              givenName: input.givenName,
              familyName: input.familyName || undefined,
              workspaceSlug: input.workspaceSlug,
              workspaceName: input.workspaceName,
            },
          }),
        );
        await queryClient.invalidateQueries();
        await navigate("/", { replace: true });
      }}
    />
  );
}
