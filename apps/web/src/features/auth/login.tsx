// Adapted from fvoci/FVOCI apps/web/src/features/auth/login.tsx
import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { problemMessage } from "@/lib/api";
import { formFieldMessage } from "@/lib/form-issues";
import type { LoginInput } from "@/lib/contracts";
import { loginInput } from "@/lib/validators";
import {
  AuthAlert,
  AuthField,
  AuthInput,
  AuthStatus,
  authPrimaryButtonClass,
} from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

export function LoginForm({
  onSubmit,
  brandingName,
  unavailableNotice,
}: {
  onSubmit: (input: LoginInput) => Promise<void>;
  brandingName?: string | null;
  unavailableNotice?: string | null;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const form = useForm<LoginInput>({
    resolver: zodResolver(loginInput),
    defaultValues: { email: "", password: "" },
  });

  return (
    <AuthLayout brandingName={brandingName}>
      <AuthPanel title={t("auth.login")}>
        {unavailableNotice ? <AuthStatus>{unavailableNotice}</AuthStatus> : null}
        <form
          onSubmit={form.handleSubmit(async (values) => {
            setServerError(null);
            try {
              await onSubmit(values);
            } catch (err) {
              setServerError(problemMessage(err, "error.auth.login"));
            }
          })}
          noValidate
          className="auth-shell__stack auth-shell__stack--form"
        >
          <AuthField
            id="login-email"
            label={t("auth.email")}
            error={formFieldMessage(form.formState.errors.email, "email") ?? undefined}
          >
            <AuthInput id="login-email" type="email" autoComplete="email" {...form.register("email")} />
          </AuthField>
          <AuthField
            id="login-password"
            label={t("auth.password")}
            error={formFieldMessage(form.formState.errors.password, "password") ?? undefined}
          >
            <AuthInput
              id="login-password"
              type="password"
              autoComplete="current-password"
              {...form.register("password")}
            />
          </AuthField>
          {serverError ? <AuthAlert>{serverError}</AuthAlert> : null}
          <Button
            type="submit"
            size="lg"
            disabled={form.formState.isSubmitting}
            className={authPrimaryButtonClass}
          >
            {form.formState.isSubmitting ? t("auth.login.pending") : t("auth.login")}
          </Button>
        </form>
        <p className="unavailable-note">{t("auth.unsupported.notice")}</p>
      </AuthPanel>
    </AuthLayout>
  );
}
