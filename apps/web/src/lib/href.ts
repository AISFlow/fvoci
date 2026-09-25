const WIKI_PREFIX = "WIKI";

/** Source `projectKeyPattern`: NFKC uppercase, 2–32, not `KEY-n`. */
export const PROJECT_KEY_PATTERN = /^(?!.*-\d+$)[A-Z][A-Z0-9-]{1,31}$/;

/** Source `RESERVED_URL_SEGMENTS` (uppercase). */
export const RESERVED_PROJECT_KEYS = new Set([
  "WIKI",
  "PROJECTS",
  "SEARCH",
  "MY-TASKS",
  "TRASH",
  "NOTIFICATIONS",
  "SETTINGS",
  "A",
]);

export type ParsedRef =
  | { kind: "item"; prefix: string; number: number; displayId: string }
  | { kind: "project"; key: string };

export function formatDisplayId(prefix: string, number: number): string {
  return `${prefix}-${number}`;
}

export function wikiDisplayId(number: number): string {
  return formatDisplayId(WIKI_PREFIX, number);
}

export function canonicalizeProjectKey(raw: string): string {
  return raw.normalize("NFKC").toUpperCase();
}

export function projectKeyIssue(raw: string): "reserved" | "pattern" | null {
  const key = canonicalizeProjectKey(raw);
  if (RESERVED_PROJECT_KEYS.has(key)) return "reserved";
  if (!PROJECT_KEY_PATTERN.test(key)) return "pattern";
  return null;
}

/** Parse `/w/:slug/:ref` wiki document refs such as `WIKI-12`. */
export function parseWikiRef(raw: string): { prefix: string; number: number } | null {
  const item = parseItemRef(raw);
  if (!item || item.prefix !== WIKI_PREFIX) return null;
  return { prefix: item.prefix, number: item.number };
}

/** Source `parseDisplayId`: prefix 2–32 then `-n` without leading zeros. */
export function parseItemRef(raw: string): { prefix: string; number: number; displayId: string } | null {
  const match = /^([A-Za-z0-9-]{2,32})-(\d{1,9})$/.exec(raw.trim());
  if (!match) return null;
  const digits = match[2];
  if (digits.length > 1 && digits.startsWith("0")) return null;
  const number = Number.parseInt(digits, 10);
  if (!Number.isFinite(number) || number < 1) return null;
  const prefix = match[1].toUpperCase();
  return { prefix, number, displayId: formatDisplayId(prefix, number) };
}

export function parseRef(raw: string): ParsedRef | null {
  const ref = raw.trim();
  if (ref === "") return null;
  const item = parseItemRef(ref);
  if (item) return { kind: "item", ...item };
  const key = canonicalizeProjectKey(ref);
  if (!PROJECT_KEY_PATTERN.test(key) || RESERVED_PROJECT_KEYS.has(key)) return null;
  return { kind: "project", key };
}

export function workspaceHomePath(slug: string): string {
  return `/w/${slug.toLowerCase()}`;
}

export function wikiPath(slug: string): string {
    return `/w/${slug.toLowerCase()}/wiki`;
}

export function trashPath(slug: string): string {
    return `/w/${slug.toLowerCase()}/trash`;
}

export function documentPath(slug: string, displayId: string): string {
  return `/w/${slug.toLowerCase()}/${displayId}`;
}

/** Source `href.attachment`: `/w/:slug/a/:attachmentId/view` with optional `?chunk=`. */
export function attachmentViewPath(
  slug: string,
  attachmentId: string,
  chunk?: number | null,
): string {
  const base = `/w/${slug.toLowerCase()}/a/${attachmentId}/view`;
  return chunk === null || chunk === undefined ? base : `${base}?chunk=${chunk}`;
}

export const COMMENTS_ANCHOR_ID = "document-comments";

export function itemPath(slug: string, displayId: string): string {
  return documentPath(slug, displayId);
}

export function settingsPath(slug: string): string {
  return `/w/${slug.toLowerCase()}/settings`;
}

export function projectsPath(slug: string): string {
  return `/w/${slug.toLowerCase()}/projects`;
}

export function notificationsPath(slug: string): string {
  return `/w/${slug.toLowerCase()}/notifications`;
}

export function searchPath(
  slug: string,
  params?: { q?: string; tab?: string; projectId?: string },
): string {
  const search = new URLSearchParams();
  if (params?.q) search.set("q", params.q);
  if (params?.tab && params.tab !== "all") search.set("tab", params.tab);
  if (params?.projectId) search.set("projectId", params.projectId);
  const qs = search.toString();
  const base = `/w/${slug.toLowerCase()}/search`;
  return qs ? `${base}?${qs}` : base;
}

export function projectTasksPath(slug: string, key: string): string {
  return `/w/${slug.toLowerCase()}/${canonicalizeProjectKey(key)}/tasks`;
}

export function projectPath(slug: string, key: string): string {
  return `/w/${slug.toLowerCase()}/${canonicalizeProjectKey(key)}`;
}
