import type { I18nKey } from "@fvoci/i18n";
import type { EmbedEntity, EntitySnapshot } from "./entities.js";

/* The embed block's view model without a UI framework: the React view
 * (react/blocks.tsx) and the Vue view (vue/EmbedNodeView.vue) render the same
 * states from these. */

export const EMBED_KIND_KEY = {
  document: "editor.embed.document",
  task: "editor.embed.task",
  project: "editor.embed.project",
  url: "editor.link",
} as const satisfies Record<EmbedEntity, I18nKey>;

export const EMBED_ICON: Record<Exclude<EmbedEntity, "url">, string> = {
  document: "📄",
  task: "☑",
  project: "📁",
};

export function isEmbedHttpUrl(value: string): boolean {
  try {
    const parsed = new URL(value);
    return parsed.protocol === "http:" || parsed.protocol === "https:";
  } catch {
    return false;
  }
}

/** The committed embed for an edited reference: an http(s) URL is always a
 * URL embed, anything else keeps the selected entity kind. */
export function resolveEmbedProps(
  raw: string,
  selected: EmbedEntity,
): { entity: EmbedEntity; ref: string } {
  const ref = raw.trim();
  if (isEmbedHttpUrl(ref)) return { entity: "url", ref };
  return { entity: selected, ref };
}

export type EmbedCardState =
  | { state: "loading" }
  | { state: "inaccessible" }
  | { state: "plain"; ref: string }
  | { state: "resolved"; snapshot: EntitySnapshot };
