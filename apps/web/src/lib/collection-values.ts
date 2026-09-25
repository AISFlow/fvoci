// Pure helpers for collection fields, values and the month calendar. Adapted
// from source apps/web/src/features/collections/{value-editor,field-settings,
// collection-field-manager,collection-calendar}.tsx and lib/datetime.ts.

export const FIELD_TYPES = [
  "text",
  "paragraph",
  "number",
  "date",
  "datetime",
  "checkbox",
  "select",
  "multi_select",
  "checkboxes",
  "user",
  "user_multi",
  "labels",
] as const;
export type FieldType = (typeof FIELD_TYPES)[number];

export const OPTION_FIELD_TYPES: readonly string[] = ["select", "multi_select", "checkboxes", "labels"];
/** Scalar types the server accepts as sort keys. */
export const SORTABLE_FIELD_TYPES: readonly string[] = [
  "text",
  "paragraph",
  "number",
  "date",
  "datetime",
  "checkbox",
];
export const DRAFT_FIELD_TYPES: readonly string[] = ["text", "paragraph", "number", "date", "datetime"];

export function isFieldType(value: string): value is FieldType {
  return (FIELD_TYPES as readonly string[]).includes(value);
}

export function fieldTakesOptions(type: string): boolean {
  return OPTION_FIELD_TYPES.includes(type);
}

export type CollectionValue =
  | null
  | { text: string }
  | { number: number }
  | { date: string }
  | { datetime: string }
  | { checkbox: boolean }
  | { options: string[] }
  | { users: string[] };

/** Narrow the generated `Record<string, never>` value payloads. */
export function asCollectionValue(raw: unknown): CollectionValue {
  if (typeof raw !== "object" || raw === null) return null;
  const value = raw as Record<string, unknown>;
  if (typeof value.text === "string") return { text: value.text };
  if (typeof value.number === "number") return { number: value.number };
  if (typeof value.date === "string") return { date: value.date };
  if (typeof value.datetime === "string") return { datetime: value.datetime };
  if (typeof value.checkbox === "boolean") return { checkbox: value.checkbox };
  if (Array.isArray(value.options)) {
    return { options: value.options.filter((id): id is string => typeof id === "string") };
  }
  if (Array.isArray(value.users)) {
    return { users: value.users.filter((id): id is string => typeof id === "string") };
  }
  return null;
}

export function selectedOptionIds(value: CollectionValue): string[] {
  return value !== null && "options" in value ? value.options : [];
}

export function selectedUserIds(value: CollectionValue): string[] {
  return value !== null && "users" in value ? value.users : [];
}

export function parseOptionLines(text: string): string[] {
  const seen = new Set<string>();
  const out: string[] = [];
  for (const line of text.split("\n")) {
    const label = line.trim();
    if (label === "" || seen.has(label)) continue;
    seen.add(label);
    out.push(label);
  }
  return out;
}

export const FIELD_KEY_PATTERN = /^[a-z][a-z0-9_]{0,49}$/;

/** Source `suggestedFieldKey`: ASCII slug of the name, or empty (server picks). */
export function suggestedFieldKey(name: string): string {
  const slug = name
    .trim()
    .toLowerCase()
    .replaceAll(/[^a-z0-9_]+/g, "_")
    .replace(/^[^a-z]+/, "")
    .replaceAll(/_+$/g, "")
    .slice(0, 50);
  return FIELD_KEY_PATTERN.test(slug) ? slug : "";
}

export type OptionDraft = { id?: string; label: string; deleted: boolean };

/**
 * PATCH `options` must list every existing option id (renamed or archived);
 * new rows have no id and blank new rows are dropped.
 */
export function optionPatch(
  drafts: readonly OptionDraft[],
): Array<{ id?: string; label: string; deleted?: boolean }> {
  const out: Array<{ id?: string; label: string; deleted?: boolean }> = [];
  for (const draft of drafts) {
    const label = draft.label.trim();
    if (draft.id) {
      out.push({ id: draft.id, label, deleted: draft.deleted });
    } else if (label !== "" && !draft.deleted) {
      out.push({ label });
    }
  }
  return out;
}

// ---- time zone helpers (source lib/datetime.ts) ----

function zonedParts(instant: Date, timeZone: string): Record<string, number> {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone,
    hourCycle: "h23",
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).formatToParts(instant);
  const out: Record<string, number> = {};
  for (const part of parts) {
    if (part.type !== "literal") out[part.type] = Number(part.value);
  }
  return out;
}

function pad(value: number, width = 2): string {
  return String(value).padStart(width, "0");
}

/** ISO instant → `YYYY-MM-DDTHH:mm` wall time in `timeZone`. */
export function isoToZonedLocal(iso: string, timeZone: string): string {
  const instant = new Date(iso);
  if (Number.isNaN(instant.getTime())) return "";
  const p = zonedParts(instant, timeZone);
  return `${pad(p.year, 4)}-${pad(p.month)}-${pad(p.day)}T${pad(p.hour)}:${pad(p.minute)}`;
}

/** `YYYY-MM-DDTHH:mm` wall time in `timeZone` → ISO instant (`Z`), or null. */
export function zonedLocalToIso(local: string, timeZone: string): string | null {
  const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(local.slice(0, 16));
  if (!match) return null;
  const [year, month, day, hour, minute] = match.slice(1).map(Number) as [
    number,
    number,
    number,
    number,
    number,
  ];
  const wall = Date.UTC(year, month - 1, day, hour, minute);
  let guess = wall;
  for (let i = 0; i < 2; i += 1) {
    const p = zonedParts(new Date(guess), timeZone);
    const seen = Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second);
    guess += wall - seen;
  }
  const check = isoToZonedLocal(new Date(guess).toISOString(), timeZone);
  if (check !== local.slice(0, 16)) return null;
  return new Date(guess).toISOString().replace(/\.\d{3}Z$/, "Z");
}

