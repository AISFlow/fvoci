import { useQueryClient } from "@tanstack/react-query";
import { useNavigate, useSearchParams } from "react-router-dom";
import { MagicLinkView } from "@/features/auth/magic-link";
import { api, ensureOk } from "@/lib/api";

export function MagicLinkPage() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const token = searchParams.get("token");

  return (
    <MagicLinkView
      token={token}
      onConsume={async (value) => {
        await ensureOk(
          await api.POST("/api/v1/auth/magic-link/consume", {
            body: { token: value },
          }),
        );
        await queryClient.invalidateQueries();
        await navigate("/", { replace: true });
      }}
    />
  );
}
