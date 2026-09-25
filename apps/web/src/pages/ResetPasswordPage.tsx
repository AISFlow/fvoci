import { useSearchParams, useNavigate } from "react-router-dom";
import { ResetPasswordView } from "@/features/auth/reset-password";
import { api, ensureOk } from "@/lib/api";

export function ResetPasswordPage() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const token = searchParams.get("token");

  return (
    <ResetPasswordView
      token={token}
      onConfirm={async (newPassword) => {
        if (!token) return;
        await ensureOk(
          await api.POST("/api/v1/auth/password-reset/confirm", {
            body: { token, newPassword },
          }),
        );
        await navigate("/login?reset=1", { replace: true });
      }}
    />
  );
}
