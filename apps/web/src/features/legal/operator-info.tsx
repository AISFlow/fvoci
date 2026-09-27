import { t } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import { AuthStatus } from "@/features/auth/auth-form";
import { AuthLayout, AuthPanel } from "@/features/auth/auth-layout";
import {
  filledOperatorFields,
  hasOperatorInfo,
  operatorFieldHref,
  type OperatorInfo,
} from "./operator-fields";

export type { OperatorInfo } from "./operator-fields";
export {
  filledOperatorFields,
  hasOperatorInfo,
  operatorFieldHref,
} from "./operator-fields";

function OperatorInfoList({ operator }: { operator: OperatorInfo | null | undefined }) {
  const rows = filledOperatorFields(operator);
  if (rows.length === 0) return null;
  return (
    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-3 break-keep text-ui text-foreground">
      {rows.map((field) => {
        const value = operator?.[field] ?? "";
        const href = operatorFieldHref(field, value);
        return (
          <div className="contents" key={field}>
            <dt className="text-muted-foreground">{t(`operator.${field}`)}</dt>
            <dd className="min-w-0 break-words">
              {href === null ? (
                value
              ) : (
                <a
                  className="auth-shell__link break-all"
                  href={href}
                  rel="noreferrer"
                  target={field === "businessInfoUrl" ? "_blank" : undefined}
                >
                  {value}
                </a>
              )}
            </dd>
          </div>
        );
      })}
    </dl>
  );
}

export function OperatorInfoView({ operator }: { operator: OperatorInfo | null }) {
  return (
    <AuthLayout width="wide" showWordmark={false}>
      <AuthPanel title={t("operator.title")}>
        {hasOperatorInfo(operator) ? (
          <OperatorInfoList operator={operator} />
        ) : (
          <AuthStatus>{t("operator.empty")}</AuthStatus>
        )}
      </AuthPanel>
    </AuthLayout>
  );
}

export const LEGAL_DOCS = [
  { kind: "terms", label: t("legal.terms") },
  { kind: "privacy", label: t("legal.privacy") },
] as const;

export function ServiceInfoFooter({
  operator,
}: {
  operator: OperatorInfo | null | undefined;
}) {
  return (
    <footer className="auth-shell__footer">
      {hasOperatorInfo(operator) ? (
        <Link className="auth-shell__footer-link" to="/service-info">
          {t("operator.title")}
        </Link>
      ) : null}
      {LEGAL_DOCS.map((doc) => (
        <Link className="auth-shell__footer-link" to={`/legal/${doc.kind}`} key={doc.kind}>
          {doc.label}
        </Link>
      ))}
      {import.meta.env.PROD ? (
        <a className="auth-shell__footer-link" href="/open-source-licenses.txt">
          {t("legal.openSourceNotices")}
        </a>
      ) : null}
    </footer>
  );
}

/** Authenticated shell: same legal bundle as the source account menu (service info + policies). */
export function AuthenticatedLegalNav({ className }: { className?: string }) {
  return (
    <nav
      className={className ?? "flex flex-wrap items-center gap-x-3 gap-y-1 text-caption text-muted-foreground"}
      aria-label={t("operator.title")}
    >
      <Link to="/service-info" className="underline underline-offset-2 hover:text-foreground">
        {t("operator.title")}
      </Link>
      {LEGAL_DOCS.map((doc) => (
        <Link
          key={doc.kind}
          to={`/legal/${doc.kind}`}
          className="underline underline-offset-2 hover:text-foreground"
        >
          {doc.label}
        </Link>
      ))}
    </nav>
  );
}
