import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Navigate, useNavigate } from "react-router-dom";
import { LoginForm } from "@/features/auth/login";
import { api, ensureOk } from "@/lib/api";
import { meQuery, setupStatusQuery } from "@/lib/queries";

export function LoginPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const setupQuery = useQuery(setupStatusQuery);
  const meQueryState = useQuery(meQuery);

  if (meQueryState.data) {
    return <Navigate to="/" replace />;
  }

  return (
    <LoginForm
      brandingName={setupQuery.data?.branding.name}
      unavailableNotice={null}
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
