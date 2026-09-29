// Adapted from source apps/web/src/features/settings/settings-instance.tsx:
// the catalog form's draft and display rules, framework-neutral (the React
// and Vue instance settings forms share them).
import { type I18nKey, isI18nKey, t } from "@fvoci/i18n";
import {
  type BrandingAssetKind,
  OVERRIDABLE_MESSAGES,
  type SettingsEntry,
  type SettingsKey,
} from "./settings-catalog";

/** The overridable server messages, in catalog order. */
export const MESSAGE_KEYS = Object.keys(OVERRIDABLE_MESSAGES);

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** The value at a dotted `path` of a settings document, or undefined. */
export function leafValue(doc: unknown, path: string): unknown {
  let cursor: unknown = doc;
  for (const part of path.split(".")) {
    if (!isRecord(cursor)) return undefined;
    cursor = cursor[part];
  }
  return cursor;
}

/** A copy of `doc` with the leaf at the dotted `path` set to `value`. */
export function withLeaf(doc: unknown, path: string, value: unknown): unknown {
  const [head, ...rest] = path.split(".");
  if (head === undefined) return value;
  const base = isRecord(doc) ? doc : {};
  return {
    ...base,
    [head]: rest.length === 0 ? value : withLeaf(base[head], rest.join("."), value),
  };
}

/** The preview never renders HTML: variables are shown by name only. */
export function previewText(text: string): string {
  return text.replace(/\{\{(\w+)\}\}/g, "$1");
}

/** A catalog label, help or option key as text (an unknown key as itself). */
export function settingLabel(key: string): string {
  return isI18nKey(key) ? t(key) : key;
}

/** The settings search: key, label, help, group and leaf names, ignoring case. */
export function settingMatches(key: SettingsKey, entry: SettingsEntry, query: string): boolean {
  if (query === "") return true;
  const hay = [
    key,
    settingLabel(entry.labelKey),
    settingLabel(entry.helpKey),
    settingLabel(entry.group),
    ...Object.keys(entry.widgets),
  ]
    .join(" ")
    .toLowerCase();
  return hay.includes(query.toLowerCase());
}

/** A number or duration field's text: emptied is unset (Number("") would be 0). */
export function numberFieldValue(raw: string): number | undefined {
  return raw === "" ? undefined : Number(raw);
}

/** A text field's text: empty is null (unset); required leaves reject both "" and null. */
export function textFieldValue(raw: string): string | null {
  return raw === "" ? null : raw;
}

/** A list field's text: one trimmed, non-empty entry per line, without repeats. */
export function listFieldValue(raw: string): string[] {
  const lines = raw
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
  return [...new Set(lines)];
}

/** The message overrides with `key` set to `text` (removed when empty). */
export function withMessageOverride(map: unknown, key: string, text: string): Record<string, string> {
  const next: Record<string, string> = {};
  if (isRecord(map)) {
    for (const [k, v] of Object.entries(map)) {
      if (typeof v === "string") next[k] = v;
    }
  }
  if (text === "") delete next[key];
  else next[key] = text;
  return next;
}

export function assetDigest(value: unknown): string | null {
  if (!isRecord(value)) return null;
  return typeof value.sha256 === "string" ? value.sha256 : null;
}

/** The upload digest versions the URL so a replaced asset is not served from cache. */
export function assetPreviewSrc(kind: BrandingAssetKind, digest: string): string {
  return `/api/v1/branding/${kind}?v=${digest.slice(0, 12)}`;
}

export const ASSET_COPY: Record<
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
