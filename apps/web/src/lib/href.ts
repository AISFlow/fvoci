const WIKI_PREFIX = "WIKI";

export function formatDisplayId(prefix: string, number: number): string {
  return `${prefix}-${number}`;
}

export function wikiDisplayId(number: number): string {
  return formatDisplayId(WIKI_PREFIX, number);
}

/** Parse `/w/:slug/:ref` wiki document refs such as `WIKI-12`. */
export function parseWikiRef(raw: string): { prefix: string; number: number } | null {
  const match = /^([A-Za-z]+)-(\d+)$/.exec(raw.trim());
  if (!match) return null;
  const digits = match[2];
  if (digits.length > 1 && digits.startsWith("0")) return null;
  const number = Number.parseInt(digits, 10);
  if (!Number.isFinite(number) || number < 1) return null;
  return { prefix: match[1].toUpperCase(), number };
}

export function wikiPath(slug: string): string {
  return `/w/${slug.toLowerCase()}/wiki`;
}

export function documentPath(slug: string, displayId: string): string {
  return `/w/${slug.toLowerCase()}/${displayId}`;
}

export function settingsPath(slug: string): string {
  return `/w/${slug.toLowerCase()}/settings`;
}
