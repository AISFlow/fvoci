// Adapted from fvoci/FVOCI apps/web/src/features/auth/setup.tsx
import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { problemMessage } from "@/lib/api";
import { formFieldMessage } from "@/lib/form-issues";
import type { SetupInput } from "@/lib/contracts";
import { setupInput } from "@/lib/validators";
import {
  AuthAlert,
  AuthField,
  AuthInput,
  authPrimaryButtonClass,
} from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

export function SetupForm({
  onSubmit,
  brandingName,
}: {
  onSubmit: (input: SetupInput) => Promise<void>;
  brandingName?: string | null;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const form = useForm<SetupInput>({
    resolver: zodResolver(setupInput),
    defaultValues: {
      email: "",
      password: "",
      familyName: "",
      givenName: "",
      workspaceSlug: "",
      workspaceName: "",
    },
  });
  const errors = form.formState.errors;

  return (
    <AuthLayout brandingName={brandingName}>
      <AuthPanel title={t("auth.setup.title")}>
        <form
          onSubmit={form.handleSubmit(async (values) => {
            setServerError(null);
            try {
              await onSubmit(values);
            } catch (err) {
              setServerError(problemMessage(err, "error.auth.setup"));
            }
          })}
          noValidate
          className="auth-shell__stack auth-shell__stack--form"
        >
          <div className="grid grid-cols-[6rem_minmax(0,1fr)] gap-3">
            <AuthField
              id="setup-family-name"
              label={t("settings.familyName")}
              error={formFieldMessage(errors.familyName, "familyName") ?? undefined}
            >
              <AuthInput id="setup-family-name" autoComplete="family-name" {...form.register("familyName")} />
            </AuthField>
            <AuthField
              id="setup-given-name"
              label={t("settings.givenName")}
              error={formFieldMessage(errors.givenName, "givenName") ?? undefined}
            >
              <AuthInput id="setup-given-name" autoComplete="given-name" {...form.register("givenName")} />
            </AuthField>
          </div>
          <AuthField
            id="setup-email"
            label={t("auth.email")}
            error={formFieldMessage(errors.email, "email") ?? undefined}
          >
            <AuthInput id="setup-email" type="email" autoComplete="email" {...form.register("email")} />
          </AuthField>
          <AuthField
            id="setup-password"
            label={t("auth.password")}
            error={formFieldMessage(errors.password, "password") ?? undefined}
          >
            <AuthInput id="setup-password" type="password" autoComplete="new-password" {...form.register("password")} />
          </AuthField>
          <AuthField
            id="setup-workspace-name"
            label={t("workspace.name")}
            error={formFieldMessage(errors.workspaceName, "workspaceName") ?? undefined}
          >
            <AuthInput id="setup-workspace-name" {...form.register("workspaceName")} />
          </AuthField>
          <AuthField
            id="setup-workspace-slug"
            label={t("auth.setup.slug")}
            hint={t("form.pattern.slug")}
            error={formFieldMessage(errors.workspaceSlug, "workspaceSlug") ?? undefined}
          >
            <AuthInput
              id="setup-workspace-slug"
              placeholder="my-workspace"
              autoComplete="off"
              spellCheck={false}
              {...form.register("workspaceSlug")}
            />
          </AuthField>
          {serverError ? <AuthAlert>{serverError}</AuthAlert> : null}
          <Button
            type="submit"
            size="lg"
            disabled={form.formState.isSubmitting}
            className={authPrimaryButtonClass}
          >
            {form.formState.isSubmitting ? t("form.creating") : t("auth.setup.start")}
          </Button>
        </form>
      </AuthPanel>
    </AuthLayout>
  );
}