export function todayInTimeZone(timeZone: string, now: Date = new Date()): string {
  const p = zonedParts(now, timeZone);
  return `${pad(p.year, 4)}-${pad(p.month)}-${pad(p.day)}`;
}

/** Editable text for scalar values. */
export function draftFromValue(value: CollectionValue, timeZone: string): string {
  if (value === null) return "";
  if ("text" in value) return value.text;
  if ("number" in value) return String(value.number);
  if ("date" in value) return value.date;
  if ("datetime" in value) return isoToZonedLocal(value.datetime, timeZone);
  return "";
}

/** Scalar draft → value; `"invalid"` when the draft cannot be sent. */
export function valueFromDraft(
  type: string,
  draft: string,
  timeZone: string,
): CollectionValue | "invalid" {
  if (type === "paragraph" ? draft === "" : draft.trim() === "") return null;
  switch (type) {
    case "number": {
      const number = Number(draft);
      return Number.isFinite(number) ? { number } : "invalid";
    }
    case "date":
      return /^\d{4}-\d{2}-\d{2}$/.test(draft) ? { date: draft } : "invalid";
    case "datetime": {
      const instant = zonedLocalToIso(draft, timeZone);
      return instant ? { datetime: instant } : "invalid";
    }
    case "text":
    case "paragraph":
      return { text: draft };
    default:
      return "invalid";
  }
}

export type NamedOption = { id: string; label: string };
export type NamedUser = { userId: string; name: string };

/** Read-only text for a value (cards, calendar). */
export function formatCollectionValue(
  value: CollectionValue,
  options: readonly NamedOption[],
  users: readonly NamedUser[],
  timeZone: string,
  labels: { yes: string; no: string },
): string {
  if (value === null) return "";
  if ("text" in value) return value.text;
  if ("number" in value) return String(value.number);
  if ("date" in value) return value.date;
  if ("datetime" in value) return isoToZonedLocal(value.datetime, timeZone).replace("T", " ");
  if ("checkbox" in value) return value.checkbox ? labels.yes : labels.no;
  if ("options" in value) {
    return value.options
      .map((id) => options.find((option) => option.id === id)?.label ?? id)
      .join(", ");
  }
  return value.users.map((id) => users.find((user) => user.userId === id)?.name ?? id).join(", ");
}

// ---- month calendar ----

export function isMonth(value: string): boolean {
  return /^\d{4}-(0[1-9]|1[0-2])$/.test(value);
}

export function shiftMonth(month: string, delta: number): string {
  const [year, monthNumber] = month.split("-").map(Number) as [number, number];
  const index = year * 12 + (monthNumber - 1) + delta;
  return `${pad(Math.floor(index / 12), 4)}-${pad((index % 12) + 1)}`;
}

/** Query `window` for one month: `to` is the exclusive first day of the next month. */
export function monthWindow(month: string): { from: string; to: string } {
  return { from: `${month}-01`, to: `${shiftMonth(month, 1)}-01` };
}

export type CalendarCell = { date: string; inMonth: boolean };

/** Weeks (7 cells each) covering `month`, starting on `weekStartsOn` (0 = Sunday). */
export function monthGrid(month: string, weekStartsOn: number): CalendarCell[][] {
  const [year, monthNumber] = month.split("-").map(Number) as [number, number];
  const first = new Date(Date.UTC(year, monthNumber - 1, 1));
  const daysInMonth = new Date(Date.UTC(year, monthNumber, 0)).getUTCDate();
  const lead = (first.getUTCDay() - weekStartsOn + 7) % 7;
  const total = Math.ceil((lead + daysInMonth) / 7) * 7;
  const weeks: CalendarCell[][] = [];
  for (let index = 0; index < total; index += 1) {
    const day = new Date(Date.UTC(year, monthNumber - 1, 1 - lead + index));
    const date = day.toISOString().slice(0, 10);
    if (index % 7 === 0) weeks.push([]);
    weeks[weeks.length - 1]!.push({ date, inMonth: date.slice(0, 7) === month });
  }
  return weeks;
}

export const SET_FIELD_TYPES: readonly string[] = [
  "select",
  "multi_select",
  "checkboxes",
  "labels",
  "user",
  "user_multi",
];

/**
 * `equals` custom-filter value for a field type (server `view_query.rs`):
 * numbers for `number`, booleans for `checkbox`, option/user ids for set
 * types, ISO strings for dates. `null` when the draft cannot be sent.
 */
export function customEqualsValue(
  type: string,
  raw: string,
  timeZone: string,
): string | number | boolean | null {
  if (raw.trim() === "") return null;
  if (type === "number") {
    const number = Number(raw);
    return Number.isFinite(number) ? number : null;
  }
  if (type === "checkbox") return raw === "true" ? true : raw === "false" ? false : null;
  if (type === "date") return /^\d{4}-\d{2}-\d{2}$/.test(raw) ? raw : null;
  if (type === "datetime") return zonedLocalToIso(raw, timeZone);
  if (type === "text" || type === "paragraph" || SET_FIELD_TYPES.includes(type)) return raw;
  return null;
}
