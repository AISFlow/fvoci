// Project task list filter/sort bar over the shared view query (source
// features/tasks/task-filter-bar + collections/native-filters, native controls).
import { formatPersonName, t } from "@fvoci/i18n";
import { useEffect, useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { CustomFilters } from "@/features/collections/custom-filters";
import type { WorkflowStatus } from "@/features/projects/queries";
import { SORTABLE_FIELD_TYPES } from "@/lib/collection-values";
import type { MemberOutput } from "@/lib/contracts";
import type { CollectionField } from "@/lib/queries/collections";
import {
  isEmptyViewQuery,
  patchViewFilter,
  readPrimarySort,
  setPrimarySort,
  type ViewQuery,
} from "@/lib/view-query";
import { PRIORITIES, priorityLabel } from "./task-edit-payload";
import type { LabelItem, MilestoneItem } from "./queries";
import { TASK_TYPES, TASK_TYPE_LABELS } from "./task-types";
import "@/features/collections/collections.css";

export function TaskFilters({
  query,
  statuses,
  labels,
  milestones,
  members,
  fields,
  timeZone,
  onChange,
}: {
  query: ViewQuery;
  statuses: readonly WorkflowStatus[];
  labels: readonly LabelItem[];
  milestones: readonly MilestoneItem[];
  members: readonly MemberOutput[];
  fields: readonly CollectionField[];
  timeZone: string;
  onChange: (next: ViewQuery) => void;
}) {
  const id = useId();
  const filters = query.filters;
  const [title, setTitle] = useState(filters.title ?? "");
  const [customOpen, setCustomOpen] = useState((filters.custom?.length ?? 0) > 0);
  useEffect(() => setTitle(filters.title ?? ""), [filters.title]);

  const activeFields = fields.filter((field) => field.deletedAt === null);
  const sortItems: Array<{ id: string; name: string }> = [
    { id: "created", name: t("collection.created") },
    { id: "title", name: t("collection.resourceTitle") },
    { id: "status", name: t("collection.status") },
    { id: "due", name: t("collection.due") },
    { id: "priority", name: t("task.priority") },
    ...activeFields
      .filter((field) => SORTABLE_FIELD_TYPES.includes(field.type))
      .map((field) => ({ id: field.id, name: field.name })),
  ];
  const primary = readPrimarySort(query);
  const simple = primary && sortItems.some((item) => item.id === primary.field) ? primary : null;
  const sortValue = simple?.field ?? (query.sort.length > 0 ? "advanced" : "default");

  function select(
    key: "type" | "statusId" | "priority" | "assigneeId" | "labelId" | "milestoneId",
    label: string,
    items: Array<{ id: string; name: string }>,
  ) {
    return (
      <div className="collection-field">
        <label htmlFor={`${id}-${key}`}>{label}</label>
        <select
          id={`${id}-${key}`}
          className="collection-select"
          data-testid={`task-filter-${key}`}
          value={filters[key] ?? ""}
          onChange={(event) => onChange(patchViewFilter(query, key, event.target.value || undefined))}
        >
          <option value="">{t("task.filter.all")}</option>
          {items.map((item) => (
            <option key={item.id} value={item.id}>
              {item.name}
            </option>
          ))}
        </select>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-2" role="search" aria-label={t("view.filter")}>
      <div className="collection-toolbar">
        <form
          className="collection-field"
          onSubmit={(event) => {
            event.preventDefault();
            onChange(patchViewFilter(query, "title", title));
          }}
        >
          <label htmlFor={`${id}-title`}>{t("gantt.search")}</label>
          <Input
            id={`${id}-title`}
            className="h-9 w-48"
            type="search"
            data-testid="task-filter-title"
            value={title}
            maxLength={1000}
            onChange={(event) => setTitle(event.target.value)}
            onBlur={() => {
              if (title.trim() !== (filters.title ?? "")) {
                onChange(patchViewFilter(query, "title", title));
              }
            }}
          />
        </form>
        {select(
          "type",
          t("task.filter.typeShort"),
          TASK_TYPES.map((type) => ({ id: type, name: TASK_TYPE_LABELS[type] })),
        )}
        {select(
          "statusId",
          t("task.filter.statusShort"),
          statuses.map((status) => ({ id: status.id, name: status.name })),
        )}
        {select(
          "priority",
          t("task.filter.priorityShort"),
          PRIORITIES.map((priority) => ({ id: priority, name: priorityLabel(priority) })),
        )}
        {select("assigneeId", t("task.filter.assigneeShort"), [
          { id: "me", name: t("task.filter.me") },
          ...members.map((member) => ({ id: member.userId, name: formatPersonName(member) })),
        ])}
        {labels.length > 0
          ? select(
              "labelId",
              t("task.filter.labelsShort"),
              labels.map((label) => ({ id: label.id, name: label.name })),
            )
          : null}
        {milestones.length > 0
          ? select(
              "milestoneId",
              t("task.filter.milestoneShort"),
              milestones.map((milestone) => ({ id: milestone.id, name: milestone.name })),
            )
          : null}
        <div className="collection-field">
          <label htmlFor={`${id}-due`}>{t("task.filter.dueBefore")}</label>
          <Input
            id={`${id}-due`}
            className="h-9"
            type="date"
            value={filters.dueBefore ?? ""}
            onChange={(event) =>
              onChange(patchViewFilter(query, "dueBefore", event.target.value || undefined))
            }
          />
        </div>
        <label className="flex min-h-9 items-center gap-2 text-ui" htmlFor={`${id}-open`}>
          <input
            id={`${id}-open`}
            type="checkbox"
            data-testid="task-filter-openOnly"
            checked={filters.openOnly === true}
            onChange={(event) => onChange(patchViewFilter(query, "openOnly", event.target.checked))}
          />
          {t("task.filter.openOnly")}
        </label>
      </div>
      <div className="collection-toolbar">
        <div className="collection-field">
          <label htmlFor={`${id}-sort`}>{t("collection.sort")}</label>
          <select
            id={`${id}-sort`}
            className="collection-select"
            data-testid="task-sort"
            value={sortValue}
            onChange={(event) => {
              const value = event.target.value;
              onChange(
                setPrimarySort(
                  query,
                  value === "default" || value === "advanced" ? null : value,
                  simple?.direction ?? "asc",
                ),
              );
            }}
          >
            <option value="default">{t("collection.sort.default")}</option>
            {sortValue === "advanced" ? (
              <option value="advanced">{t("collection.sort.advanced")}</option>
            ) : null}
            {sortItems.map((item) => (
              <option key={item.id} value={item.id}>
                {item.name}
              </option>
            ))}
          </select>
        </div>
        <Button
          type="button"
          size="sm"
          variant="outline"
          disabled={!simple}
          onClick={() => {
            if (!simple) return;
            onChange(
              setPrimarySort(query, simple.field, simple.direction === "asc" ? "desc" : "asc"),
            );
          }}
        >
          {simple?.direction === "desc" ? t("collection.descending") : t("collection.ascending")}
        </Button>
        {activeFields.length > 0 ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            aria-expanded={customOpen}
            onClick={() => setCustomOpen((open) => !open)}
          >
            {t("collection.filter.custom")}
          </Button>
        ) : null}
        {!isEmptyViewQuery(query) ? (
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={() => onChange({ filters: {}, sort: [] })}
          >
            {t("task.filter.clear")}
          </Button>
        ) : null}
      </div>
      {customOpen && activeFields.length > 0 ? (
        <CustomFilters
          fields={activeFields}
          members={members}
          timeZone={timeZone}
          query={query}
          onQueryChange={onChange}
        />
      ) : null}
    </div>
  );
}
