// Adapted from fvoci/FVOCI apps/web/src/features/auth/login.tsx
import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { problemMessage } from "@/lib/api";
import { formFieldMessage } from "@/lib/form-issues";
import type { LoginInput } from "@/lib/contracts";
import { loginInput, passwordResetInput } from "@/lib/validators";
import {
  AuthAlert,
  AuthDisclosure,
  AuthField,
  AuthInput,
  AuthStatus,
  authPrimaryButtonClass,
} from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

const RESET_SENT_NOTICE = t("auth.reset.sent");
const RESET_DONE_NOTICE = t("auth.reset.done");

function PasswordResetForm({
  onSubmit,
}: {
  onSubmit: (email: string) => Promise<void>;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const form = useForm<{ email: string }>({
    resolver: zodResolver(passwordResetInput),
    defaultValues: { email: "" },
  });
  const fieldError = formFieldMessage(form.formState.errors.email, "email");

  return (
    <form
      onSubmit={form.handleSubmit(async (values) => {
        setServerError(null);
        try {
          await onSubmit(values.email);
          setSent(true);
        } catch (err) {
          setServerError(problemMessage(err, "error.auth.resetRequest"));
        }
      })}
      noValidate
      className="auth-shell__stack"
    >
      <AuthField
        id="password-reset-email"
        label={t("auth.email")}
        error={fieldError ?? undefined}
      >
        <AuthInput
          id="password-reset-email"
          type="email"
          autoComplete="email"
          {...form.register("email")}
        />
      </AuthField>
      {serverError ? <AuthAlert>{serverError}</AuthAlert> : null}
      {sent ? <AuthStatus>{RESET_SENT_NOTICE}</AuthStatus> : null}
      <Button
        type="submit"
        size="lg"
        disabled={form.formState.isSubmitting}
        className={authPrimaryButtonClass}
      >
        {form.formState.isSubmitting ? t("form.requesting") : t("auth.reset.submit")}
      </Button>
    </form>
  );
}

export function LoginForm({
  onSubmit,
  brandingName,
  unavailableNotice,
  mailEnabled,
  onPasswordReset,
  resetNotice,
}: {
  onSubmit: (input: LoginInput) => Promise<void>;
  brandingName?: string | null;
  unavailableNotice?: string | null;
  mailEnabled?: boolean;
  onPasswordReset?: (email: string) => Promise<void>;
  resetNotice?: boolean;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const [resetOpen, setResetOpen] = useState(false);
  const form = useForm<LoginInput>({
    resolver: zodResolver(loginInput),
    defaultValues: { email: "", password: "" },
  });

  return (
    <AuthLayout brandingName={brandingName}>
      <AuthPanel title={t("auth.login")}>
        {resetNotice ? <AuthStatus>{RESET_DONE_NOTICE}</AuthStatus> : null}
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
        {mailEnabled && onPasswordReset ? (
          <AuthDisclosure
            trigger={t("auth.reset.forgot")}
            open={resetOpen}
            onOpenChange={setResetOpen}
          >
            <PasswordResetForm onSubmit={onPasswordReset} />
          </AuthDisclosure>
        ) : null}
        <p className="unavailable-note">{t("auth.unsupported.notice")}</p>
      </AuthPanel>
    </AuthLayout>
  );
}
