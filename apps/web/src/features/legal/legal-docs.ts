import { t } from "@fvoci/i18n";

/** The policy pages every footer links to (`/legal/:kind`), framework-neutral for both web apps. */
export const LEGAL_DOCS = [
  { kind: "terms", label: t("legal.terms") },
  { kind: "privacy", label: t("legal.privacy") },
] as const;
