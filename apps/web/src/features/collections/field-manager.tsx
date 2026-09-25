// Adapted from source apps/web/src/features/collections/collection-field-manager.tsx.
import { t } from "@fvoci/i18n";
import { useMutation } from "@tanstack/react-query";
import { useId, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { api, ensureOk, ProblemError } from "@/lib/api";
import {
  FIELD_KEY_PATTERN,
  FIELD_TYPES,
  fieldTakesOptions,
  parseOptionLines,
  suggestedFieldKey,
  type FieldType,
} from "@/lib/collection-values";
import type { CollectionField } from "@/lib/queries/collections";
import { FieldSettings } from "./field-settings";
import { fieldTypeLabel } from "./field-type-label";
import "./collections.css";

export function CollectionFieldManager({
  workspaceId,
  collectionId,
  fields,
  canManage,
  onSaved,
}: {
  workspaceId: string;
  collectionId: string;
  fields: readonly CollectionField[];
  canManage: boolean;
  onSaved: () => Promise<void>;
}) {
  const baseId = useId();
  const [name, setName] = useState("");
  const [key, setKey] = useState("");
  const [keyTouched, setKeyTouched] = useState(false);
  const [type, setType] = useState<FieldType>("text");
  const [options, setOptions] = useState("");
  const [error, setError] = useState<string | null>(null);
  const keyValue = keyTouched ? key : suggestedFieldKey(name);
  const trimmedKey = keyValue.trim();
  const keyValid = trimmedKey === "" || FIELD_KEY_PATTERN.test(trimmedKey);

  const create = useMutation({
    mutationFn: async () =>
      ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/collections/{collection_id}/fields", {
          params: { path: { workspace_id: workspaceId, collection_id: collectionId } },
          body: {
            name: name.trim(),
            type,
            ...(trimmedKey === "" ? {} : { key: trimmedKey }),
            ...(fieldTakesOptions(type) ? { options: parseOptionLines(options) } : {}),
          },
        }),
      ),
    onMutate: () => setError(null),
    onError: (err) =>
      setError(err instanceof ProblemError && err.titleKnown ? err.title : t("collection.saveError")),
    onSuccess: async () => {
      setName("");
      setKey("");
      setKeyTouched(false);
      setOptions("");
      await onSaved();
    },
  });

  return (
    <section
      aria-labelledby={`${baseId}-title`}
      className="settings-section"
      data-testid="collection-field-manager"
    >
      <div>
        <h2 id={`${baseId}-title`} className="settings-section__title">
          {t("collection.fieldSettings")}
        </h2>
        <p className="settings-section__lede">{t("project.settings.fields.description")}</p>
      </div>
      {canManage ? (
        <form
          className="flex flex-col gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (!create.isPending && keyValid && name.trim()) create.mutate();
          }}
          onKeyDown={(event) => {
            if (event.key === "Enter" && event.nativeEvent.isComposing) event.preventDefault();
          }}
        >
          <div className="collection-toolbar">
            <div className="collection-field">
              <label htmlFor={`${baseId}-name`}>{t("collection.fieldName")}</label>
              <Input
                id={`${baseId}-name`}
                className="h-9"
                value={name}
                maxLength={100}
                disabled={create.isPending}
                onChange={(event) => setName(event.target.value)}
              />
            </div>
            <div className="collection-field">
              <label htmlFor={`${baseId}-key`}>{t("collection.fieldKey")}</label>
              <Input
                id={`${baseId}-key`}
                className="h-9"
                aria-describedby={`${baseId}-key-hint`}
                aria-invalid={keyValid ? undefined : true}
                value={keyValue}
                maxLength={50}
                disabled={create.isPending}
                onChange={(event) => {
                  setKeyTouched(true);
                  setKey(event.target.value);
                }}
              />
            </div>
            <div className="collection-field">
              <label htmlFor={`${baseId}-type`}>{t("collection.fieldType")}</label>
              <select
                id={`${baseId}-type`}
                className="collection-select"
                value={type}
                disabled={create.isPending}
                onChange={(event) => {
                  const next = FIELD_TYPES.find((value) => value === event.target.value);
                  if (next) setType(next);
                }}
              >
                {FIELD_TYPES.map((value) => (
                  <option key={value} value={value}>
                    {fieldTypeLabel(value)}
                  </option>
                ))}
              </select>
            </div>
            <Button
              type="submit"
              size="sm"
              disabled={create.isPending || !name.trim() || !keyValid}
            >
              {t("collection.addField")}
            </Button>
          </div>
          <p id={`${baseId}-key-hint`} className="text-dense text-muted-foreground">
            {t("collection.fieldKey.hint")}
          </p>
          {fieldTakesOptions(type) ? (
            <div className="collection-field">
              <label htmlFor={`${baseId}-options`}>{t("collection.options")}</label>
              <textarea
                id={`${baseId}-options`}
                className="collection-textarea"
                value={options}
                disabled={create.isPending}
                onChange={(event) => setOptions(event.target.value)}
              />
            </div>
          ) : null}
          {error ? (
            <p role="alert" className="text-ui text-destructive">
              {error}
            </p>
          ) : null}
        </form>
      ) : (
        <p className="text-ui text-muted-foreground">{t("collection.readonly")}</p>
      )}
      {fields.length === 0 ? (
        <p className="text-ui text-muted-foreground">{t("project.settings.fields.empty")}</p>
      ) : (
        <div className="flex flex-col">
          {fields.map((field) => (
            <FieldSettings
              key={`${field.id}:${field.version}`}
              workspaceId={workspaceId}
              collectionId={collectionId}
              field={field}
              canManage={canManage}
              onSaved={onSaved}
            />
          ))}
        </div>
      )}
    </section>
  );
}
