// Adapted from source apps/web/src/features/settings/settings-legal.tsx: the
// legal document publish form's rules, framework-neutral.
import { t } from "@fvoci/i18n";
import { z } from "zod";

/** The kinds offered as one-click choices; any other valid kind can be typed. */
export const LEGAL_KIND_PRESETS = [
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
