import { isI18nKey, t } from "@fvoci/i18n";
import type { z } from "zod";

function issueText(message: string | undefined): string {
  if (!message) return t("form.invalid");
  if (message.startsWith("i18n:")) {
    const key = message.slice(5);
    return isI18nKey(key) ? t(key) : message;
  }
  return message;
}

/** Client-side field check with the shared Zod schemas (no react-hook-form). */
export function parseForm<T>(
  schema: z.ZodType<T>,
  value: unknown,
): { ok: true; data: T } | { ok: false; message: string } {
  const result = schema.safeParse(value);
  if (result.success) return { ok: true, data: result.data };
  return { ok: false, message: issueText(result.error.issues[0]?.message) };
}

export function fieldIssue(schema: z.ZodType, value: unknown, field: string): string | null {
  const result = schema.safeParse(value);
  if (result.success) return null;
  const issue = result.error.issues.find((item) => item.path[0] === field) ?? result.error.issues[0];
  return issue ? issueText(issue.message) : null;
}
