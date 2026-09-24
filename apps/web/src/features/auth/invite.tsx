import { t } from "@fvoci/i18n";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { problemMessage } from "@/lib/api";
import type { InvitationAcceptInput, InvitationPublicOutput } from "@/lib/contracts";
import { invitationAcceptInput } from "@/lib/validators";
import {
  AuthAlert,
  AuthField,
  AuthInput,
  AuthStatus,
  authPrimaryButtonClass,
} from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

type AcceptField = "email" | "familyName" | "givenName" | "password";

function isAcceptField(value: string): value is AcceptField {
  return ["email", "familyName", "givenName", "password"].includes(value);
}

function roleLabel(role: string): string {
  if (role === "owner") return t("role.owner");
  if (role === "admin") return t("role.admin");
  if (role === "guest") return t("role.guest");
  return t("role.member");
}

export function InviteLoadError({ message }: { message: string }) {
  return (
    <AuthLayout>
      <AuthPanel title={t("auth.invite.title", { name: "…" })}>
        <AuthAlert>{message}</AuthAlert>
      </AuthPanel>
    </AuthLayout>
  );
}

export function InviteAcceptForm({
  invitation,
  brandingName,
  onSubmit,
}: {
  invitation: InvitationPublicOutput;
  brandingName?: string | null;
  onSubmit: (input: InvitationAcceptInput) => Promise<void>;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const form = useForm<{
    email: string;
    familyName: string;
    givenName: string;
    password: string;
  }>({
    defaultValues: { email: "", familyName: "", givenName: "", password: "" },
  });
  const errors = form.formState.errors;

  return (
    <AuthLayout brandingName={brandingName}>
      <AuthPanel
        title={t("auth.invite.title", { name: invitation.workspaceName })}
        lead={t("auth.invite.body", {
          email: invitation.emailMasked,
          role: roleLabel(invitation.role),
        })}
      >
        <form
          onSubmit={form.handleSubmit(async (values) => {
            setServerError(null);
            const payload = {
              email: values.email || undefined,
              familyName: values.familyName || undefined,
              givenName: values.givenName || undefined,
              password: values.password || undefined,
            };
            const parsed = invitationAcceptInput.safeParse(payload);
            if (!parsed.success) {
              for (const issue of parsed.error.issues) {
                const field = String(issue.path[0] ?? "");
                const message = issue.message.startsWith("i18n:")
                  ? t(issue.message.slice(5) as Parameters<typeof t>[0])
                  : issue.message;
                if (isAcceptField(field)) {
                  form.setError(field, { message });
                } else {
                  setServerError(message);
                }
              }
              return;
            }
            try {
              await onSubmit(parsed.data);
            } catch (err) {
              setServerError(problemMessage(err, "error.auth.invite"));
            }
          })}
          noValidate
          className="auth-shell__stack auth-shell__stack--form"
        >
          <AuthField
            id="invite-email"
            label={t("auth.email")}
            error={errors.email?.message}
          >
            <AuthInput
              id="invite-email"
              type="email"
              autoComplete="email"
              aria-invalid={errors.email ? true : undefined}
              {...form.register("email")}
            />
          </AuthField>
          <div className="grid grid-cols-[6rem_minmax(0,1fr)] gap-3">
            <AuthField
              id="invite-family-name"
              label={t("settings.familyName")}
              error={errors.familyName?.message}
            >
              <AuthInput
                id="invite-family-name"
                autoComplete="family-name"
                aria-invalid={errors.familyName ? true : undefined}
                {...form.register("familyName")}
              />
            </AuthField>
            <AuthField
              id="invite-given-name"
              label={t("settings.givenName")}
              error={errors.givenName?.message}
            >
              <AuthInput
                id="invite-given-name"
                autoComplete="given-name"
                aria-invalid={errors.givenName ? true : undefined}
                {...form.register("givenName")}
              />
            </AuthField>
          </div>
          <AuthStatus>{t("auth.invite.newAccountHint")}</AuthStatus>
          <AuthField
            id="invite-password"
            label={t("auth.password")}
            error={errors.password?.message}
          >
            <AuthInput
              id="invite-password"
              type="password"
              autoComplete="new-password"
              aria-invalid={errors.password ? true : undefined}
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
            {form.formState.isSubmitting
              ? t("auth.invite.accepting")
              : t("auth.invite.accept")}
          </Button>
        </form>
      </AuthPanel>
    </AuthLayout>
  );
}
