import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { MagicLinkView } from "@/features/auth/magic-link";
import { MfaStep } from "@/features/auth/mfa";
import { api, ensureOk } from "@/lib/api";

export function MagicLinkPage() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const token = searchParams.get("token");
  const [mfaToken, setMfaToken] = useState<string | null>(null);

  async function enterApp(): Promise<void> {
    await queryClient.invalidateQueries();
    await navigate("/", { replace: true });
  }

  if (mfaToken !== null) {
    return (
      <MfaStep
        mfaToken={mfaToken}
        onBack={() => {
          void navigate("/login");
        }}
        onVerified={enterApp}
      />
    );
  }

  return (
    <MagicLinkView
      token={token}
      onConsume={async (value) => {
        const result = await ensureOk(
          await api.POST("/api/v1/auth/magic-link/consume", {
            body: { token: value },
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
