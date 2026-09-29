import { t } from "@fvoci/i18n";
import type { components } from "@/generated/api";
import { isSafeShareHref } from "@/lib/share-links";

export type OperatorInfo = components["schemas"]["OperatorSettings"];

const FIELDS = [
  "businessName",
  "representative",
  "registrationNumber",
  "mailOrderNumber",
  "address",
  "phone",
  "supportEmail",
  "businessInfoUrl",
  "hostingProvider",
] as const;

export type OperatorField = (typeof FIELDS)[number];

export function filledOperatorFields(operator: OperatorInfo | null | undefined): OperatorField[] {
  return FIELDS.filter((field) => operator?.[field] != null);
}

export function hasOperatorInfo(operator: OperatorInfo | null | undefined): boolean {
  return filledOperatorFields(operator).length > 0;
}

/** Safe href for operator link fields; plain text fields return null. */
export function operatorFieldHref(field: OperatorField, value: string): string | null {
  if (field === "supportEmail") {
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(value)) return null;
    const href = `mailto:${value}`;
    return isSafeShareHref(href) ? href : null;
  }
  if (field === "businessInfoUrl") {
    return isSafeShareHref(value) && /^https?:/i.test(value) ? value : null;
  }
  return null;
}

export { FIELDS as OPERATOR_FIELDS };

/** The public legal documents every sign-in page and signed-in shell links to. */
export const LEGAL_DOCS = [
  { kind: "terms", label: t("legal.terms") },
  { kind: "privacy", label: t("legal.privacy") },
] as const;
