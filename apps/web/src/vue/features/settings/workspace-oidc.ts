import { z } from "zod";
import type { components } from "@/generated/api";

type WorkspaceOidcGetOutput = components["schemas"]["WorkspaceOidcGetOutput"];

function isHttpUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return url.protocol === "https:" || url.protocol === "http:";
  } catch {
    return false;
  }
}

/** Same field limits as the React workspace SSO form. */
export const workspaceOidcForm = z.object({
  issuer: z
    .string()
    .trim()
    .min(1, "i18n:form.too_small")
    .max(2048, "i18n:form.too_big")
    .refine(isHttpUrl, { message: "i18n:form.invalid" }),
  clientId: z.string().trim().min(1, "i18n:form.too_small").max(256, "i18n:form.too_big"),
  clientSecret: z.string().min(1, "i18n:form.too_small").max(4096, "i18n:form.too_big"),
  label: z.string().trim().max(100, "i18n:form.too_big"),
});

export function isOidcConfigured(row: WorkspaceOidcGetOutput | null): boolean {
  return row !== null && row.issuer !== null && row.clientId !== null && row.label !== null;
}
