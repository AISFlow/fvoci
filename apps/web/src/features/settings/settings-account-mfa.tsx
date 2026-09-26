// Adapted from fvoci/FVOCI apps/web/src/features/settings/settings-account-mfa.tsx
import { t } from "@fvoci/i18n";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { problemMessage } from "@/lib/api";
import { qrModules } from "@/lib/qr";
import type { components } from "@/generated/api";
import "./settings-shell.css";

type MfaStatusOutput = components["schemas"]["MfaStatusOutput"];
type MfaSetupOutput = components["schemas"]["MfaSetupOutput"];
type MfaSetupInput = components["schemas"]["MfaSetupBody"];
type MfaDisableInput = components["schemas"]["MfaDisableBody"];

interface MfaSectionProps {
  status: MfaStatusOutput;
  hasPassword: boolean;
  onSetup: (input: MfaSetupInput) => Promise<MfaSetupOutput>;
  onEnable: (code: string) => Promise<void>;
  onDisable: (input: MfaDisableInput) => Promise<void>;
}

/* WHY: QR 은 모듈 격자를 path 하나로 그린다 — 라이브러리의 SVG 문자열 주입(innerHTML)을 피한다. */
function QrSvg({ text }: { text: string }) {
  const { size, path } = qrModules(text);
  return (
    <svg
      viewBox={`-2 -2 ${size + 4} ${size + 4}`}
      shapeRendering="crispEdges"
      className="size-44 rounded-md bg-white"
      aria-hidden="true"
      data-testid="mfa-qr"
    >
      <title>QR</title>
      <path d={path} fill="#000" />
    </svg>
  );
}

async function copyText(value: string): Promise<void> {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(value);
    return;
  }
  throw new Error("clipboard unavailable");
}

function SetupFlow({
  setup,
  onEnable,
  onDone,
}: {
  setup: MfaSetupOutput;
  onEnable: (code: string) => Promise<void>;
  onDone: () => void;
}) {
  const [enabled, setEnabled] = useState(false);
  const [copied, setCopied] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);
  const form = useForm<{ code: string }>({ defaultValues: { code: "" } });

  if (enabled) {
    return (
      <div className="flex flex-col gap-2">
        <p role="status" className="text-ui font-medium">
          {t("auth.account.mfa.enabled")}
        </p>
        <p className="text-ui font-medium">{t("auth.account.mfa.recovery.title")}</p>
        <p className="break-keep text-ui text-muted-foreground">
          {t("auth.account.mfa.recovery.body")}
        </p>
        <ul
          className="grid grid-cols-2 gap-x-6 gap-y-1 font-mono text-ui"
          data-testid="mfa-recovery-codes"
        >
          {setup.recoveryCodes.map((code) => (
            <li key={code}>{code}</li>
          ))}
        </ul>
        <div className="flex gap-2">
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => {
              copyText(setup.recoveryCodes.join("\n")).then(
                () => setCopied(true),
                () => setCopied(false),
              );
            }}
          >
            {copied ? t("auth.account.mfa.recovery.copied") : t("auth.account.mfa.recovery.copy")}
          </Button>
          <Button type="button" size="sm" onClick={onDone}>
            {t("auth.account.mfa.recovery.done")}
          </Button>
        </div>
      </div>
    );
  }

  return (
    <form
      onSubmit={form.handleSubmit(async (values) => {
        setServerError(null);
        try {
          await onEnable(values.code.trim());
          setEnabled(true);
        } catch (err) {
          setServerError(problemMessage(err, "error.auth.mfa"));
        }
      })}
      noValidate
      className="flex flex-col gap-2"
    >
      <p className="break-keep text-ui text-muted-foreground">{t("auth.account.mfa.scan")}</p>
      <div className="flex flex-wrap items-start gap-4">
        <a
          href={setup.otpauthUri}
          className="shrink-0"
          aria-label={t("auth.account.mfa.scan")}
          data-testid="mfa-otpauth-uri"
        >
          <QrSvg text={setup.otpauthUri} />
        </a>
        <div className="flex min-w-0 flex-col gap-1.5">
          <p className="text-ui font-medium">{t("auth.account.mfa.manualKey")}</p>
          <code className="break-all font-mono text-ui" data-testid="mfa-secret">
            {setup.secret}
          </code>
        </div>
      </div>
      <Label htmlFor="settings-mfa-code">{t("auth.mfa.code")}</Label>
      <Input
        id="settings-mfa-code"
        autoComplete="one-time-code"
        inputMode="numeric"
        {...form.register("code", { required: true })}
      />
      {serverError ? (
        <p role="alert" className="text-ui text-destructive">
          {serverError}
        </p>
      ) : null}
      <div className="flex gap-2">
        <Button type="submit" size="sm" disabled={form.formState.isSubmitting}>
          {form.formState.isSubmitting
            ? t("auth.mfa.setup.confirming")
            : t("auth.account.mfa.enable")}
        </Button>
        <Button type="button" variant="outline" size="sm" onClick={onDone}>
          {t("auth.mfa.setup.cancel")}
        </Button>
      </div>
    </form>
  );
}

