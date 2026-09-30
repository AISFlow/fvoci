import { t } from "@fvoci/i18n";
import { Link } from "react-router-dom";
import {
  hasOperatorInfo,
  LEGAL_DOCS,
  type OperatorInfo,
} from "./operator-fields";

export type { OperatorInfo } from "./operator-fields";
export {
  filledOperatorFields,
  operatorFieldHref,
  hasOperatorInfo,
  LEGAL_DOCS,
} from "./operator-fields";

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
