import type { FieldError } from "react-hook-form";
import { t } from "@fvoci/i18n";

export function formFieldMessage(
  error: FieldError | undefined,
  _field: string,
): string | null {
  if (!error?.message) return null;
  if (error.message.startsWith("i18n:")) {
    const key = error.message.slice(5);
    return t(key as Parameters<typeof t>[0]);
  }
  return error.message;
}
