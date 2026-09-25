// Adapted from source apps/web/src/features/settings/settings-instance.tsx.
// The catalog draws the form: adding a key needs no screen code, and the
// widget switch has no default so a new widget is a compile error.
import { type I18nKey, isI18nKey, t } from "@fvoci/i18n";
import { useState } from "react";
import { ConfirmActionButton } from "@/components/confirm-action";
import { QueryError, QueryLoading } from "@/components/query-status";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { components } from "@/generated/api";
import { problemMessage } from "@/lib/api";
import {
  BRANDING_ASSET_KINDS,
  BRANDING_ASSET_MIME,
  type BrandingAssetKind,
  EE_GATED,
  OVERRIDABLE_MESSAGES,
  SETTING_ENUM_OPTIONS,
  SETTINGS_CATALOG,
  SETTINGS_ENTRIES,
  type SettingsEntry,
  type SettingsKey,
  type SettingsWidget,
  optionKey,
  withoutAssets,
} from "./settings-catalog";
import "./settings-shell.css";

type AdminInstanceSettingsOutput = components["schemas"]["AdminInstanceSettingsOutput"];

const MESSAGE_KEYS = Object.keys(OVERRIDABLE_MESSAGES);

const textareaClass =
  "w-full min-w-0 rounded-md border border-input bg-background px-3 py-2 text-ui outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring disabled:opacity-50";
const badgeClass =
  "inline-flex items-center rounded-full border border-border px-2 py-0.5 text-caption text-muted-foreground";

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function leafValue(doc: unknown, path: string): unknown {
  let cursor: unknown = doc;
  for (const part of path.split(".")) {
    if (!isRecord(cursor)) return undefined;
    cursor = cursor[part];
  }
  return cursor;
}

function withLeaf(doc: unknown, path: string, value: unknown): unknown {
  const [head, ...rest] = path.split(".");
  if (head === undefined) return value;
  const base = isRecord(doc) ? doc : {};
  return {
    ...base,
    [head]: rest.length === 0 ? value : withLeaf(base[head], rest.join("."), value),
  };
}

/** The preview never renders HTML: variables are shown by name only. */
function previewText(text: string): string {
  return text.replace(/\{\{(\w+)\}\}/g, "$1");
}

function label(key: string): string {
  return isI18nKey(key) ? t(key) : key;
}

function matches(key: SettingsKey, entry: SettingsEntry, q: string) {
  if (q === "") return true;
  const hay = [key, label(entry.labelKey), label(entry.helpKey), label(entry.group), ...Object.keys(entry.widgets)]
    .join(" ")
    .toLowerCase();
  return hay.includes(q.toLowerCase());
}

function MessageOverrides({
  value,
  disabled,
  onChange,
}: {
  value: unknown;
  disabled: boolean;
  onChange: (next: Record<string, string>) => void;
}) {
  const map = isRecord(value) ? value : {};
  return (
    <div className="flex flex-col gap-4">
      {MESSAGE_KEYS.map((key) => {
        const raw = map[key];
        const current = typeof raw === "string" ? raw : "";
        const invalid = current.includes("<");
        return (
          <div className="flex flex-col gap-1" key={key}>
            <Label className="font-mono text-caption" htmlFor={`msg-${key}`}>
              {key}
            </Label>
            <p className="whitespace-pre-line text-caption text-muted-foreground">{label(key)}</p>
            <textarea
              className={textareaClass}
              disabled={disabled}
              id={`msg-${key}`}
              onChange={(event) => {
                const next: Record<string, string> = {};
                for (const [k, v] of Object.entries(map)) {
                  if (typeof v === "string") next[k] = v;
                }
                if (event.target.value === "") delete next[key];
                else next[key] = event.target.value;
                onChange(next);
              }}
              rows={2}
              value={current}
            />
            {invalid ? (
              <p className="text-caption text-destructive" role="alert">
                {t("settings.ui.angleBracket")}
              </p>
            ) : null}
            {current === "" ? null : (
              <p className="text-caption text-muted-foreground">
                {t("settings.ui.preview")}: {previewText(current)}
              </p>
            )}
          </div>
        );
      })}
    </div>
  );
}

