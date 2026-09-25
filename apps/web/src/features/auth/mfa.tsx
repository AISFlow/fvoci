// Adapted from fvoci/FVOCI apps/web/src/features/auth/mfa.tsx
import { t } from "@fvoci/i18n";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { api, ensureOk, problemMessage } from "@/lib/api";
import { AuthAlert, AuthField, AuthInput, authPrimaryButtonClass } from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

/**
 * Second login step: password, magic-link, invitation or OIDC login answered
 * with `mfaToken` instead of a session. `onVerified` runs once the verify call
 * set the session cookie.
 */
export function MfaStep({
  mfaToken,
  onBack,
  onVerified,
  brandingName,
}: {
  mfaToken: string;
  onBack: () => void;
  onVerified: () => Promise<void> | void;
  brandingName?: string | null;
}) {
  return (
    <MfaVerifyForm
      brandingName={brandingName}
      onBack={onBack}
      onSubmit={async (code) => {
        await ensureOk(
          await api.POST("/api/v1/auth/mfa/verify", {
            body: { mfaToken, code },
          }),
        );
        await onVerified();
      }}
    />
  );
}

export function MfaVerifyForm({
  onSubmit,
  onBack,
  brandingName,
}: {
  onSubmit: (code: string) => Promise<void>;
  onBack: () => void;
  brandingName?: string | null;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const form = useForm<{ code: string }>({ defaultValues: { code: "" } });

  return (
    <AuthLayout brandingName={brandingName}>
      <AuthPanel title={t("auth.mfa.title")}>
        <form
          onSubmit={form.handleSubmit(async (values) => {
            setServerError(null);
            try {
              await onSubmit(values.code.trim());
            } catch (err) {
              setServerError(problemMessage(err, "error.auth.mfa"));
            }
          })}
          noValidate
          className="auth-shell__stack auth-shell__stack--form"
        >
          <AuthField id="mfa-code" label={t("auth.mfa.code")} hint={t("auth.mfa.code.hint")}>
            <AuthInput
              id="mfa-code"
              autoComplete="one-time-code"
              inputMode="numeric"
              autoFocus
              {...form.register("code", { required: true })}
            />
          </AuthField>
          {serverError ? <AuthAlert>{serverError}</AuthAlert> : null}
          <Button
            type="submit"
            size="lg"
            disabled={form.formState.isSubmitting}
            className={authPrimaryButtonClass}
          >
            {form.formState.isSubmitting ? t("auth.mfa.verifying") : t("auth.mfa.verify")}
          </Button>
          <Button type="button" variant="link" onClick={onBack} className="w-full justify-center">
            {t("auth.mfa.back")}
          </Button>
        </form>
      </AuthPanel>
    </AuthLayout>
  );
}
