// Adapted from source apps/web/src/features/settings/settings-audit.tsx.
import { t } from "@fvoci/i18n";
import { QueryLoading } from "@/components/query-status";
import type { components } from "@/generated/api";
import { formatInstant } from "@/lib/datetime";
import "./settings-shell.css";

type AuditLogItem = components["schemas"]["AuditLogItemOutput"];

const cellClass = "border-b border-border px-2 py-2 align-top text-ui";

export function AuditSettingsView({
  items,
  timeZone,
  loading,
  eeRequired,
  error,
}: {
  items: AuditLogItem[];
  timeZone: string;
  loading: boolean;
  eeRequired: boolean;
  error: string | null;
}) {
  return (
    <div className="settings-stack">
      <section className="settings-section" aria-labelledby="audit-title">
        <h2 className="settings-section__title text-title" id="audit-title">
          {t("audit.title")}
        </h2>
        <div className="flex flex-col gap-4">
          {eeRequired ? <p className="text-ui text-muted-foreground">{t("ee.required")}</p> : null}
          {error ? (
            <p className="text-ui text-destructive" role="alert">
              {error}
            </p>
          ) : null}
          {loading ? <QueryLoading /> : null}
          {!eeRequired && !loading && !error && items.length === 0 ? (
            <p className="text-ui text-muted-foreground">{t("audit.empty")}</p>
          ) : null}
          {!eeRequired && !loading && items.length > 0 ? (
            <div className="overflow-x-auto">
              <table className="w-full border-collapse">
                <tbody>
                  {items.map((row) => (
                    <tr key={row.id}>
                      <td className={`${cellClass} font-mono`}>{row.verb}</td>
                      <td className={`${cellClass} settings-tabular`}>
                        {formatInstant(row.createdAt, timeZone, {
                          year: "numeric",
                          month: "2-digit",
                          day: "2-digit",
                          hour: "2-digit",
                          minute: "2-digit",
                        })}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : null}
        </div>
      </section>
    </div>
  );
}
