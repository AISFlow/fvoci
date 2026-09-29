import { t } from "@fvoci/i18n";

/**
 * A validation message as shown: `i18n:<key>` (the form schemas' messages,
 * lib/validators.ts) is that catalog string, anything else is shown as is.
 */
export function issueMessage(message: string): string {
  return message.startsWith("i18n:") ? t(message.slice(5) as Parameters<typeof t>[0]) : message;
}
