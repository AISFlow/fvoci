// Adapted from source apps/web/src/features/settings/settings-legal.tsx.
import { t } from "@fvoci/i18n";
import { useState } from "react";
import { useForm } from "react-hook-form";
import { z } from "zod";
import { QueryError, QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { components } from "@/generated/api";
import { ProblemError } from "@/lib/api";
import { formatDateKo } from "@/lib/datetime";
import "./settings-shell.css";

type LegalDocument = components["schemas"]["LegalDocumentOutput"];
export type LegalPublishInput = components["schemas"]["LegalPublishBody"];

const KIND_PRESETS = [
  { kind: "terms", label: t("legal.terms") },
  { kind: "privacy", label: t("legal.privacy") },
];

/** Source legalPublishInput; the date field becomes midnight UTC (`Z`) as the server requires. */
export const legalPublishInput = z.object({
  kind: z
    .string()
    .min(1, "i18n:form.too_small")
    .max(50, "i18n:form.too_big")
    .regex(/^[a-z0-9-]+$/, "i18n:form.invalid"),
  title: z.string().trim().min(1, "i18n:form.too_small").max(300, "i18n:form.too_big"),
  bodyMarkdown: z.string().min(1, "i18n:form.too_small").max(200_000, "i18n:form.too_big"),
  required: z.boolean(),
  effectiveAt: z
    .string()
    .regex(/^\d{4}-\d{2}-\d{2}$/, "i18n:form.invalid")
    .transform((date) => `${date}T00:00:00Z`),
});

function issueText(message: string): string {
  return message.startsWith("i18n:") ? t(message.slice(5) as Parameters<typeof t>[0]) : message;
}

const textareaClass =
  "min-h-48 w-full min-w-0 rounded-md border border-input bg-background px-3 py-2 text-ui outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring";

export function SettingsLegalView({
  kind,
  onKindChange,
  current,
  loading,
  error,
  onRetry,
  onPublish,
}: {
  kind: string;
  onKindChange: (kind: string) => void;
  current: LegalDocument | null;
  loading: boolean;
  error: string | null;
  onRetry: () => void;
  onPublish: (input: LegalPublishInput) => Promise<void>;
}) {
  const [serverError, setServerError] = useState<string | null>(null);
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [published, setPublished] = useState(false);
  const empty = { title: "", bodyMarkdown: "", required: true, effectiveAt: "" };
  const form = useForm<{ title: string; bodyMarkdown: string; required: boolean; effectiveAt: string }>({
    defaultValues: empty,
  });

  return (
    <section className="settings-section" aria-labelledby="legal-manage-title">
      <h2 className="settings-section__title text-title" id="legal-manage-title">
        {t("legal.manage")}
      </h2>
      <div className="flex flex-col gap-6">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="legal-kind">{t("legal.kind")}</Label>
          <div className="flex flex-wrap gap-2">
            {KIND_PRESETS.map((p) => (
              <Button key={p.kind} type="button" variant="outline" size="sm" onClick={() => onKindChange(p.kind)}>
                {p.label}({p.kind})
              </Button>
            ))}
          </div>
          <Input id="legal-kind" value={kind} onChange={(e) => onKindChange(e.target.value)} />
        </div>

        <div className="flex flex-col gap-1.5 border-t border-border pt-4">
          <p className="text-ui font-medium">{t("legal.current")}</p>
          {loading ? <QueryLoading /> : null}
          {!loading && error ? <QueryError message={error} onRetry={onRetry} /> : null}
          {!loading && !error && current ? (
            <ul className="text-ui text-muted-foreground" aria-label={t("legal.current")}>
              <li>
                {t("legal.document.title")}: {current.title}
              </li>
              <li>
                {t("legal.document.version")}: v{current.version}
              </li>
              <li>
                {t("legal.effectiveAt")}: {formatDateKo(current.effectiveAt)}
              </li>
              <li>
                {t("legal.requiredFlag")}: {current.required ? t("common.required") : t("common.optional")}
              </li>
            </ul>
          ) : null}
          {!loading && !error && !current ? (
            <p className="text-ui text-muted-foreground">{t("legal.nonePublished")}</p>
          ) : null}
        </div>

        <form
          onSubmit={form.handleSubmit(async (values) => {
            setServerError(null);
            setFieldError(null);
            setPublished(false);
            const parsed = legalPublishInput.safeParse({ kind, ...values });
            if (!parsed.success) {
              setFieldError(issueText(parsed.error.issues[0]?.message ?? "i18n:form.invalid"));
              return;
            }
            try {
              await onPublish(parsed.data);
              form.reset(empty);
              setPublished(true);
            } catch (err) {
              setServerError(err instanceof ProblemError ? err.title : t("legal.publishError"));
            }
          })}
          noValidate
          className="flex flex-col gap-3 border-t border-border pt-4"
        >
          <p className="text-ui font-medium">{t("legal.publishNew")}</p>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="legal-title">{t("legal.document.title")}</Label>
            <Input id="legal-title" {...form.register("title")} />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="legal-body">{t("legal.bodyMarkdown")}</Label>
            <textarea id="legal-body" rows={8} className={textareaClass} {...form.register("bodyMarkdown")} />
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="legal-effective-at">{t("legal.effectiveAt")}</Label>
            <Input id="legal-effective-at" type="date" {...form.register("effectiveAt")} />
          </div>
          <div className="flex min-h-11 items-center gap-2">
            <input id="legal-required" type="checkbox" className="size-5" {...form.register("required")} />
            <Label htmlFor="legal-required" className="font-normal">
              {t("legal.requiredDoc")}
            </Label>
          </div>
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
          {published ? (
            <p role="status" className="text-ui text-muted-foreground">
              {t("legal.published")}
            </p>
          ) : null}
          <Button type="submit" size="sm" className="w-fit" disabled={form.formState.isSubmitting}>
            {form.formState.isSubmitting ? t("form.publishing") : t("legal.publish")}
          </Button>
        </form>
      </div>
    </section>
  );
}
