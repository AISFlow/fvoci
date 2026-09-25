// Adapted from source apps/web/src/features/collections/value-editor.tsx (native controls).
import { formatPersonName, t } from "@fvoci/i18n";
import { useEffect, useId, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  draftFromValue,
  selectedOptionIds,
  selectedUserIds,
  valueFromDraft,
  type CollectionValue,
} from "@/lib/collection-values";
import type { MemberOutput } from "@/lib/contracts";
import type { CollectionField } from "@/lib/queries/collections";

export function ValueEditor({
  field,
  value,
  members,
  timeZone,
  readOnly,
  showLabel = true,
  labelSuffix = "",
  onSave,
}: {
  field: CollectionField;
  value: CollectionValue;
  members: readonly MemberOutput[];
  timeZone: string;
  readOnly: boolean;
  showLabel?: boolean;
  /** Appended to the accessible name (e.g. the row title in a table). */
  labelSuffix?: string;
  onSave: (value: CollectionValue) => Promise<void>;
}) {
  const controlId = useId();
  const text = draftFromValue(value, timeZone);
  const [draft, setDraft] = useState(text);
  useEffect(() => setDraft(text), [text]);
  const [error, setError] = useState(false);
  const [saving, setSaving] = useState(false);
  const disabled = readOnly || saving || field.deletedAt !== null;
  const selectedOptions = selectedOptionIds(value);
  const selectedUsers = selectedUserIds(value);
  const accessibleName = labelSuffix ? `${field.name} · ${labelSuffix}` : field.name;

  async function save(next: CollectionValue) {
    setSaving(true);
    setError(false);
    try {
      await onSave(next);
    } catch {
      setError(true);
    } finally {
      setSaving(false);
    }
  }

  let control: ReactNode;
  if (field.type === "checkbox") {
    control = (
      <input
        id={controlId}
        type="checkbox"
        className="size-5"
        aria-label={accessibleName}
        disabled={disabled}
        checked={value !== null && "checkbox" in value && value.checkbox}
        onChange={(event) => void save({ checkbox: event.target.checked })}
      />
    );
  } else if (field.type === "select" || field.type === "user") {
    const people = field.type === "user";
    const current = people ? (selectedUsers[0] ?? "") : (selectedOptions[0] ?? "");
    control = (
      <select
        id={controlId}
        className="collection-select"
        aria-label={accessibleName}
        disabled={disabled}
        value={current}
        onChange={(event) => {
          const id = event.target.value;
          void save(id === "" ? null : people ? { users: [id] } : { options: [id] });
        }}
      >
        <option value="">{t("collection.unassigned")}</option>
        {people
          ? members.map((member) => (
              <option key={member.userId} value={member.userId}>
                {formatPersonName(member)}
              </option>
            ))
          : field.options.map((option) => (
              <option
                key={option.id}
                value={option.id}
                disabled={option.deletedAt !== null && option.id !== current}
              >
                {option.label}
                {option.deletedAt ? ` · ${t("collection.archived")}` : ""}
              </option>
            ))}
      </select>
    );
  } else if (["multi_select", "checkboxes", "labels", "user_multi"].includes(field.type)) {
    const people = field.type === "user_multi";
    const selected = people ? selectedUsers : selectedOptions;
    const choices = people
      ? members.map((member) => ({ id: member.userId, label: formatPersonName(member), deleted: false }))
      : field.options.map((option) => ({
          id: option.id,
          label: option.label,
          deleted: option.deletedAt !== null,
        }));
    control = (
      <fieldset className="flex flex-wrap gap-x-3 gap-y-1" aria-label={accessibleName}>
        {choices.map((choice) => {
          const checked = selected.includes(choice.id);
          return (
            <label
              key={choice.id}
              htmlFor={`${controlId}-${choice.id}`}
              className="flex min-h-8 items-center gap-1.5 text-ui"
            >
              <input
                id={`${controlId}-${choice.id}`}
                type="checkbox"
                disabled={disabled || (choice.deleted && !checked)}
                checked={checked}
                onChange={(event) => {
                  const ids = event.target.checked
                    ? [...selected, choice.id]
                    : selected.filter((id) => id !== choice.id);
                  void save(ids.length === 0 ? null : people ? { users: ids } : { options: ids });
                }}
              />
              {choice.label}
              {choice.deleted ? ` · ${t("collection.archived")}` : ""}
            </label>
          );
        })}
      </fieldset>
    );
  } else {
    control = (
      <form
        className="flex flex-wrap items-center gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          if (disabled || draft === text) return;
          const next = valueFromDraft(field.type, draft, timeZone);
          if (next === "invalid") {
            setError(true);
            return;
          }
          void save(next);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter" && event.nativeEvent.isComposing) event.preventDefault();
        }}
      >
        {field.type === "paragraph" ? (
          <textarea
            id={controlId}
            className="collection-textarea"
            aria-label={accessibleName}
            disabled={disabled}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
        ) : (
          <Input
            id={controlId}
            className="h-9 min-w-0 flex-1"
            aria-label={accessibleName}
            disabled={disabled}
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
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
          />
        )}
        {!readOnly ? (
          <Button type="submit" size="sm" variant="outline" disabled={disabled || draft === text}>
            {t("collection.save")}
          </Button>
        ) : null}
      </form>
    );
  }

  return (
    <div className="flex min-w-0 flex-col gap-1" data-testid={`value-editor-${field.key}`}>
      {showLabel ? (
        <span className="text-dense text-muted-foreground">
          {field.name}
          {field.type === "datetime" ? ` · ${timeZone}` : ""}
          {field.description ? ` — ${field.description}` : ""}
        </span>
      ) : null}
      {control}
      {error ? (
        <p role="alert" className="text-dense text-destructive">
          {t("collection.saveError")}
        </p>
      ) : null}
    </div>
  );
}
