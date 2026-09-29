import { t } from "@fvoci/i18n";
import { apiTokenScope } from "@/lib/validators";
import type { z } from "zod";

export type TokenScope = z.infer<typeof apiTokenScope>;

export const TOKEN_SCOPE_LABEL: Record<TokenScope, Parameters<typeof t>[0]> = {
  "documents.read": "token.scope.documents.read",
  "documents.write": "token.scope.documents.write",
  "tasks.read": "token.scope.tasks.read",
  "tasks.write": "token.scope.tasks.write",
  "projects.read": "token.scope.projects.read",
  "projects.manage": "token.scope.projects.manage",
  "share.manage": "token.scope.share.manage",
  "workspace.manage": "token.scope.workspace.manage",
};

export function scopeDomId(scope: TokenScope): string {
  return scope.replaceAll(".", "-");
}

export function formatExpiry(expiresAt: string | null): string {
  if (expiresAt === null) return t("token.unlimited");
  return new Date(expiresAt).toLocaleDateString("ko-KR", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  });
}

export function tokenScopeLabels(scopes: readonly string[]): string {
  return scopes
    .map((scope) => t(TOKEN_SCOPE_LABEL[scope as TokenScope] ?? "token.scopes"))
    .join(", ");
}
