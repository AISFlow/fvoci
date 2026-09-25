// Adapted from source apps/web/src/features/collections/collection-custom-filters.tsx.
import { formatPersonName, t } from "@fvoci/i18n";
import { useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  customEqualsValue,
  isoToZonedLocal,
  SET_FIELD_TYPES,
} from "@/lib/collection-values";
import type { MemberOutput } from "@/lib/contracts";
import type { CollectionField } from "@/lib/queries/collections";
import { addCustomFilter, removeCustomFilter, type CustomFilter, type ViewQuery } from "@/lib/view-query";

function describeFilter(
  filter: CustomFilter,
  fields: readonly CollectionField[],
  members: readonly MemberOutput[],
  timeZone: string,
): { name: string; text: string } {
  const field = fields.find((item) => item.id === filter.fieldId);
  const name = field?.name ?? filter.fieldId;
  if (filter.operator === "empty") return { name, text: `${name}: ${t("collection.filter.empty")}` };
  let label = String(filter.value);
  if (field && typeof filter.value === "string") {
    const option = field.options.find((item) => item.id === filter.value);
    const member = members.find((item) => item.userId === filter.value);
    if (option) label = option.label;
    else if (member) label = formatPersonName(member);
    else if (field.type === "datetime") label = isoToZonedLocal(filter.value, timeZone).replace("T", " ");
  }
  if (typeof filter.value === "boolean") {
    label = filter.value ? t("collection.filter.true") : t("collection.filter.false");
  }
  return { name, text: `${name} ${t("collection.filter.equals")} ${label}` };
}

export function CustomFilters({
  fields,
  members,
  timeZone,
  query,
  onQueryChange,
}: {
  fields: readonly CollectionField[];
  members: readonly MemberOutput[];
  timeZone: string;
  query: ViewQuery;
  onQueryChange: (next: ViewQuery) => void;
}) {
  const baseId = useId();
  const [fieldId, setFieldId] = useState("");
  const [operator, setOperator] = useState<"equals" | "empty">("equals");
  const [raw, setRaw] = useState("");
  const field = fields.find((item) => item.id === fieldId);
  const current = query.filters.custom ?? [];
  const value = field && operator === "equals" ? customEqualsValue(field.type, raw, timeZone) : null;
  const ready = Boolean(field) && (operator === "empty" || value !== null);
  const people = field?.type === "user" || field?.type === "user_multi";

  return (
    <div className="flex flex-col gap-2" data-testid="custom-filters">
      {current.length > 0 ? (
        <ul className="flex flex-wrap gap-2">
          {current.map((filter, index) => {
            const described = describeFilter(filter, fields, members, timeZone);
            return (
              <li key={`${filter.fieldId}:${index}`} className="tag-chip" data-color="blue">
                {described.text}
                <button
                  type="button"
                  className="tags-bar__remove"
                  aria-label={t("collection.filter.remove", { name: described.name })}
                  onClick={() => onQueryChange(removeCustomFilter(query, index))}
                >
                  <span aria-hidden="true">×</span>
                </button>
              </li>
            );
          })}
        </ul>
      ) : null}
      <form
        className="collection-toolbar"
        onSubmit={(event) => {
          event.preventDefault();
          if (!field || !ready) return;
          const filter: CustomFilter =
            operator === "empty"
              ? { fieldId: field.id, operator: "empty" }
              : { fieldId: field.id, operator: "equals", value: value as string | number | boolean };
          onQueryChange(addCustomFilter(query, filter));
          setRaw("");
        }}
      >
        <div className="collection-field">
          <label htmlFor={`${baseId}-field`}>{t("collection.filter.field")}</label>
          <select
            id={`${baseId}-field`}
            className="collection-select"
            value={fieldId}
            onChange={(event) => {
              setFieldId(event.target.value);
              setRaw("");
            }}
          >
            <option value="">{t("collection.none")}</option>
            {fields.map((item) => (
              <option key={item.id} value={item.id}>
                {item.name}
              </option>
            ))}
          </select>
        </div>
        <div className="collection-field">
          <label htmlFor={`${baseId}-operator`}>{t("collection.filter.operator")}</label>
          <select
            id={`${baseId}-operator`}
            className="collection-select"
            value={operator}
            onChange={(event) => setOperator(event.target.value === "empty" ? "empty" : "equals")}
          >
            <option value="equals">{t("collection.filter.equals")}</option>
            <option value="empty">{t("collection.filter.empty")}</option>
          </select>
        </div>
        {field && operator === "equals" ? (
          <div className="collection-field">
            <label htmlFor={`${baseId}-value`}>{t("collection.filter.value")}</label>
            {field.type === "checkbox" ? (
              <select
                id={`${baseId}-value`}
                className="collection-select"
                value={raw}
                onChange={(event) => setRaw(event.target.value)}
              >
                <option value="">{t("collection.none")}</option>
                <option value="true">{t("collection.filter.true")}</option>
                <option value="false">{t("collection.filter.false")}</option>
              </select>
            ) : SET_FIELD_TYPES.includes(field.type) ? (
              <select
                id={`${baseId}-value`}
                className="collection-select"
                value={raw}
                onChange={(event) => setRaw(event.target.value)}
              >
                <option value="">{t("collection.none")}</option>
                {people
                  ? members.map((member) => (
                      <option key={member.userId} value={member.userId}>
                        {formatPersonName(member)}
                      </option>
                    ))
                  : field.options
                      .filter((option) => option.deletedAt === null)
                      .map((option) => (
                        <option key={option.id} value={option.id}>
                          {option.label}
                        </option>
                      ))}
              </select>
            ) : (
              <Input
                id={`${baseId}-value`}
                className="h-9"
                type={
                  field.type === "number"
                    ? "number"
                    : field.type === "date"
                      ? "date"
                      : field.type === "datetime"
                        ? "datetime-local"
                        : "text"
                }
                step="any"
                value={raw}
                onChange={(event) => setRaw(event.target.value)}
              />
            )}
          </div>
        ) : null}
        <Button type="submit" size="sm" variant="outline" disabled={!ready}>
          {t("collection.filter.add")}
        </Button>
      </form>
    </div>
  );
}
