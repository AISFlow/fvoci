// Fixed source393795: lib/search-tag.ts; tag:name is a case-insensitive leading token.
export function parseSearchTagPrefix(raw: string, tags: ReadonlyArray<{ id: string; name: string }>): { q: string; tag?: string } {
  const match = /^tag:(\S+)(?:\s+(.*))?$/i.exec(raw.trim());
  if (!match) return { q: raw };
  const name = match[1] ?? "";
  const found = tags.find(tag => tag.name.toLowerCase() === name.toLowerCase());
  if (!found) return { q: raw };
  const rest = (match[2] ?? "").trim();
  return { q: rest || name, tag: found.id };
}
