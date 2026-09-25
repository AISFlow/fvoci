// Adapted from fvoci/FVOCI apps/web/src/features/auth/confirm-email.tsx
import { t } from "@fvoci/i18n";
import { useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { problemMessage } from "@/lib/api";
import { AuthAlert, authPrimaryButtonClass } from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

interface ConfirmEmailViewProps {
  token: string | null;
  onConfirm: (token: string) => Promise<void>;
}

export function ConfirmEmailView({ token, onConfirm }: ConfirmEmailViewProps) {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(token ? null : t("magic_invalid"));

  async function handleClick() {
    if (!token) return;
    setPending(true);
    setError(null);
    try {
      await onConfirm(token);
    } catch (err) {
      setError(problemMessage(err, "error.auth.confirm"));
    } finally {
      setPending(false);
    }
  }

  return (
    <AuthLayout>
      <AuthPanel title={t("confirm.email.title")}>
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
            {pending ? t("auth.emailChange.confirming") : t("confirm.email.submit")}
          </Button>
        ) : null}
      </AuthPanel>
    </AuthLayout>
  );
}
