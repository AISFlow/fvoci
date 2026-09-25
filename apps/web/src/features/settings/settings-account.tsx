// Adapted from fvoci/FVOCI apps/web/src/features/settings/settings-account.tsx
// Locale/timezone/week-start/text-scale/theme, MFA and OIDC link/unlink are
// not ported; the sections below are wired to the Rust account endpoints.
import { t } from "@fvoci/i18n";
import { zodResolver } from "@hookform/resolvers/zod";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { problemMessage } from "@/lib/api";
import type {
  IdentityOutput,
  PasswordChangeInput,
  ProfileNameInput,
  ProviderOutput,
  SessionUserOutput,
  WithdrawInput,
} from "@/lib/contracts";
import { formFieldMessage } from "@/lib/form-issues";
import {
  emailChangeInput,
  passwordChangeForm,
  passwordCreateForm,
  profileNameInput,
  withdrawConfirmForm,
} from "@/lib/validators";
import "./settings-shell.css";

const SEND_DISABLED_NOTICE = t("auth.account.email.sendDisabled");
const VERIFY_SENT_NOTICE = t("auth.account.email.verifySent");
const EMAIL_CHANGE_SENT_NOTICE = t("auth.account.email.changeSent");
const PASSWORD_CHANGED_NOTICE = t("auth.account.password.changed");
const WITHDRAW_LOCAL_PART_HINT = t("auth.account.withdraw.hint");

