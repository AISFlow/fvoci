// Adapted from fvoci/FVOCI apps/web/src/features/auth/cancel-withdraw.tsx
import { t } from "@fvoci/i18n";
import { useState } from "react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import type { ErasureFragment } from "@/lib/erasure-hash";
import {
  AuthAlert,
  AuthStatus,
  authOutlineButtonClass,
  authPrimaryButtonClass,
} from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

const FALLBACK_TZ = "Asia/Seoul";

function formatDeadline(iso: string): string {
  return new Date(iso).toLocaleString("ko-KR", {
    hour12: false,
    timeZone: FALLBACK_TZ,
    year: "numeric",
    month: "long",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}

// Opening the page never cancels: the token is spent only on the button press.
export function CancelWithdrawView({
  token,
  eraseAt,
  scheduled,
  mailSent,
  recoveryHref,
  onCancel,
}: ErasureFragment & {
  recoveryHref: string | null;
  onCancel: (token: string) => Promise<void>;
}) {
  const [pending, setPending] = useState(false);
  const [done, setDone] = useState(false);
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(
    token ? null : t("auth.erasure.cancel.failed"),
  );

  async function handleCancel() {
    if (!token || pending) return;
    setPending(true);
    setError(null);
    try {
      await onCancel(token);
      setDone(true);
    } catch {
      setError(t("auth.erasure.cancel.failed"));
    } finally {
      setPending(false);
    }
  }

  return (
    <AuthLayout>
      <AuthPanel title={t("auth.erasure.title")}>
        {done ? (
          <div className="auth-shell__stack">
            <AuthStatus>{t("auth.erasure.cancel.done")}</AuthStatus>
            <Link to="/login" className="auth-shell__link">
              {t("auth.backToLogin")}
            </Link>
          </div>
        ) : (
          <div className="auth-shell__stack auth-shell__stack--form">
            {scheduled && eraseAt ? (
              <AuthStatus>
                {t("auth.erasure.scheduled", { date: formatDeadline(eraseAt) })}
              </AuthStatus>
            ) : null}
            {scheduled && recoveryHref ? (
              <>
                <AuthStatus>
                  {mailSent === false
                    ? t("auth.erasure.mailNotSent")
                    : t("auth.erasure.copyHint")}
                </AuthStatus>
                <p className="break-all font-mono text-ui text-foreground">{recoveryHref}</p>
                <Button
                  type="button"
                  variant="outline"
                  size="lg"
                  className={authOutlineButtonClass}
                  onClick={() => {
                    void copyText(recoveryHref).then(
                      () => setCopied(true),
                      () => setCopied(false),
                    );
                  }}
                >
                  {copied ? t("common.copyLink.done") : t("common.copyLink")}
                </Button>
              </>
            ) : null}
            {error ? (
              <div className="auth-shell__stack">
                <AuthAlert>{error}</AuthAlert>
                <Link to="/login" className="auth-shell__link">
                  {t("auth.backToLogin")}
                </Link>
              </div>
            ) : null}
            {token ? (
              <Button
                type="button"
                size="lg"
                disabled={pending}
                onClick={() => {
                  void handleCancel();
                }}
                className={authPrimaryButtonClass}
              >
                {pending ? t("auth.erasure.cancel.pending") : t("auth.erasure.cancel")}
              </Button>
            ) : null}
          </div>
        )}
      </AuthPanel>
    </AuthLayout>
  );
}
