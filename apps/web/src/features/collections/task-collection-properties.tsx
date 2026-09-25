// Task detail "속성" section: this task's custom field values in its project
// collection (source CollectionPanel on the task page, reduced to one row).
import { t } from "@fvoci/i18n";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { QueryError, QueryLoading, loadErrorMessage } from "@/components/query-status";
import { asCollectionValue, type CollectionValue } from "@/lib/collection-values";
import { FALLBACK_TZ } from "@/lib/datetime";
import { membersQuery, meQuery } from "@/lib/queries";
import {
  collectionFieldsQuery,
  collectionPrefix,
  putCollectionValue,
  taskCollectionItemQuery,
  type CollectionField,
} from "@/lib/queries/collections";
import { ValueEditor } from "./value-editor";
import "./collections.css";
import "@/features/settings/settings-shell.css";

export function TaskCollectionProperties({
  workspaceId,
  taskId,
  readOnly,
}: {
  workspaceId: string;
  taskId: string;
  readOnly: boolean;
}) {
  const queryClient = useQueryClient();
  const lookup = useQuery(taskCollectionItemQuery(workspaceId, taskId));
  const item = lookup.data?.item ?? null;
  const collectionId = item?.collectionId ?? "";
  const fields = useQuery(collectionFieldsQuery(workspaceId, collectionId));
  const members = useQuery(membersQuery(workspaceId));
  const me = useQuery(meQuery);
  const timeZone = me.data?.timezone ?? FALLBACK_TZ;
  const active = (fields.data?.items ?? []).filter((field) => field.deletedAt === null);

  async function save(field: CollectionField, value: CollectionValue) {
    if (!item) return;
    try {
      await putCollectionValue(workspaceId, collectionId, item.id, {
        fieldId: field.id,
        expectedVersion: item.version,
        expectedFieldVersion: field.version,
        value,
      });
    } finally {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: collectionPrefix(workspaceId, collectionId) }),
        queryClient.invalidateQueries({ queryKey: ["collection-item", workspaceId, "task", taskId] }),
      ]);
    }
  }

  if (lookup.isSuccess && lookup.data.item === null) return null;
  if (fields.isSuccess && active.length === 0) return null;

  const error = lookup.error ?? fields.error;
  return (
    <section className="settings-section mt-6" data-testid="task-properties" aria-labelledby="task-properties-title">
      <h2 id="task-properties-title" className="settings-section__title">
        {t("task.properties")}
      </h2>
      {lookup.isPending || fields.isPending ? <QueryLoading /> : null}
      {error ? (
        <QueryError
          message={loadErrorMessage(error)}
          onRetry={() => {
            void lookup.refetch();
            void fields.refetch();
          }}
        />
      ) : null}
      {item && fields.data ? (
        <div className="grid gap-3 sm:grid-cols-2">
          {active.map((field) => (
            <ValueEditor
              key={field.id}
              field={field}
              value={asCollectionValue(
                (lookup.data?.values as Record<string, unknown> | undefined)?.[field.id],
              )}
              members={members.data?.items ?? []}
              timeZone={timeZone}
              readOnly={readOnly || !(lookup.data?.canEdit ?? false)}
              onSave={(value) => save(field, value)}
            />
          ))}
        </div>
      ) : null}
    </section>
  );
}
