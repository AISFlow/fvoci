// Adapted from source apps/web/src/lib/view-query.ts. The source parses with the
// shared zod `viewQuery` contract; this rewrite normalizes by hand into the same
// canonical key order so equality and `expectedConfig` compare-and-swap agree
// with the server (`src/tasks/list_query.rs`).

export type SortDirection = "asc" | "desc";
/** Built-in task sort keys; any other value is a collection field id. */
export const BUILTIN_SORT_FIELDS = [
  "priority",
  "due",
  "updated",
  "created",
  "rank",
  "title",
  "status",
  "number",
] as const;
export type ViewSort = { field: string; direction: SortDirection };

export type CustomFilter =
  | { fieldId: string; operator: "equals"; value: string | number | boolean }
  | { fieldId: string; operator: "empty" };

export type ViewFilters = {
  type?: string;
  statusId?: string;
  assigneeId?: string;
  priority?: string;
  labelId?: string;
  milestoneId?: string;
  openOnly?: boolean;
  dueBefore?: string;
  title?: string;
  custom?: CustomFilter[];
};

export type ViewQuery = { filters: ViewFilters; sort: ViewSort[] };

export const SORT_MAX = 3;
export const CUSTOM_FILTERS_MAX = 25;
const TITLE_MAX = 1000;

const STRING_FILTER_KEYS = [
  "type",
  "statusId",
  "assigneeId",
  "priority",
  "labelId",
  "milestoneId",
] as const;

export const EMPTY_VIEW_QUERY: ViewQuery = Object.freeze({
  filters: Object.freeze({}) as ViewFilters,
  sort: Object.freeze([]) as unknown as ViewSort[],
}) as ViewQuery;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function normalizeCustom(raw: unknown): CustomFilter[] | null {
  if (!Array.isArray(raw)) return null;
  const out: CustomFilter[] = [];
  for (const entry of raw.slice(0, CUSTOM_FILTERS_MAX)) {
    if (!isRecord(entry) || typeof entry.fieldId !== "string" || entry.fieldId === "") return null;
    if (entry.operator === "empty") {
      out.push({ fieldId: entry.fieldId, operator: "empty" });
    } else if (entry.operator === "equals") {
      const value = entry.value;
      if (
        typeof value !== "string" &&
        typeof value !== "boolean" &&
        !(typeof value === "number" && Number.isFinite(value))
      ) {
        return null;
      }
      out.push({ fieldId: entry.fieldId, operator: "equals", value });
    } else {
      return null;
    }
  }
  return out;
}

/**
 * Canonical view query: fixed key order, empty/false values dropped. Returns
 * `null` for input the server would reject as `invalid_input`.
 */
export function normalizeViewQuery(raw: unknown): ViewQuery | null {
  if (raw === undefined || raw === null) return { filters: {}, sort: [] };
  if (!isRecord(raw)) return null;
  const rawFilters = raw.filters ?? {};
  const rawSort = raw.sort ?? [];
  if (!isRecord(rawFilters) || !Array.isArray(rawSort)) return null;

  const filters: ViewFilters = {};
  for (const key of STRING_FILTER_KEYS) {
    const value = rawFilters[key];
    if (value === undefined || value === null || value === "") continue;
    if (typeof value !== "string") return null;
    filters[key] = value;
  }
  if (rawFilters.openOnly === true) filters.openOnly = true;
  else if (rawFilters.openOnly !== undefined && rawFilters.openOnly !== false) return null;
  if (typeof rawFilters.dueBefore === "string" && rawFilters.dueBefore !== "") {
    if (!/^\d{4}-\d{2}-\d{2}$/.test(rawFilters.dueBefore)) return null;
    filters.dueBefore = rawFilters.dueBefore;
  }
  if (typeof rawFilters.title === "string") {
    const title = rawFilters.title.trim().slice(0, TITLE_MAX);
    if (title !== "") filters.title = title;
  }
  if (rawFilters.custom !== undefined) {
    const custom = normalizeCustom(rawFilters.custom);
    if (custom === null) return null;
    if (custom.length > 0) filters.custom = custom;
  }

  if (rawSort.length > SORT_MAX) return null;
  const sort: ViewSort[] = [];
  for (const entry of rawSort) {
    if (!isRecord(entry) || typeof entry.field !== "string" || entry.field === "") return null;
    if (entry.direction !== "asc" && entry.direction !== "desc") return null;
    sort.push({ field: entry.field, direction: entry.direction });
  }
  return { filters, sort };
}

function canonical(query: ViewQuery): ViewQuery {
  return normalizeViewQuery(query) ?? { filters: {}, sort: [] };
}

export function isEmptyViewQuery(query: ViewQuery): boolean {
  const normalized = canonical(query);
  return Object.keys(normalized.filters).length === 0 && normalized.sort.length === 0;
}

export function viewQueriesEqual(left: ViewQuery, right: ViewQuery): boolean {
  return JSON.stringify(canonical(left)) === JSON.stringify(canonical(right));
}

/** `?query=` value for the task list; `undefined` when nothing narrows or orders. */
export function encodeViewQueryParam(query: ViewQuery): string | undefined {
  const normalized = canonical(query);
  return isEmptyViewQuery(normalized) ? undefined : JSON.stringify(normalized);
}

export function parseViewQueryParam(raw: string | null | undefined): ViewQuery | null {
  if (!raw) return { filters: {}, sort: [] };
  if (raw.length > 16_000) return null;
  try {
    return normalizeViewQuery(JSON.parse(raw));
  } catch {
    return null;
  }
}

export function patchViewFilter<K extends keyof ViewFilters>(
  query: ViewQuery,
  key: K,
  value: ViewFilters[K] | undefined,
): ViewQuery {
  const filters: ViewFilters = { ...query.filters };
  if (
    value === undefined ||
    value === false ||
    value === "" ||
    (Array.isArray(value) && value.length === 0)
  ) {
    delete filters[key];
  } else {
    filters[key] = value;
  }
  return canonical({ ...query, filters });
}

export function readPrimarySort(query: ViewQuery): ViewSort | null {
  return query.sort.length === 1 ? (query.sort[0] ?? null) : null;
}

export function setPrimarySort(
  query: ViewQuery,
  field: string | null,
  direction: SortDirection,
): ViewQuery {
  return canonical({ ...query, sort: field ? [{ field, direction }] : [] });
}

export function addCustomFilter(query: ViewQuery, filter: CustomFilter): ViewQuery {
  const current = query.filters.custom ?? [];
  if (current.some((item) => JSON.stringify(item) === JSON.stringify(filter))) return query;
  return patchViewFilter(query, "custom", [...current, filter]);
}

export function removeCustomFilter(query: ViewQuery, index: number): ViewQuery {
  return patchViewFilter(
    query,
    "custom",
    (query.filters.custom ?? []).filter((_, current) => current !== index),
  );
}
