// Adapted from source apps/web/src/features/auth/consent.tsx.
import { asSafeHtml, SafeHtmlView } from "@fvoci/editor/safe-html";
import { t } from "@fvoci/i18n";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import type { components } from "@/generated/api";
import { problemMessage } from "@/lib/api";
import { formatDateKo } from "@/lib/datetime";
import { AuthAlert, AuthStatus, authPrimaryButtonClass } from "./auth-form";
import { AuthLayout, AuthPanel } from "./auth-layout";

type LegalDocument = components["schemas"]["LegalDocumentOutput"];

export function ConsentView({
  pending,
  onSubmit,
}: {
  pending: LegalDocument[];
  onSubmit: (items: { kind: string; version: number }[]) => Promise<void>;
}) {
  const [checked, setChecked] = useState<Record<string, boolean>>({});
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const allChecked = pending.length > 0 && pending.every((d) => checked[`${d.kind}:${d.version}`] === true);

  if (pending.length === 0) {
    return (
      <AuthLayout>
        <AuthPanel title={t("consent.title")}>
          <AuthStatus>{t("consent.empty")}</AuthStatus>
        </AuthPanel>
      </AuthLayout>
    );
  }

  return (
    <AuthLayout>
      <AuthPanel title={t("consent.title")} lead={t("consent.hint")}>
        {pending.map((doc) => {
          const key = `${doc.kind}:${doc.version}`;
          return (
            <div key={key} className="auth-shell__stack border-b border-border pb-6 last:border-b-0 last:pb-0">
              <div>
                <h2 className="text-ui font-medium text-foreground">{doc.title}</h2>
                <p className="mt-1 text-dense text-muted-foreground">
                  {t("consent.effectiveAt", { date: formatDateKo(doc.effectiveAt) })}
                </p>
              </div>
              {/* bodyHtml is rendered server side from markdown by the sanitizing convert helper. */}
              <SafeHtmlView className="auth-shell__doc-body break-keep text-ui" html={asSafeHtml(doc.bodyHtml)} />
              <div className="flex min-h-11 items-start gap-3">
                <input
                  id={`consent-${key}`}
                  type="checkbox"
                  className="mt-1 size-5"
                  checked={checked[key] ?? false}
                  onChange={(event) => setChecked((c) => ({ ...c, [key]: event.target.checked }))}
                  aria-describedby={`consent-${key}-title`}
                />
                <label htmlFor={`consent-${key}`} className="text-ui text-foreground">
                  {t("consent.agree")}
                  <span id={`consent-${key}-title`} className="sr-only">
                    {doc.title}
                  </span>
                </label>
              </div>
            </div>
          );
        })}
        {error ? <AuthAlert>{error}</AuthAlert> : null}
        <form
          onSubmit={(event) => {
            event.preventDefault();
            if (!allChecked || submitting) return;
            setError(null);
            setSubmitting(true);
            onSubmit(pending.map((d) => ({ kind: d.kind, version: d.version })))
              .catch((err: unknown) => {
                setError(problemMessage(err, "error.auth.consent"));
              })
              .finally(() => setSubmitting(false));
          }}
        >
          <Button
            type="submit"
            size="lg"
            disabled={!allChecked || submitting}
            className={authPrimaryButtonClass}
          >
            {submitting ? t("form.submitting") : t("consent.submit")}
          </Button>
        </form>
      </AuthPanel>
    </AuthLayout>
  );
}
