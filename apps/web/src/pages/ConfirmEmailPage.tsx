import { useQueryClient } from "@tanstack/react-query";
import { useNavigate, useSearchParams } from "react-router-dom";
import { ConfirmEmailView } from "@/features/auth/confirm-email";
import { api, ensureOk } from "@/lib/api";

export function ConfirmEmailPage() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const token = searchParams.get("token");

  return (
    <ConfirmEmailView
      token={token}
      onConfirm={async (value) => {
        await ensureOk(
          await api.POST("/api/v1/auth/email/confirm", {
            body: { token: value },
          }),
        );
        await queryClient.invalidateQueries({ queryKey: ["auth", "me"] });
        await navigate("/settings/account?email_changed=1", { replace: true });
      }}
    />
  );
}
