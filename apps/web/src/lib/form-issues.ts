import { issueMessage } from "@/lib/issue-message";

export function formFieldMessage(
  error: { message?: string } | undefined,
  _field: string,
): string | null {
  if (!error?.message) return null;
  return issueMessage(error.message);
}
