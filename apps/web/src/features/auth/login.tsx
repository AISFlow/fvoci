// Adapted from fvoci/FVOCI apps/web/src/features/auth/login.tsx
import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { problemMessage } from "@/lib/api";
import { formFieldMessage } from "@/lib/form-issues";
import type { LoginInput, ProviderOutput } from "@/lib/contracts";
import { oidcStartHref, WORKSPACE_SSO_ACTION } from "@/lib/oidc";
import { loginInput, magicLinkInput, passwordResetInput } from "@/lib/validators";
import {
  AuthAlert,
  AuthDisclosure,
  AuthField,
  AuthInput,
  AuthStatus,
  authOutlineButtonClass,
  authOutlineLinkClass,
  authPrimaryButtonClass,
} from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

const RESET_SENT_NOTICE = t("auth.reset.sent");
const RESET_DONE_NOTICE = t("auth.reset.done");
const MAGIC_SENT_NOTICE = t("auth.magic.sent");
const MAGIC_DISABLED_NOTICE = t("auth.magic.disabled");
const WITHDRAWN_NOTICE = t("auth.withdrawn");

function EmailActionForm({
  emailId,
  schema,
  onSubmit,
  submitLabel,
  sentNotice,
  errorKey,
}: {
  emailId: string;
  schema: typeof magicLinkInput | typeof passwordResetInput;
  onSubmit: (email: string) => Promise<void>;
  submitLabel: string;
  sentNotice: string;
  errorKey: "error.auth.magic" | "error.auth.resetRequest";
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const form = useForm<{ email: string }>({
    resolver: zodResolver(schema),
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
          setServerError(problemMessage(err, errorKey));
        }
      })}
      noValidate
      className="auth-shell__stack"
    >
      <AuthField
        id={emailId}
        label={t("auth.email")}
        error={fieldError ?? undefined}
      >
        <AuthInput
          id={emailId}
          type="email"
          autoComplete="email"
          {...form.register("email")}
        />
      </AuthField>
      {serverError ? <AuthAlert>{serverError}</AuthAlert> : null}
      {sent ? <AuthStatus>{sentNotice}</AuthStatus> : null}
      <Button
        type="submit"
        size="lg"
        disabled={form.formState.isSubmitting}
        className={authPrimaryButtonClass}
      >
        {form.formState.isSubmitting ? t("form.requesting") : submitLabel}
      </Button>
    </form>
  );
}

// Plain GET form: the server resolves the workspace slug and redirects the
// browser to that workspace's IdP.
function SsoSlugForm() {
  return (
    <form method="get" action={WORKSPACE_SSO_ACTION} className="auth-shell__stack">
      <AuthField id="login-sso-slug" label={t("auth.sso.slug")}>
        <AuthInput id="login-sso-slug" name="slug" autoComplete="off" maxLength={32} />
      </AuthField>
      <Button type="submit" variant="outline" size="lg" className={authOutlineButtonClass}>
        {t("auth.sso.login")}
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
  withdrawnNotice,
  magicLink,
  onMagicLink,
  notice,
  providers,
  providersLoading,
  workspaceSso,
}: {
  onSubmit: (input: LoginInput) => Promise<void>;
  brandingName?: string | null;
  unavailableNotice?: string | null;
  mailEnabled?: boolean;
  onPasswordReset?: (email: string) => Promise<void>;
  resetNotice?: boolean;
  withdrawnNotice?: boolean;
  // `undefined` while GET /auth/providers is loading: show neither state.
  magicLink?: boolean;
  onMagicLink?: (email: string) => Promise<void>;
  // OIDC callback `?error=` message.
  notice?: string | null;
  providers?: ProviderOutput[];
  providersLoading?: boolean;
  workspaceSso?: boolean;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const [magicOpen, setMagicOpen] = useState(false);
  const [resetOpen, setResetOpen] = useState(false);
  const form = useForm<LoginInput>({
    resolver: zodResolver(loginInput),
    defaultValues: { email: "", password: "" },
  });

  return (
    <AuthLayout brandingName={brandingName}>
      <AuthPanel title={t("auth.login")}>
        {resetNotice ? <AuthStatus>{RESET_DONE_NOTICE}</AuthStatus> : null}
        {withdrawnNotice ? <AuthStatus>{WITHDRAWN_NOTICE}</AuthStatus> : null}
        {unavailableNotice ? <AuthStatus>{unavailableNotice}</AuthStatus> : null}
        {notice ? <AuthAlert>{notice}</AuthAlert> : null}
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
        {magicLink === true && onMagicLink ? (
          <>
            <hr className="my-1 border-border" />
            <AuthDisclosure
              trigger={t("auth.magic.cta")}
              open={magicOpen}
              onOpenChange={setMagicOpen}
            >
              <EmailActionForm
                emailId="magic-link-email"
                schema={magicLinkInput}
                onSubmit={onMagicLink}
                submitLabel={t("auth.magic.submit")}
                sentNotice={MAGIC_SENT_NOTICE}
                errorKey="error.auth.magic"
              />
            </AuthDisclosure>
          </>
        ) : null}
        {mailEnabled && onPasswordReset ? (
          <AuthDisclosure
            trigger={t("auth.reset.forgot")}
            open={resetOpen}
            onOpenChange={setResetOpen}
          >
            <EmailActionForm
              emailId="password-reset-email"
              schema={passwordResetInput}
              onSubmit={onPasswordReset}
              submitLabel={t("auth.reset.submit")}
              sentNotice={RESET_SENT_NOTICE}
              errorKey="error.auth.resetRequest"
            />
          </AuthDisclosure>
        ) : null}
        {magicLink === false ? <AuthStatus>{MAGIC_DISABLED_NOTICE}</AuthStatus> : null}
        {providers && providers.length > 0 ? (
          <>
            <hr className="my-1 border-border" />
            <p className="text-ui font-medium text-muted-foreground">{t("auth.login.social")}</p>
            <div className="auth-shell__stack">
              {providers.map((p) => (
                <a
                  key={p.provider}
                  href={oidcStartHref(p.provider)}
                  className={authOutlineLinkClass}
                >
                  {p.label}
                </a>
              ))}
            </div>
          </>
        ) : null}
        {providersLoading || workspaceSso !== true ? null : (
          <>
            <hr className="my-1 border-border" />
            <SsoSlugForm />
          </>
        )}
      </AuthPanel>
    </AuthLayout>
  );
}