function SettingField({
  widget,
  id,
  value,
  disabled,
  options,
  onChange,
}: {
  widget: Exclude<SettingsWidget, "asset">;
  id: string;
  value: unknown;
  disabled: boolean;
  options: readonly { value: string; label: string }[];
  onChange: (next: unknown) => void;
}) {
  switch (widget) {
    case "boolean":
      return (
        <input
          type="checkbox"
          className="size-5"
          checked={value === true}
          disabled={disabled}
          id={id}
          onChange={(event) => onChange(event.target.checked)}
        />
      );
    case "enum":
      return (
        <select
          className="h-11 w-56 rounded-md border border-input bg-background px-3 text-ui disabled:opacity-50"
          disabled={disabled}
          id={id}
          onChange={(event) =>
            onChange(typeof value === "number" ? Number(event.target.value) : event.target.value)
          }
          value={String(value)}
        >
          {options.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      );
    case "number":
    case "duration":
      return (
        <Input
          className="w-56"
          disabled={disabled}
          id={id}
          onChange={(event) => {
            // Number("") === 0 — an emptied field stays unset instead.
            const raw = event.target.value;
            onChange(raw === "" ? undefined : Number(raw));
          }}
          type="number"
          value={typeof value === "number" ? value : ""}
        />
      );
    case "text":
      return (
        <Input
          disabled={disabled}
          id={id}
          // Empty = null (unset); required leaves reject both "" and null.
          onChange={(event) => onChange(event.target.value === "" ? null : event.target.value)}
          value={typeof value === "string" ? value : ""}
        />
      );
    case "list":
      return (
        <textarea
          className={textareaClass}
          disabled={disabled}
          id={id}
          onChange={(event) => {
            const lines = event.target.value
              .split("\n")
              .map((line) => line.trim())
              .filter((line) => line !== "");
            onChange([...new Set(lines)]);
          }}
          rows={4}
          value={Array.isArray(value) ? value.join("\n") : ""}
        />
      );
    case "i18n_override":
      return <MessageOverrides disabled={disabled} onChange={onChange} value={value} />;
  }
}

function assetDigest(value: unknown): string | null {
  if (!isRecord(value)) return null;
  return typeof value.sha256 === "string" ? value.sha256 : null;
}

/** The upload digest versions the URL so a replaced asset is not served from cache. */
export function assetPreviewSrc(kind: BrandingAssetKind, digest: string): string {
  return `/api/v1/branding/${kind}?v=${digest.slice(0, 12)}`;
}

const ASSET_COPY: Record<
  BrandingAssetKind,
  { alt: I18nKey; none: I18nKey; clear: I18nKey; title: I18nKey; body: I18nKey }
> = {
  logo: {
    alt: "settings.ui.assetAlt.logo",
    none: "settings.ui.assetNone.logo",
    clear: "settings.ui.assetClear.logo",
    title: "settings.ui.assetClearTitle.logo",
    body: "settings.ui.assetClearBody.logo",
  },
  favicon: {
    alt: "settings.ui.assetAlt.favicon",
    none: "settings.ui.assetNone.favicon",
    clear: "settings.ui.assetClear.favicon",
    title: "settings.ui.assetClearTitle.favicon",
    body: "settings.ui.assetClearBody.favicon",
  },
};

/** Asset leaves bypass the draft: upload is a raw POST, clearing is a DELETE. */
function AssetField({
  kind,
  id,
  value,
  instanceName,
  disabled,
  onAsset,
}: {
  kind: BrandingAssetKind;
  id: string;
  value: unknown;
  instanceName: string;
  disabled: boolean;
  onAsset: (kind: BrandingAssetKind, file: File | null) => Promise<void>;
}) {
  const [problem, setProblem] = useState<string | null>(null);
  const copy = ASSET_COPY[kind];
  const digest = assetDigest(value);
  function run(file: File | null): void {
    setProblem(null);
    void onAsset(kind, file).catch((err: unknown) => {
      setProblem(problemMessage(err, "error.network"));
    });
  }
  return (
    <div className="flex flex-col gap-2">
      {digest === null ? (
        <p className="text-caption text-muted-foreground">{t(copy.none)}</p>
      ) : (
        <div className="flex h-16 w-40 items-center justify-start rounded-md border border-border p-2">
          <img
            alt={t(copy.alt, { name: instanceName })}
            className="max-h-full max-w-full object-contain"
            src={assetPreviewSrc(kind, digest)}
          />
        </div>
      )}
      <Input
        accept={BRANDING_ASSET_MIME.join(",")}
        aria-describedby={problem === null ? undefined : `${id}-problem`}
        disabled={disabled}
        id={id}
        onChange={(event) => {
          const file = event.target.files?.[0];
          // Picking the same file again fires no change unless the field is cleared.
          event.target.value = "";
          if (file !== undefined) run(file);
        }}
        type="file"
      />
      {problem === null ? null : (
        <p className="text-caption text-destructive" id={`${id}-problem`} role="alert">
          {problem}
        </p>
      )}
      {digest === null ? null : (
        <div>
          <ConfirmActionButton
            actionLabel={t(copy.clear)}
            description={t(copy.body)}
            disabled={disabled}
            onConfirm={() => run(null)}
            title={t(copy.title)}
          >
            {t(copy.clear)}
          </ConfirmActionButton>
        </div>
      )}
    </div>
  );
}

export function InstanceSettingsView({
  data,
  loading,
  error,
  pending,
  onSave,
  onAsset,
  onRetry,
}: {
  data: AdminInstanceSettingsOutput | null;
  loading: boolean;
  error: string | null;
  onRetry?: () => void;
  pending: boolean;
  /** The body carries only the documents the admin actually edited. */
  onSave: (patch: Record<string, unknown>) => Promise<void>;
  /** `null` clears the asset. */
  onAsset: (kind: BrandingAssetKind, file: File | null) => Promise<void>;
}) {
  const [query, setQuery] = useState("");
  const [draft, setDraft] = useState<Partial<Record<SettingsKey, unknown>>>({});

  const values: Record<string, unknown> = data?.values ?? {};
  const overridden = new Set(data?.overridden ?? []);
  const restartPending = new Set(data?.restartRequired ?? []);
  const envApplied = new Set(data?.envApplied ?? []);
  const eeFeatures = new Set(data?.eeFeatures ?? []);
  const brandingName = leafValue(values.branding, "name");
  const instanceName = typeof brandingName === "string" ? brandingName : "";

  const shown = SETTINGS_ENTRIES.filter(([key, entry]) => matches(key, entry, query));

  async function submit(key: SettingsKey, next: unknown): Promise<void> {
    if (next === undefined) return;
    try {
      await onSave({ [key]: next === null ? null : withoutAssets(key, next) });
    } catch {
      // Keep the edit; the page shows the error.
      return;
    }
    setDraft((prev) => {
      const rest = { ...prev };
      delete rest[key];
      return rest;
    });
  }

  return (
    <section className="settings-section" aria-labelledby="instance-settings-title">
      <h2 className="settings-section__title text-title" id="instance-settings-title">
        {t("settings.ui.title")}
      </h2>
      <p className="settings-section__lede">{t("settings.ui.help")}</p>
      <div className="flex flex-col gap-4 text-ui">
        <Label className="sr-only" htmlFor="settings-search">
          {t("settings.ui.search")}
        </Label>
        <Input
          id="settings-search"
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t("settings.ui.search")}
          value={query}
        />
        {loading ? <QueryLoading /> : null}
        {error && onRetry ? (
          <QueryError message={error} onRetry={onRetry} />
        ) : error ? (
          <p className="text-ui text-destructive" role="alert">
            {error}
          </p>
        ) : null}
        {!loading && shown.length === 0 ? (
          <p className="text-ui text-muted-foreground">{t("settings.ui.noMatch")}</p>
        ) : null}
        {data === null
          ? null
          : shown.map(([key, entry]) => {
              const edited = draft[key] !== undefined;
              const current = edited ? draft[key] : values[key];
              const parsed = edited ? SETTINGS_CATALOG[key].schema.safeParse(current) : null;
              const issue = parsed && !parsed.success ? parsed.error.issues[0] : undefined;
              const invalidDraft = issue !== undefined;
              const gate = EE_GATED[key];
              const eeLocked = gate !== undefined && !eeFeatures.has(gate);
              return (
                <section
                  aria-labelledby={`setting-${key}`}
                  className="flex flex-col gap-3 rounded-md border border-border p-4"
                  key={key}
                >
                  <div className="flex flex-wrap items-center gap-2">
                    <h3 className="text-ui font-medium" id={`setting-${key}`}>
                      {label(entry.labelKey)}
                    </h3>
                    <span className={badgeClass}>{label(entry.group)}</span>
                    {entry.safety === "restart_required" ? (
                      <span className={badgeClass}>{t("settings.ui.restart")}</span>
                    ) : null}
                    {overridden.has(key) ? (
                      <span className={badgeClass}>{t("settings.ui.overridden")}</span>
                    ) : null}
                    {eeLocked ? <span className={badgeClass}>{t("ee.badge")}</span> : null}
                  </div>
                  <p className="text-caption text-muted-foreground">{label(entry.helpKey)}</p>
                  {eeLocked ? (
                    <p className="text-caption text-muted-foreground">{t("ee.required")}</p>
                  ) : null}
                  {restartPending.has(key) ? (
                    <p className="text-caption text-destructive">{t("settings.ui.restartPending")}</p>
                  ) : null}
                  {Object.entries(entry.widgets).map(([leaf, widget]) => {
                    const path = `${key}.${leaf}`;
                    const fixed = envApplied.has(path);
                    const locked = pending || fixed || eeLocked;
                    const assetKind = BRANDING_ASSET_KINDS.find((kind) => kind === leaf) ?? null;
                    const options = (SETTING_ENUM_OPTIONS[path] ?? []).map((value) => ({
                      value,
                      label: label(optionKey(key, leaf, value)),
                    }));
                    return (
                      <div className="flex flex-col gap-1" key={leaf}>
                        <Label className="font-mono text-caption" htmlFor={path} id={`${path}-label`}>
                          {path}
                        </Label>
                        {widget === "asset" ? (
                          assetKind === null ? null : (
                            <AssetField
                              disabled={locked}
                              id={path}
                              instanceName={instanceName}
                              kind={assetKind}
                              onAsset={onAsset}
                              value={leafValue(values[key], leaf)}
                            />
                          )
                        ) : (
                          <SettingField
                            disabled={locked}
                            id={path}
                            onChange={(next) => {
                              setDraft((prev) => ({
                                ...prev,
                                [key]: withLeaf(prev[key] ?? values[key], leaf, next),
                              }));
                            }}
                            options={options}
                            value={leafValue(current, leaf)}
                            widget={widget}
                          />
                        )}
                        {fixed ? (
                          <p className="text-caption text-muted-foreground">{t("settings.ui.envFixed")}</p>
                        ) : null}
                      </div>
                    );
                  })}
                  <div className="flex flex-wrap gap-2">
                    {entry.confirmDestructive ? (
                      <ConfirmActionButton
                        actionLabel={t("settings.ui.save")}
                        description={t("settings.ui.confirmBody")}
                        disabled={pending || !edited || invalidDraft || eeLocked}
                        onConfirm={() => submit(key, draft[key])}
                        title={t("settings.ui.confirmTitle")}
                        triggerVariant="default"
                      >
                        {t("settings.ui.save")}
                      </ConfirmActionButton>
                    ) : (
                      <Button
                        disabled={pending || !edited || invalidDraft || eeLocked}
                        onClick={() => void submit(key, draft[key])}
                        size="sm"
                        type="button"
                      >
                        {t("settings.ui.save")}
                      </Button>
                    )}
                    {overridden.has(key) ? (
                      <Button
                        disabled={pending || eeLocked}
                        onClick={() => void submit(key, null)}
                        size="sm"
                        type="button"
                        variant="outline"
                      >
                        {t("settings.ui.reset")}
                      </Button>
                    ) : null}
                  </div>
                  {issue ? (
                    <p className="text-caption text-destructive" role="alert">
                      {issue.path.join(".")}: {issue.message}
                    </p>
                  ) : null}
                </section>
              );
            })}
      </div>
    </section>
  );
}