export function EmailVerificationRow({
  email,
  verified,
  magicLink,
  onSendVerification,
}: {
  email: string;
  verified: boolean;
  magicLink: boolean;
  onSendVerification: (email: string) => Promise<void>;
}) {
  const [pending, setPending] = useState(false);
  const [sent, setSent] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function handleClick() {
    setError(null);
    setPending(true);
    try {
      await onSendVerification(email);
      setSent(true);
    } catch (err) {
      setError(problemMessage(err, "error.email.verifySend"));
    } finally {
      setPending(false);
    }
  }

  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <p className="text-ui text-muted-foreground" data-testid="account-email">
          {email}
        </p>
        <span role="status" className="text-dense text-muted-foreground">
          {verified ? t("auth.account.email.verified") : t("auth.account.email.unverified")}
        </span>
      </div>
      {!verified ? (
        <div className="flex flex-col gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="w-fit"
            disabled={!magicLink || pending}
            onClick={() => {
              void handleClick();
            }}
          >
            {t("auth.account.email.sendVerify")}
          </Button>
          {!magicLink ? (
            <p className="break-keep text-ui text-muted-foreground">{SEND_DISABLED_NOTICE}</p>
          ) : null}
          {error ? (
            <p role="alert" className="text-ui text-destructive">
              {error}
            </p>
          ) : null}
          {sent ? (
            <p role="status" className="text-ui text-muted-foreground">
              {VERIFY_SENT_NOTICE}
            </p>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

export function EmailChangeForm({
  magicLink,
  onChangeEmail,
}: {
  magicLink: boolean;
  onChangeEmail: (newEmail: string) => Promise<void>;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const form = useForm<{ newEmail: string }>({
    resolver: zodResolver(emailChangeInput),
    defaultValues: { newEmail: "" },
  });
  const fieldError = formFieldMessage(form.formState.errors.newEmail, "newEmail");

  return (
    <form
      onSubmit={form.handleSubmit(async (values) => {
        setServerError(null);
        try {
          await onChangeEmail(values.newEmail);
          setSent(true);
          form.reset({ newEmail: "" });
        } catch (err) {
          setServerError(problemMessage(err, "error.email.change"));
        }
      })}
      noValidate
      className="flex flex-col gap-1.5"
    >
      <Label htmlFor="settings-new-email">{t("auth.account.email.new")}</Label>
      <div className="flex gap-2">
        <Input
          id="settings-new-email"
          type="email"
          autoComplete="email"
          disabled={!magicLink}
          aria-invalid={form.formState.errors.newEmail ? true : undefined}
          {...form.register("newEmail")}
        />
        <Button type="submit" size="sm" disabled={!magicLink || form.formState.isSubmitting}>
          {form.formState.isSubmitting ? t("auth.emailChange.requesting") : t("common.change")}
        </Button>
      </div>
      {!magicLink ? (
        <p className="break-keep text-ui text-muted-foreground">{SEND_DISABLED_NOTICE}</p>
      ) : null}
      {fieldError ? (
        <p role="alert" className="text-ui text-destructive">
          {fieldError}
        </p>
      ) : null}
      {serverError ? (
        <p role="alert" className="text-ui text-destructive">
          {serverError}
        </p>
      ) : null}
      {sent ? (
        <p role="status" className="break-keep text-ui text-muted-foreground">
          {EMAIL_CHANGE_SENT_NOTICE}
        </p>
      ) : null}
    </form>
  );
}

function ProfileNameForm({
  me,
  onSaveName,
}: {
  me: SessionUserOutput;
  onSaveName: (input: ProfileNameInput) => Promise<void>;
}) {
  const [saveError, setSaveError] = useState<string | null>(null);
  const form = useForm<{ familyName: string; givenName: string }>({
    resolver: zodResolver(profileNameInput),
    defaultValues: { familyName: me.familyName ?? "", givenName: me.givenName },
  });
  const fieldError =
    formFieldMessage(form.formState.errors.familyName, "familyName") ??
    formFieldMessage(form.formState.errors.givenName, "givenName");

  return (
    <form
      onSubmit={form.handleSubmit(async (values) => {
        setSaveError(null);
        try {
          await onSaveName({
            givenName: values.givenName,
            familyName: values.familyName === "" ? null : values.familyName,
          });
        } catch (err) {
          setSaveError(problemMessage(err, "settings.save.failed"));
        }
      })}
      noValidate
      className="flex flex-col gap-1.5"
    >
      <Label htmlFor="settings-given-name">{t("settings.givenName")}</Label>
      <div className="flex gap-2">
        <Input
          id="settings-family-name"
          className="h-11 w-24 flex-none"
          aria-label={t("settings.familyName")}
          autoComplete="family-name"
          {...form.register("familyName")}
        />
        <Input
          id="settings-given-name"
          className="h-11 min-w-0 flex-1"
          autoComplete="given-name"
          {...form.register("givenName")}
        />
        <Button type="submit" size="sm" disabled={form.formState.isSubmitting}>
          {form.formState.isSubmitting ? t("settings.profile.saving") : t("settings.profile.save")}
        </Button>
      </div>
      {fieldError ? (
        <p role="alert" className="text-ui text-destructive">
          {fieldError}
        </p>
      ) : null}
      {saveError ? (
        <p role="alert" className="text-ui text-destructive">
          {saveError}
        </p>
      ) : null}
    </form>
  );
}

export function PasswordSection({
  hasPassword,
  onChangePassword,
}: {
  hasPassword: boolean;
  onChangePassword: (input: PasswordChangeInput) => Promise<void>;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);
  const form = useForm<{ currentPassword: string; newPassword: string }>({
    resolver: zodResolver(hasPassword ? passwordChangeForm : passwordCreateForm),
    defaultValues: { currentPassword: "", newPassword: "" },
  });
  const currentPasswordError = formFieldMessage(
    form.formState.errors.currentPassword,
    "currentPassword",
  );
  const newPasswordError = formFieldMessage(form.formState.errors.newPassword, "newPassword");

  return (
    <div className="flex flex-col gap-2">
      <p className="text-ui font-medium">{t("auth.password")}</p>
      <form
        onSubmit={form.handleSubmit(async (values) => {
          setServerError(null);
          setSuccess(false);
          try {
            await onChangePassword({
              currentPassword: hasPassword ? values.currentPassword : null,
              newPassword: values.newPassword,
            });
            setSuccess(true);
            form.reset({ currentPassword: "", newPassword: "" });
          } catch (err) {
            setServerError(problemMessage(err, "error.password.change"));
          }
        })}
        noValidate
        className="flex flex-col gap-1.5"
      >
        {hasPassword ? (
          <>
            <Label htmlFor="settings-current-password">{t("auth.passwordCurrent")}</Label>
            <Input
              id="settings-current-password"
              type="password"
              autoComplete="current-password"
              aria-invalid={currentPasswordError ? true : undefined}
              aria-describedby={
                currentPasswordError ? "settings-current-password-error" : undefined
              }
              {...form.register("currentPassword")}
            />
            {currentPasswordError ? (
              <p
                id="settings-current-password-error"
                role="alert"
                className="text-ui text-destructive"
              >
                {currentPasswordError}
              </p>
            ) : null}
          </>
        ) : null}
        <Label htmlFor="settings-new-password">
          {hasPassword ? t("auth.passwordNew") : t("auth.password")}
        </Label>
        <Input
          id="settings-new-password"
          type="password"
          autoComplete="new-password"
          aria-invalid={newPasswordError ? true : undefined}
          aria-describedby={newPasswordError ? "settings-new-password-error" : undefined}
          {...form.register("newPassword")}
        />
        {newPasswordError ? (
          <p id="settings-new-password-error" role="alert" className="text-ui text-destructive">
            {newPasswordError}
          </p>
        ) : null}
        {serverError ? (
          <p role="alert" className="text-ui text-destructive">
            {serverError}
          </p>
        ) : null}
        {success ? (
          <p role="status" className="text-ui text-muted-foreground">
            {PASSWORD_CHANGED_NOTICE}
          </p>
        ) : null}
        <Button type="submit" size="sm" disabled={form.formState.isSubmitting}>
          {form.formState.isSubmitting
            ? t("form.changing")
            : hasPassword
              ? t("auth.reset.change")
              : t("auth.account.password.create")}
        </Button>
      </form>
    </div>
  );
}

// Source "social accounts" block. The Rust server configures no OIDC
// providers yet, so this renders the empty state and never a link button.
function LoginMethodsSection({
  providers,
  identities,
}: {
  providers: ProviderOutput[];
  identities: IdentityOutput[];
}) {
  const linkedByProvider = new Map(identities.map((i) => [i.provider, i]));
  return (
    <div className="flex flex-col gap-2">
      <p className="text-ui font-medium">{t("auth.account.social.title")}</p>
      {providers.length === 0 ? (
        <p className="text-ui text-muted-foreground">{t("auth.account.social.empty")}</p>
      ) : null}
      {providers.map((p) => {
        const linked = linkedByProvider.get(p.provider);
        return (
          <p key={p.provider} className="text-ui">
            {p.label}
            {linked ? (
              <span className="text-muted-foreground">
                {" "}
                — {linked.email ?? t("common.connected")}
              </span>
            ) : null}
          </p>
        );
      })}
    </div>
  );
}

export function WithdrawSection({
  hasPassword,
  onWithdraw,
}: {
  hasPassword: boolean;
  onWithdraw: (input: WithdrawInput) => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const form = useForm<{ confirmValue: string }>({
    resolver: zodResolver(withdrawConfirmForm),
    defaultValues: { confirmValue: "" },
  });
  const fieldError = formFieldMessage(form.formState.errors.confirmValue, "confirmValue");

  return (
    <section className="settings-section">
      <h2 className="settings-section__title text-title">{t("auth.account.withdraw.title")}</h2>
      <form
        onSubmit={form.handleSubmit(async (values) => {
          setError(null);
          try {
            await onWithdraw({
              currentPassword: hasPassword ? values.confirmValue : null,
              emailLocalPart: hasPassword ? null : values.confirmValue,
            });
          } catch (err) {
            setError(problemMessage(err, "error.withdraw"));
          }
        })}
        noValidate
        className="flex flex-col gap-1.5"
      >
        <Label htmlFor="withdraw-confirm">
          {hasPassword ? t("auth.passwordCurrent") : t("auth.account.withdraw.localPart")}
        </Label>
        <p className="break-keep text-ui text-muted-foreground">
          {t("auth.account.withdraw.body")}
        </p>
        {!hasPassword ? (
          <p className="break-keep text-ui text-muted-foreground">{WITHDRAW_LOCAL_PART_HINT}</p>
        ) : null}
        <Input
          id="withdraw-confirm"
          type={hasPassword ? "password" : "text"}
          autoComplete={hasPassword ? "current-password" : "off"}
          {...form.register("confirmValue")}
        />
        {fieldError ?? error ? (
          <p role="alert" className="text-ui text-destructive">
            {fieldError ?? error}
          </p>
        ) : null}
        <Button
          type="submit"
          variant="destructive"
          size="sm"
          disabled={form.formState.isSubmitting}
        >
          {form.formState.isSubmitting
            ? t("auth.withdraw.pending")
            : t("auth.account.withdraw.submit")}
        </Button>
      </form>
    </section>
  );
}

interface AccountSettingsViewProps {
  me: SessionUserOutput;
  identities: IdentityOutput[];
  providers: ProviderOutput[];
  magicLink: boolean;
  successNotice?: string | null;
  onSaveName: (input: ProfileNameInput) => Promise<void>;
  onSendVerification: (email: string) => Promise<void>;
  onChangeEmail: (newEmail: string) => Promise<void>;
  onChangePassword: (input: PasswordChangeInput) => Promise<void>;
  onWithdraw: (input: WithdrawInput) => Promise<void>;
  onExport: () => Promise<void>;
}

export function AccountSettingsView({
  me,
  identities,
  providers,
  magicLink,
  successNotice,
  onSaveName,
  onSendVerification,
  onChangeEmail,
  onChangePassword,
  onWithdraw,
  onExport,
}: AccountSettingsViewProps) {
  const [exportPending, setExportPending] = useState(false);
  const [exportError, setExportError] = useState<string | null>(null);

  async function handleExport() {
    setExportError(null);
    setExportPending(true);
    try {
      await onExport();
    } catch (err) {
      setExportError(problemMessage(err, "error.export.me"));
    } finally {
      setExportPending(false);
    }
  }

  return (
    <div className="settings-stack">
      <section className="settings-section">
        <h1 className="settings-section__title text-title">{t("auth.account.title")}</h1>
        <div className="flex flex-col gap-6">
          {successNotice ? (
            <p role="status" className="text-ui text-muted-foreground">
              {successNotice}
            </p>
          ) : null}
          <EmailVerificationRow
            email={me.email}
            verified={me.emailVerifiedAt !== null}
            magicLink={magicLink}
            onSendVerification={onSendVerification}
          />
          <EmailChangeForm magicLink={magicLink} onChangeEmail={onChangeEmail} />
          <ProfileNameForm me={me} onSaveName={onSaveName} />
          <PasswordSection hasPassword={me.hasPassword} onChangePassword={onChangePassword} />
          <LoginMethodsSection providers={providers} identities={identities} />
        </div>
      </section>
      <section className="settings-section">
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="w-fit"
          disabled={exportPending}
          onClick={() => {
            void handleExport();
          }}
        >
          {t("export.me")}
        </Button>
        {exportError ? (
          <p role="alert" className="settings-notice settings-notice--danger">
            {exportError}
          </p>
        ) : null}
      </section>
      <WithdrawSection hasPassword={me.hasPassword} onWithdraw={onWithdraw} />
    </div>
  );
}
