// Adapted from source apps/web/src/features/collections/field-settings.tsx.
import { t } from "@fvoci/i18n";
import { useId, useState } from "react";
import { ConfirmActionButton } from "@/components/confirm-action";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api, ensureOk, ProblemError } from "@/lib/api";
import { fieldTakesOptions, optionPatch, type OptionDraft } from "@/lib/collection-values";
import type { CollectionField } from "@/lib/queries/collections";
import { fieldTypeLabel } from "./field-type-label";

export function FieldSettings({
  workspaceId,
  collectionId,
  field,
  canManage,
  onSaved,
}: {
  workspaceId: string;
  collectionId: string;
  field: CollectionField;
  canManage: boolean;
  onSaved: () => Promise<void>;
}) {
  const baseId = useId();
  const [name, setName] = useState(field.name);
  const [description, setDescription] = useState(field.description ?? "");
  const [options, setOptions] = useState<OptionDraft[]>(() =>
    field.options.map((option) => ({
      id: option.id,
      label: option.label,
      deleted: option.deletedAt !== null,
    })),
  );
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const takesOptions = fieldTakesOptions(field.type);
  const archived = field.deletedAt !== null;
  const disabled = saving || !canManage;

  async function save(deleted: boolean) {
    setSaving(true);
    setError(null);
    try {
      await ensureOk(
        await api.PATCH(
          "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/fields/{field_id}",
          {
            params: {
              path: { workspace_id: workspaceId, collection_id: collectionId, field_id: field.id },
            },
            body: {
              expectedVersion: field.version,
              name: name.trim(),
              description: description.trim() === "" ? null : description.trim(),
              deleted,
              ...(takesOptions ? { options: optionPatch(options) } : {}),
            },
          },
        ),
      );
      await onSaved();
    } catch (err) {
      setError(
        err instanceof ProblemError && err.status === 409
          ? t("collection.saveError")
          : err instanceof ProblemError && err.titleKnown
            ? err.title
            : t("collection.saveError"),
      );
      if (err instanceof ProblemError && err.status === 409) await onSaved();
    } finally {
      setSaving(false);
    }
  }

  return (
    <details className="border-b border-border py-2" data-testid={`field-settings-${field.key}`}>
      <summary className="min-h-11 cursor-pointer py-2 text-ui font-medium">
        {field.name} · {fieldTypeLabel(field.type)}
        {archived ? ` · ${t("collection.archived")}` : ""}
      </summary>
      <div className="flex flex-col gap-3 py-3">
        <div className="collection-field">
          <label htmlFor={`${baseId}-name`}>{t("collection.fieldName")}</label>
          <Input
            id={`${baseId}-name`}
            className="h-9"
            value={name}
            maxLength={100}
            disabled={disabled}
            onChange={(event) => setName(event.target.value)}
          />
        </div>
        <div className="collection-field">
          <label htmlFor={`${baseId}-description`}>{t("project.description")}</label>
          <Input
            id={`${baseId}-description`}
            className="h-9"
            value={description}
            maxLength={1000}
            disabled={disabled}
            onChange={(event) => setDescription(event.target.value)}
          />
        </div>
        <p className="text-dense text-muted-foreground">
          {t("collection.fieldKey")}: <code>{field.key}</code>
        </p>
        {takesOptions ? (
          <fieldset className="flex flex-col gap-2">
            <legend className="text-ui font-medium">{t("collection.optionLabel")}</legend>
            {options.map((option, index) => (
              <div key={option.id ?? `new-${index}`} className="flex flex-wrap items-center gap-2">
                <Input
                  className="h-9 min-w-0 flex-1"
                  aria-label={`${t("collection.optionLabel")} ${index + 1}`}
                  value={option.label}
                  maxLength={100}
                  disabled={disabled}
                  onChange={(event) =>
                    setOptions(
                      options.map((item, i) =>
                        i === index ? { ...item, label: event.target.value } : item,
                      ),
                    )
                  }
                />
                {option.deleted ? (
                  <span className="text-dense text-muted-foreground">{t("collection.archived")}</span>
                ) : null}
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={disabled || index === 0}
                  onClick={() => {
                    const next = [...options];
                    const previous = next[index - 1];
                    if (!previous) return;
                    next[index - 1] = option;
                    next[index] = previous;
                    setOptions(next);
                  }}
                >
                  {t("collection.moveUp")}
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="outline"
                  disabled={disabled}
                  onClick={() =>
                    setOptions(
                      options.map((item, i) =>
                        i === index ? { ...item, deleted: !item.deleted } : item,
                      ),
                    )
                  }
                >
                  {option.deleted ? t("collection.restore") : t("collection.archive")}
                </Button>
              </div>
            ))}
            <Button
              type="button"
              size="sm"
              variant="outline"
              className="w-fit"
              disabled={disabled}
              onClick={() => setOptions([...options, { label: "", deleted: false }])}
            >
              {t("collection.addOption")}
            </Button>
          </fieldset>
        ) : null}
        {canManage ? (
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              size="sm"
              disabled={saving || name.trim() === ""}
              onClick={() => void save(archived)}
            >
              {t("collection.save")}
            </Button>
            {archived ? (
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={saving}
                onClick={() => void save(false)}
              >
                {t("collection.restore")}
              </Button>
            ) : (
              <ConfirmActionButton
                title={t("collection.archive")}
                description={t("collection.archiveDescription")}
                actionLabel={t("collection.archive")}
                disabled={saving}
                onConfirm={() => save(true)}
              >
                {t("collection.archive")}
              </ConfirmActionButton>
            )}
          </div>
        ) : null}
        {error ? (
          <p role="alert" className="text-ui text-destructive">
            {error}
          </p>
        ) : null}
      </div>
    </details>
  );
}
