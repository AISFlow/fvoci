import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Navigate, useNavigate, useSearchParams } from "react-router-dom";
import { LoginForm } from "@/features/auth/login";
import { api, ensureOk } from "@/lib/api";
import { meQuery, setupStatusQuery } from "@/lib/queries";

export function LoginPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [searchParams] = useSearchParams();
  const setupQuery = useQuery(setupStatusQuery);
  const meQueryState = useQuery(meQuery);
  const resetNotice = searchParams.get("reset") === "1";

  if (meQueryState.data) {
    return <Navigate to="/" replace />;
  }

  return (
    <LoginForm
      brandingName={setupQuery.data?.branding.name}
      unavailableNotice={null}
      mailEnabled={setupQuery.data?.mailEnabled === true}
      resetNotice={resetNotice}
      onPasswordReset={async (email) => {
        await ensureOk(
          await api.POST("/api/v1/auth/password-reset", {
            body: { email },
          }),
        );
      }}
      onSubmit={async (input) => {
        await ensureOk(
          await api.POST("/api/v1/auth/login", {
            body: input,
          }),
        );
        await queryClient.invalidateQueries();
        await navigate("/", { replace: true });
      }}
    />
  );
}
