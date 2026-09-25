import { useState } from "react";
import { CancelWithdrawView } from "@/features/auth/cancel-withdraw";
import { api, ensureOk } from "@/lib/api";
import { parseErasureHash } from "@/lib/erasure-hash";

export function CancelWithdrawPage() {
  const [fragment] = useState(() => parseErasureHash(window.location.hash));
  const recoveryHref =
    fragment.scheduled && fragment.token
      ? `${window.location.origin}/cancel-withdraw${window.location.hash}`
      : null;

  return (
    <CancelWithdrawView
      token={fragment.token}
      eraseAt={fragment.eraseAt}
      scheduled={fragment.scheduled}
      mailSent={fragment.mailSent}
      recoveryHref={recoveryHref}
      onCancel={async (token) => {
        await ensureOk(
          await api.POST("/api/v1/auth/cancel-withdraw", {
            body: { token },
          }),
        );
        // The spent token must not linger in history or a copied address.
        window.history.replaceState(null, "", "/cancel-withdraw");
      }}
    />
  );
}
