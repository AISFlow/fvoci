// Adapted from fvoci/FVOCI apps/web/src/features/auth/reset-password.tsx
import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { problemMessage } from "@/lib/api";
import { formFieldMessage } from "@/lib/form-issues";
import { passwordResetConfirmInput } from "@/lib/validators";
import {
  AuthAlert,
  AuthField,
  AuthInput,
  authPrimaryButtonClass,
} from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

interface ResetPasswordViewProps {
  token: string | null;
  onConfirm: (newPassword: string) => Promise<void>;
}

export function ResetPasswordView({
  token,
  onConfirm,
}: ResetPasswordViewProps) {
  const [serverError, setServerError] = useState<string | null>(
    token ? null : t("magic_invalid"),
  );
  const form = useForm<{ newPassword: string }>({
    resolver: zodResolver(passwordResetConfirmInput.omit({ token: true })),
    defaultValues: { newPassword: "" },
  });
  const fieldError = formFieldMessage(
    form.formState.errors.newPassword,
    "newPassword",
  );

  if (!token) {
    return (
      <AuthLayout>
        <AuthPanel title={t("auth.reset.title")}>
          <AuthAlert>{serverError}</AuthAlert>
          <Link to="/login" className="auth-shell__link">
            {t("auth.reset.retry")}
          </Link>
        </AuthPanel>
      </AuthLayout>
    );
  }

  return (
    <AuthLayout>
      <AuthPanel title={t("auth.reset.title")}>
        <form
          onSubmit={form.handleSubmit(async (values) => {
            setServerError(null);
            try {
              await onConfirm(values.newPassword);
            } catch (err) {
              setServerError(problemMessage(err, "error.password.change"));
            }
          })}
          noValidate
          className="auth-shell__stack auth-shell__stack--form"
        >
          <AuthField
            id="reset-password-new"
            label={t("auth.passwordNew")}
            error={fieldError ?? undefined}
          >
            <AuthInput
              id="reset-password-new"
              type="password"
              autoComplete="new-password"
              {...form.register("newPassword")}
            />
          </AuthField>
          {serverError ? (
            <div className="auth-shell__stack">
              <AuthAlert>{serverError}</AuthAlert>
              <Link to="/login" className="auth-shell__link">
                {t("auth.reset.retry")}
              </Link>
            </div>
          ) : null}
          <Button
            type="submit"
            size="lg"
            disabled={form.formState.isSubmitting}
            className={authPrimaryButtonClass}
          >
            {form.formState.isSubmitting
              ? t("form.changing")
              : t("auth.reset.change")}
          </Button>
        </form>
      </AuthPanel>
    </AuthLayout>
  );
}
