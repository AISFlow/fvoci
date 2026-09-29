import type { FieldError } from "react-hook-form";
import { issueMessage } from "@/lib/issue-message";

export function formFieldMessage(
  error: FieldError | undefined,
  _field: string,
): string | null {
  if (!error?.message) return null;
  return issueMessage(error.message);
}
