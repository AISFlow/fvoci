// Adapted from source apps/web/src/routes/legal.$kind.tsx and features/legal/legal.tsx.
import { asSafeHtml, SafeHtmlView } from "@fvoci/editor/safe-html";
import { t } from "@fvoci/i18n";
import { useQuery } from "@tanstack/react-query";
import { useParams, useSearchParams } from "react-router-dom";
import { loadErrorMessage, QueryError, QueryLoading } from "@/components/query-status";
import { AuthAlert } from "@/features/auth/auth-form";
import { AuthLayout, AuthPanel } from "@/features/auth/auth-layout";
import { ProblemError } from "@/lib/api";
import { formatDateKo } from "@/lib/datetime";
import { legalDocQuery, legalVersionsQuery } from "@/lib/queries/admin";

export function LegalPage() {
  const { kind = "" } = useParams<{ kind: string }>();
  const [params] = useSearchParams();
  const raw = params.get("version");
  const version = raw && /^\d+$/.test(raw) ? Number(raw) : undefined;
  const doc = useQuery(legalDocQuery(kind, version));
  const versions = useQuery(legalVersionsQuery(kind));

  if (doc.error instanceof ProblemError && doc.error.status === 404) {
    return (
      <AuthLayout>
        <AuthPanel title={kind}>
          <AuthAlert>{t("legal.empty")}</AuthAlert>
        </AuthPanel>
      </AuthLayout>
    );
  }
  if (doc.data === undefined) {
    return (
      <AuthLayout>
        {doc.isError ? (
          <QueryError message={loadErrorMessage(doc.error)} onRetry={() => void doc.refetch()} />
        ) : (
          <QueryLoading />
        )}
      </AuthLayout>
    );
  }

  const current = doc.data;
  const others = (versions.data ?? []).filter((v) => v.version !== current.version);

  return (
    <AuthLayout>
      <AuthPanel
        title={current.title}
        lead={t("legal.meta", { version: current.version, date: formatDateKo(current.effectiveAt) })}
      >
        <SafeHtmlView className="break-keep text-ui text-foreground" html={asSafeHtml(current.bodyHtml)} />
        {others.length > 0 ? (
          <div className="auth-shell__stack border-t border-border pt-5">
            <p className="text-ui font-medium text-foreground">{t("legal.previous")}</p>
            {others.map((v) => (
              <a key={v.version} href={`/legal/${kind}?version=${v.version}`} className="auth-shell__link break-keep">
                v{v.version}({formatDateKo(v.effectiveAt)})
              </a>
            ))}
          </div>
        ) : null}
      </AuthPanel>
    </AuthLayout>
  );
}
