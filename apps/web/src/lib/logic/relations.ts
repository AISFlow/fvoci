/**
 * Card relation property values: links to other cards on the same board, each
 * carrying a dependency kind (`fs`, `ss`, `ff`, `sf`) and lag in whole calendar
 * days (`-3650..=3650`).
 *
 * Stored values are always normalized 3-field objects `{ card, kind, lagDays }`.
 * Legacy string-ID arrays (`["card-id"]`) are not accepted.
 */

/** The most cards one relation value names. */
export const RELATION_LIMIT = 50;

/** The four dependency kinds between a predecessor card and a successor card. */
export const RELATION_KINDS = ['fs', 'ss', 'ff', 'sf'] as const;
export type RelationKind = (typeof RELATION_KINDS)[number];

/** Minimum allowed lag in whole calendar days (-10 years). */
export const RELATION_LAG_MIN = -3650;

/** Maximum allowed lag in whole calendar days (+10 years). */
export const RELATION_LAG_MAX = 3650;

/** One normalized entry in a card relation property value. */
export interface RelationLink {
  card: string;
  kind: RelationKind;
  lagDays: number;
}

/** Whether `value` is one of the four relation kinds (`fs`, `ss`, `ff`, `sf`). */
export function isRelationKind(value: unknown): value is RelationKind {
  return value === 'fs' || value === 'ss' || value === 'ff' || value === 'sf';
}

/** Whether `value` is a whole integer within `RELATION_LAG_MIN..=RELATION_LAG_MAX`. */
export function isRelationLagDays(value: unknown): value is number {
  return (
    typeof value === 'number' &&
    Number.isInteger(value) &&
    value >= RELATION_LAG_MIN &&
    value <= RELATION_LAG_MAX
  );
}

/** Constructs a normalized relation link (defaults: `kind = 'fs'`, `lagDays = 0`). */
export function relationLink(card: string, kind: RelationKind = 'fs', lagDays = 0): RelationLink {
  return { card, kind, lagDays: lagDays === 0 ? 0 : lagDays };
}

/**
 * Validates and normalizes a single wire value as a `RelationLink`.
 *
 * Rejects non-objects (including legacy string IDs), unknown keys, empty `card`,
 * invalid `kind`, and non-integer or out-of-range `lagDays`. Omitted `kind`
 * defaults to `'fs'` and omitted `lagDays` defaults to `0`.
 */
export function parseRelationLink(value: unknown): RelationLink | undefined {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return undefined;
  const record = value as Record<string, unknown>;
  for (const key of Object.keys(record)) {
    if (key !== 'card' && key !== 'kind' && key !== 'lagDays') return undefined;
  }
  const card = record.card;
  if (typeof card !== 'string' || card === '') return undefined;
  let kind: RelationKind = 'fs';
  if (Object.hasOwn(record, 'kind')) {
    if (!isRelationKind(record.kind)) return undefined;
    kind = record.kind;
  }
  let lagDays = 0;
  if (Object.hasOwn(record, 'lagDays')) {
    if (!isRelationLagDays(record.lagDays)) return undefined;
    lagDays = record.lagDays === 0 ? 0 : record.lagDays;
  }
  return { card, kind, lagDays };
}

/** Whether `value` is a non-empty list of objects (used to detect relation-link-shaped arrays). */
export function isRelationLinksValue(value: unknown): boolean {
  return (
    Array.isArray(value) &&
    value.length > 0 &&
    value.every((item) => typeof item === 'object' && item !== null && !Array.isArray(item))
  );
}

/**
 * The relation links a relation property holds, each card once in the order it
 * appears, normalized to `{ card, kind, lagDays }`. Whatever else is stored
 * (text left by another kind, a date, legacy string arrays, malformed link
 * objects) is ignored.
 */
export function parseRelationValue(stored: unknown): RelationLink[] {
  if (!Array.isArray(stored)) return [];
  const seen = new Set<string>();
  const links: RelationLink[] = [];
  for (const item of stored as unknown[]) {
    const link = parseRelationLink(item);
    if (!link || seen.has(link.card)) continue;
    seen.add(link.card);
    links.push(link);
  }
  return links;
}

/** The card identifiers a relation property names, each once in stored order. */
export function relationCardIds(stored: unknown): string[] {
  return parseRelationValue(stored).map((link) => link.card);
}

/** Clamps a numeric lag value to an integer within `RELATION_LAG_MIN..=RELATION_LAG_MAX`. */
export function clampRelationLagDays(value: number): number {
  if (!Number.isFinite(value)) return 0;
  const clamped = Math.min(RELATION_LAG_MAX, Math.max(RELATION_LAG_MIN, Math.trunc(value)));
  return clamped === 0 ? 0 : clamped;
}

/**
 * Parses a user-entered lag value as a whole integer in `RELATION_LAG_MIN..=RELATION_LAG_MAX`.
 * Returns `undefined` when empty, non-integer, or out of bounds.
 */
export function parseLagDaysInput(raw: string | number | null | undefined): number | undefined {
  if (raw === null || raw === undefined) return undefined;
  if (typeof raw === 'number') return isRelationLagDays(raw) ? (raw === 0 ? 0 : raw) : undefined;
  const trimmed = raw.trim();
  if (!/^[+-]?\d+$/.test(trimmed)) return undefined;
  const parsed = Number(trimmed);
  return isRelationLagDays(parsed) ? (parsed === 0 ? 0 : parsed) : undefined;
}

/**
 * Returns a copy of `links` with the entry for `targetCardId` updated, keeping
 * every other entry (including hidden/deleted cards) and their order intact.
 */
export function updateRelationLink(
  links: readonly RelationLink[],
  targetCardId: string,
  patch: { kind?: RelationKind; lagDays?: number },
): RelationLink[] {
  return links.map((link) => {
    if (link.card !== targetCardId) return relationLink(link.card, link.kind, link.lagDays);
    const kind = patch.kind ?? link.kind;
    const lagDays =
      patch.lagDays === undefined ? link.lagDays : clampRelationLagDays(patch.lagDays);
    return relationLink(link.card, kind, lagDays);
  });
}

/**
 * Returns a copy of `links` without `targetCardId`, keeping every other entry
 * (including hidden/deleted cards) and their order intact.
 */
export function removeRelationLink(
  links: readonly RelationLink[],
  targetCardId: string,
): RelationLink[] {
  return links
    .filter((link) => link.card !== targetCardId)
    .map((link) => relationLink(link.card, link.kind, link.lagDays));
}
