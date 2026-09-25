// Adapted from fvoci/FVOCI apps/web/src/features/auth/magic-link.tsx
import { t } from "@fvoci/i18n";
import { useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { problemMessage } from "@/lib/api";
import { AuthAlert, authPrimaryButtonClass } from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

interface MagicLinkViewProps {
  token: string | null;
  onConsume: (token: string) => Promise<void>;
}

// The link is single-use, so consuming waits for an explicit click instead of
// firing on load (mail scanners prefetch links).
export function MagicLinkView({ token, onConsume }: MagicLinkViewProps) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(token ? null : t("magic_invalid"));

  async function handleClick() {
    if (!token) return;
    setPending(true);
    setError(null);
    try {
      await onConsume(token);
    } catch (err) {
      setError(problemMessage(err, "error.auth.login"));
    } finally {
      setPending(false);
    }
  }

  return (
    <AuthLayout>
      <AuthPanel title={t("auth.magic.title")}>
        {error ? (
          <div className="auth-shell__stack">
            <AuthAlert>{error}</AuthAlert>
            <Link to="/login" className="auth-shell__link">
              {t("auth.backToLogin")}
            </Link>
          </div>
        ) : null}
        {token && !error ? (
          <Button
            type="button"
            size="lg"
            disabled={pending}
            onClick={() => {
              void handleClick();
            }}
            className={authPrimaryButtonClass}
          >
            {pending ? t("auth.login.pending") : t("auth.login")}
          </Button>
        ) : null}
      </AuthPanel>
    </AuthLayout>
  );
}