/** One re-auth field plus a button: setup (password) and disable (password or current code) share it. */
function ReauthForm({
  label,
  secret,
  action,
  fallback,
  onSubmit,
}: {
  label: string | null;
  secret: boolean;
  action: string;
  fallback: "error.mfa.setup" | "error.mfa.disable";
  onSubmit: (value: string | null) => Promise<void>;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const form = useForm<{ confirm: string }>({ defaultValues: { confirm: "" } });
  return (
    <form
      onSubmit={form.handleSubmit(async (values) => {
        setServerError(null);
        try {
          await onSubmit(label === null ? null : values.confirm.trim());
        } catch (err) {
          setServerError(problemMessage(err, fallback));
        }
      })}
      noValidate
      className="flex flex-col gap-1.5"
    >
      {label !== null ? <Label htmlFor="settings-mfa-confirm">{label}</Label> : null}
      <div className="flex gap-2">
        {label !== null ? (
          <Input
            id="settings-mfa-confirm"
            type={secret ? "password" : "text"}
            autoComplete={secret ? "current-password" : "one-time-code"}
            {...form.register("confirm", { required: label !== null })}
          />
        ) : null}
        <Button
          type="submit"
          variant="outline"
          size="sm"
          disabled={form.formState.isSubmitting}
        >
          {form.formState.isSubmitting ? t("auth.mfa.reauth.pending") : action}
        </Button>
      </div>
      {serverError ? (
        <p role="alert" className="text-ui text-destructive">
          {serverError}
        </p>
      ) : null}
    </form>
  );
}

export function MfaSection({ status, hasPassword, onSetup, onEnable, onDisable }: MfaSectionProps) {
  const [setup, setSetup] = useState<MfaSetupOutput | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  return (
    <div className="flex flex-col gap-2" data-testid="mfa-section">
      <h2 className="text-ui font-medium">{t("auth.mfa.title")}</h2>
      <p className="text-ui text-muted-foreground" data-testid="mfa-status">
        {status.enabled
          ? t("auth.account.mfa.on", { count: status.recoveryCodesLeft })
          : t("auth.account.mfa.off")}
      </p>
      {notice ? (
        <p role="status" className="text-ui text-muted-foreground">
          {notice}
        </p>
      ) : null}
      {setup ? (
        // The recovery-code screen stays until the user confirms, even after
        // the status refetch flips `enabled`.
        <SetupFlow setup={setup} onEnable={onEnable} onDone={() => setSetup(null)} />
      ) : status.enabled ? (
        <ReauthForm
          label={hasPassword ? t("auth.passwordCurrent") : t("auth.account.mfa.disable.code")}
          secret={hasPassword}
          action={t("auth.account.mfa.disable")}
          fallback="error.mfa.disable"
          onSubmit={async (value) => {
            await onDisable(
              hasPassword
                ? { currentPassword: value, code: null }
                : { currentPassword: null, code: value },
            );
            setNotice(t("auth.account.mfa.disabled"));
          }}
        />
      ) : (
        <ReauthForm
          label={hasPassword ? t("auth.passwordCurrent") : null}
          secret
          action={t("auth.account.mfa.setup")}
          fallback="error.mfa.setup"
          onSubmit={async (value) => {
            setNotice(null);
            setSetup(await onSetup({ currentPassword: value }));
          }}
        />
      )}
    </div>
  );
}
