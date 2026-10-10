// Digest of one Playwright trace.zip that CI may upload (web-e2e-run-group.sh).
//
// Prints each request's method, status, resource type and path (no headers,
// cookies, bodies or query strings), browser console warnings/errors and page
// errors. Share, invite, invitation and ICS path tokens, parameters whose key
// ends in token/code/secret/password/state/ticket/key, long base64url runs and
// postgres URLs are redacted. The raw trace (headers, cookies, DOM, bodies)
// stays local. Usage: bun tools/web-e2e/trace-summary.ts <trace.zip>
// Differences from the replaced Python script are in the migration commit message.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import process from "node:process";
import { unzipSync } from "fflate";
import JSZip from "jszip";
import {
  JsonFloat,
  PY_SPACE,
  codePointLength,
  compareCodePoints,
  display,
  formatFixed,
  get,
  has,
  isDict,
  numeric,
  parseJson,
  pyJsonDumps,
  pyRstrip,
  pySplitlines,
  sliceCodePoints,
  truthy,
  type Dict,
} from "./compat.ts";

const PATH_TOKEN = new RegExp(`(/(?:s|invite|share|invitations|ics)/)[^/?#${PY_SPACE}"'<>]+`, "gu");
// Python matched the key words case-insensitively, which also folds İ/ı to i,
// ſ to s and the Kelvin sign to k; the classes keep exactly those matches.
const FOLD: Record<string, string> = { i: "iI\u0130\u0131", k: "kK\u212a", s: "sS\u017f" };
const caseless = (word: string) =>
  Array.from(word)
    .map((char) => `[${FOLD[char] ?? char + char.toUpperCase()}]`)
    .join("");
const KEY_WORDS = ["token", "code", "secret", "password", "state", "ticket", "key"]
  .map(caseless)
  .join("|");
const WORD = "\\p{L}\\p{N}_";
// The lookbehind anchors a match at the start of its key, which keeps a
// long run of key characters without "=" linear instead of quadratic.
const PARAM_TOKEN = new RegExp(
  `(?<![${WORD}.-])([${WORD}.-]*(?:${KEY_WORDS}))=[^&${PY_SPACE}"'<>#]+`,
  "gu",
);
const LONG_TOKEN = /[A-Za-z0-9_-]{40,}/g;
const DB_URL = new RegExp(`postgres(?:ql)?://[^${PY_SPACE}]+`, "gu");
const MAX_LINES = 2000;
const DIAGNOSTIC_NAME = "w3-template-native-selection-observation.json";
const DIAGNOSTIC_ACTIONS = [
  "openDoc:original",
  "caret:after-click",
  "caret:after-End",
  "ShiftHome:original-return",
  "bubble:original-visible",
  "popup:original-open-focus",
  "compositionEnter:original-no-link",
  "Cancel:original-native-text",
  "Apply:original-selected-text",
  "save:original",
  "finally",
] as const;
const FIXED_STAGES: readonly string[] = [
  ...DIAGNOSTIC_ACTIONS,
  "frame",
  "provider:authenticated",
  "provider:status",
  "provider:synced",
  "selectionchange:",
  "selectionchange::microtask",
  "focusin:",
  "focusin::microtask",
  "focusout:",
  "focusout::microtask",
  "pointerdown:",
  "pointerdown::microtask",
  "pagehide:",
  "pagehide::microtask",
];
const TRANSACTION_STAGE =
  /^(?:transaction:doc=(?:true|false):selection=(?:true|false)|Yupdate:local=(?:true|false))$/u;
// Python's \d is any Unicode decimal digit.
const KEY_STAGE =
  /^(?:keydown|keyup):(?:Home|End|Shift|Enter|Escape|ArrowLeft|ArrowRight|ArrowUp|ArrowDown|PageUp|PageDown):shift=(?:true|false):composing=(?:true|false):keyCode=\p{Nd}{1,3}(?::microtask)?$/u;
const ATTACHMENT_PATH = /^attachments\/[a-f0-9]{40,64}$/;

/** One central-directory record, as fflate's reader reports it. */
export interface Member {
  index: number;
  name: string;
  originalSize: number;
}

/** The archive's central directory, in order and with duplicate names. */
export function listMembers(archive: Uint8Array): Member[] {
  const members: Member[] = [];
  unzipSync(archive, {
    filter: (file) => {
      members.push({ index: members.length, name: file.name, originalSize: file.originalSize });
      return false;
    },
  });
  return members;
}

/** Inflate exactly the given records; a later record of one name replaces an earlier one. */
function readMembers(
  archive: Uint8Array,
  wanted: (member: Member) => boolean,
): Map<string, Uint8Array> {
  const out = new Map<string, Uint8Array>();
  let index = 0;
  const files = unzipSync(archive, {
    filter: (file) => {
      const keep = wanted({ index, name: file.name, originalSize: file.originalSize });
      index += 1;
      return keep;
    },
  });
  for (const name of Object.keys(files)) {
    const data = files[name];
    if (data) out.set(name, data);
  }
  return out;
}

function readMember(archive: Uint8Array, member: Member): Uint8Array {
  const data = readMembers(archive, (item) => item.index === member.index).get(member.name);
  if (!data) throw new Error("member not read");
  return data;
}

/** Attachment records whose entry type JSZip reports as a plain file; null if JSZip refuses the archive. */
export type PlainFiles = ReadonlySet<string> | null;

/**
 * Python refused an attachment record that is a directory, encrypted, or whose
 * Unix mode is neither unset nor a regular file. fflate does not expose those
 * header fields, so JSZip (which rejects encrypted archives) reports them.
 */
export async function plainAttachmentFiles(archive: Uint8Array): Promise<PlainFiles> {
  let zip: JSZip;
  try {
    zip = await JSZip.loadAsync(archive);
  } catch {
    return null;
  }
  const plain = new Set<string>();
  for (const [name, entry] of Object.entries(zip.files)) {
    if (!name.startsWith("attachments/") || entry.dir) continue;
    const mode = typeof entry.unixPermissions === "number" ? entry.unixPermissions & 0o170000 : 0;
    if (mode === 0 || mode === 0o100000) plain.add(name);
  }
  return plain;
}

/** `bytes.splitlines()`: LF, CR and CRLF only. */
function splitByteLines(data: Uint8Array): Uint8Array[] {
  const lines: Uint8Array[] = [];
  let start = 0;
  for (let at = 0; at < data.length; at += 1) {
    const byte = data[at];
    if (byte !== 0x0a && byte !== 0x0d) continue;
    lines.push(data.subarray(start, at));
    if (byte === 0x0d && data[at + 1] === 0x0a) at += 1;
    start = at + 1;
  }
  if (start < data.length) lines.push(data.subarray(start));
  return lines;
}

/** `json.loads(bytes)` for UTF-8 input (an initial BOM is skipped, as utf-8-sig). */
function parseJsonBytes(data: Uint8Array): unknown {
  return parseJson(new TextDecoder("utf-8", { fatal: true }).decode(data));
}

type Unavailable = { available: false; reason: string };
const unavailable = (reason: string): Unavailable => ({ available: false, reason });

function number(value: unknown): number | JsonFloat | null {
  const amount =
    typeof value === "number" || value instanceof JsonFloat ? numeric(value) : undefined;
  return amount !== undefined && Number.isFinite(amount) && amount >= 0 && amount <= 1e12
    ? (value as number | JsonFloat)
    : null;
}

function integer(value: unknown): number | null {
  return typeof value === "number" &&
    Number.isInteger(value) &&
    value >= 0 &&
    value <= 1_000_000_000
    ? value
    : null;
}

function boolean(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

function choice(value: unknown, allowed: readonly string[]): string {
  return typeof value === "string" && allowed.includes(value) ? value : "unknown";
}

function stage(value: unknown): string {
  if (typeof value !== "string") return "unknown";
  if (FIXED_STAGES.includes(value) || TRANSACTION_STAGE.test(value) || KEY_STAGE.test(value)) {
    return value;
  }
  return "unknown";
}

function owner(value: unknown): (number | null)[] | null {
  if (typeof value !== "string" || codePointLength(value) > 256) return null;
  let ids: unknown;
  try {
    ids = parseJson(value);
  } catch {
    return null;
  }
  if (!Array.isArray(ids) || ids.length !== 7) return null;
  if (ids.some((id) => id !== null && integer(id) === null)) return null;
  return ids as (number | null)[];
}

function positions(value: unknown): { anchor: number | null; head: number | null } {
  const source = isDict(value) ? value : {};
  return { anchor: integer(get(source, "anchor")), head: integer(get(source, "head")) };
}

/** `str.encode("utf-8", "surrogatepass")`: a lone surrogate keeps its 3-byte form. */
function surrogatePassUtf8(text: string): Buffer {
  const bytes: number[] = [];
  for (const char of text) {
    const point = char.codePointAt(0) ?? 0;
    if (point >= 0xd800 && point <= 0xdfff) {
      bytes.push(0xe0 | (point >> 12), 0x80 | ((point >> 6) & 0x3f), 0x80 | (point & 0x3f));
    } else {
      bytes.push(...Buffer.from(char, "utf8"));
    }
  }
  return Buffer.from(bytes);
}

const startsWithAny = (text: string, prefixes: readonly string[]) =>
  prefixes.some((prefix) => text.startsWith(prefix));

function dicts(value: unknown): Dict[] {
  return Array.isArray(value) ? value.filter(isDict) : [];
}

/**
 * Read this fixture's one named attachment; export only an explicit allowlist.
 *
 * Raw trace/attachment content stays on the runner. All invalid-input outcomes
 * are fixed enums, never exception messages or a raw-content fallback.
 */
export function diagnosticDigest(
  archive: Uint8Array,
  members: Member[],
  plainFiles: PlainFiles,
): Dict | Unavailable {
  if (members.length > 4096) return unavailable("member_limit");
  const named = members.filter((item) => item.name === "test.trace");
  const testTrace = named[0];
  if (named.length !== 1 || !testTrace) return unavailable("missing_or_duplicate_test_trace");
  if (testTrace.originalSize > 4 * 1024 * 1024) return unavailable("test_trace_size_limit");
  let data: unknown;
  try {
    const lines = splitByteLines(readMember(archive, testTrace));
    if (lines.length > 10000) return unavailable("test_event_limit");
    const references: Dict[] = [];
    for (const line of lines) {
      const event = parseJsonBytes(line);
      if (!isDict(event)) return unavailable("invalid_test_event");
      const attachments = has(event, "attachments") ? event.attachments : [];
      if (!Array.isArray(attachments) || attachments.length > 32) {
        return unavailable("invalid_attachment_list");
      }
      references.push(
        ...attachments.filter(
          (item): item is Dict => isDict(item) && get(item, "name") === DIAGNOSTIC_NAME,
        ),
      );
    }
    const reference = references[0];
    if (references.length !== 1 || !reference)
      return unavailable("missing_or_duplicate_attachment");
    const path = get(reference, "file");
    if (
      get(reference, "contentType") !== "application/json" ||
      typeof path !== "string" ||
      !ATTACHMENT_PATH.test(path)
    ) {
      return unavailable("invalid_attachment_reference");
    }
    const matching = members.filter((item) => item.name === path);
    const member = matching[0];
    if (matching.length !== 1 || !member || !plainFiles?.has(path))
      return unavailable("missing_or_invalid_attachment_member");
    if (member.originalSize > 1024 * 1024) return unavailable("attachment_size_limit");
    data = parseJsonBytes(readMember(archive, member));
  } catch {
    return unavailable("invalid_attachment_data");
  }
  if (!isDict(data)) return unavailable("invalid_schema");
  const keys = ["frames", "critical", "ownerChanges", "observedMismatches"] as const;
  const limits = [512, 2048, 2048, 4096];
  if (
    keys.some((key, index) => {
      const list = get(data, key);
      return !Array.isArray(list) || list.length > (limits[index] ?? 0) || !list.every(isDict);
    })
  ) {
    return unavailable("invalid_event_schema_or_limit");
  }
  const [frames, critical, ownerChanges, observedMismatches] = keys.map((key) => dicts(data[key]));
  if (!frames || !critical || !ownerChanges || !observedMismatches) {
    return unavailable("invalid_event_schema_or_limit");
  }
  const boundariesValue = has(data, "actionBoundaries") ? data.actionBoundaries : [];
  if (
    !Array.isArray(boundariesValue) ||
    boundariesValue.length > 16 ||
    !boundariesValue.every(isDict)
  ) {
    return unavailable("invalid_boundary_schema");
  }
  const boundaries = boundariesValue;
  const caretBoundaries = has(data, "caretBoundaries") ? data.caretBoundaries : [];
  if (!Array.isArray(caretBoundaries) || caretBoundaries.length > 2) {
    return unavailable("invalid_caret_boundary_schema");
  }
  const caretByAction = new Map<string, Dict>();
  for (const boundary of caretBoundaries as unknown[]) {
    if (!isDict(boundary)) return unavailable("invalid_caret_boundary_schema");
    const caretStage = get(boundary, "stage");
    if (caretStage !== "after-click" && caretStage !== "after-End") {
      return unavailable("invalid_caret_boundary_schema");
    }
    const action = `caret:${caretStage}`;
    const native = get(boundary, "native");
    if (
      get(boundary, "ownerRecordStage") !== action ||
      caretByAction.has(action) ||
      number(get(boundary, "at")) === null ||
      !isDict(native)
    ) {
      return unavailable("invalid_caret_boundary_schema");
    }
    const anchor = get(native, "anchor");
    const head = get(native, "head");
    if (!isDict(anchor) || !isDict(head)) return unavailable("invalid_caret_boundary_schema");
    const checks: [Dict, string[]][] = [
      [boundary, ["wide", "rich"]],
      [anchor, ["inside", "noneditableLeaf"]],
      [head, ["inside", "noneditableLeaf"]],
    ];
    if (checks.some(([value, fields]) => fields.some((key) => !optionalBoolean(value, key)))) {
      return unavailable("invalid_boolean_schema");
    }
    if (
      [anchor, head].some(
        (endpoint) =>
          has(endpoint, "position") &&
          endpoint.position !== null &&
          integer(endpoint.position) === null,
      )
    ) {
      return unavailable("invalid_position_schema");
    }
    const endpoint = (value: Dict) => ({
      inside: boolean(get(value, "inside")),
      noneditableLeaf: boolean(get(value, "noneditableLeaf")),
      position: integer(get(value, "position")),
    });
    caretByAction.set(action, {
      anchor: endpoint(anchor),
      head: endpoint(head),
      wide: boolean(get(boundary, "wide")),
      rich: boolean(get(boundary, "rich")),
    });
  }
  const events: Dict[] = [
    ...frames,
    ...critical,
    ...ownerChanges,
    ...observedMismatches,
    ...boundaries,
  ];
  for (const key of ["firstObservedState", "firstRetiredEvent"]) {
    if (has(data, key) && data[key] !== null) {
      const value = data[key];
      if (!isDict(value)) return unavailable("invalid_first_state_schema");
      events.push(value);
    }
  }
  for (const event of events) {
    const eventStage = get(event, "stage");
    if (
      number(get(event, "at")) === null ||
      typeof eventStage !== "string" ||
      codePointLength(eventStage) > 512
    ) {
      return unavailable("invalid_event_schema");
    }
    if (["owner", "previousOwner"].some((key) => has(event, key) && owner(event[key]) === null)) {
      return unavailable("invalid_owner_schema");
    }
    if (["native", "pm", "focus", "auth"].some((key) => has(event, key) && !isDict(event[key]))) {
      return unavailable("invalid_snapshot_schema");
    }
    const snapshot = (key: string): Dict => {
      const value = get(event, key);
      return isDict(value) ? value : {};
    };
    const native = snapshot("native");
    const text = get(native, "text");
    if (
      has(native, "text") &&
      text !== null &&
      (typeof text !== "string" || codePointLength(text) > 4096)
    ) {
      return unavailable("invalid_native_text_schema");
    }
    for (const value of [
      get(native, "positions"),
      get(event, "nativePositions"),
      get(event, "pm"),
    ]) {
      if (
        value !== null &&
        (!isDict(value) ||
          ["anchor", "head"].some((key) => has(value, key) && integer(value[key]) === null))
      ) {
        return unavailable("invalid_position_schema");
      }
    }
    const checks: [Dict, string[]][] = [
      [native, ["inside"]],
      [snapshot("pm"), ["empty"]],
      [snapshot("focus"), ["editor", "editorEditable", "composing"]],
      [snapshot("auth"), ["authenticated", "synced"]],
    ];
    if (checks.some(([value, fields]) => fields.some((key) => !optionalBoolean(value, key)))) {
      return unavailable("invalid_boolean_schema");
    }
  }
  const updates = integer(get(data, "updates"));
  const localUpdates = integer(get(data, "localUpdates"));
  if (updates === null || localUpdates === null || localUpdates > updates) {
    return unavailable("invalid_update_counts");
  }
  const totals = has(data, "totals") ? data.totals : {};
  const dropped = has(data, "dropped") ? data.dropped : {};
  if (!isDict(totals) || !isDict(dropped)) return unavailable("invalid_collection_counts");
  const retainedLists: Record<string, Dict[]> = {
    frames,
    critical,
    ownerChanges,
    observedMismatches,
    actionBoundaries: boundaries,
  };
  const counts: Dict = {};
  for (const [key, list] of Object.entries(retainedLists)) {
    const retained = list.length;
    const total = integer(get(totals, key));
    const lost = integer(get(dropped, key));
    if (total !== null && lost !== null && total !== retained + lost) {
      return unavailable("inconsistent_collection_counts");
    }
    counts[key] = { retained, total, dropped: lost, unknown: total === null || lost === null };
  }

  const firstValue = get(data, "firstObservedState");
  const first = isDict(firstValue) ? firstValue : (frames.find((x) => has(x, "pm")) ?? null);
  const baselineId = first ? get(first, "nativeId") : null;

  const sample = (raw: unknown): Dict | null => {
    if (!isDict(raw)) return null;
    const part = (key: string): Dict => {
      const value = get(raw, key);
      return isDict(value) ? value : {};
    };
    const native = part("native");
    const pm = part("pm");
    const focus = part("focus");
    const auth = part("auth");
    const text = get(native, "text");
    const nativeId = get(raw, "nativeId");
    const validId =
      typeof nativeId === "string" && nativeId.length > 0 && codePointLength(nativeId) <= 256;
    const bubble = get(raw, "bubble");
    const nativePositions = get(native, "positions");
    return {
      at: number(get(raw, "at")),
      stage: stage(get(raw, "stage")),
      bindingGeneration: integer(get(raw, "bindingGeneration")),
      eventBindingGeneration: integer(get(raw, "eventBindingGeneration")),
      retiredEvent: boolean(get(raw, "retiredEvent")),
      owner: owner(get(raw, "owner")),
      previousOwner: owner(get(raw, "previousOwner")),
      unavailable: has(raw, "unavailable")
        ? choice(raw.unavailable, ["missing-editor", "destroyed-editor"])
        : null,
      captureUnknown: has(raw, "unknown"),
      native: {
        inside: boolean(get(native, "inside")),
        codePoints: typeof text === "string" ? codePointLength(text) : null,
        equalsKnownFixtureCjkEmoji: typeof text === "string" ? text === "한글과 😀 링크" : null,
        positions: positions(
          has(native, "positions") ? nativePositions : get(raw, "nativePositions"),
        ),
        mappingUnknown: isDict(nativePositions) && has(nativePositions, "unknown"),
      },
      pm: {
        ...positions(pm),
        empty: boolean(get(pm, "empty")),
        type: choice(get(pm, "type"), ["text", "node", "cell", "all"]),
      },
      focus: {
        activeTag: choice(get(focus, "activeTag"), ["BODY", "DIV", "INPUT", "BUTTON", "TEXTAREA"]),
        editor: boolean(get(focus, "editor")),
        editorEditable: boolean(get(focus, "editorEditable")),
        composing: boolean(get(focus, "composing")),
        domEditable: choice(get(focus, "domEditable"), ["true", "false", "inherit"]),
      },
      auth: {
        authenticated: boolean(get(auth, "authenticated")),
        synced: boolean(get(auth, "synced")),
        scope: choice(get(auth, "scope"), ["read-write", "readonly"]),
        status: choice(get(auth, "status"), ["connected", "connecting", "disconnected"]),
      },
      nativeId: {
        present: validId,
        hash: validId
          ? createHash("sha256").update(surrogatePassUtf8(nativeId)).digest("hex").slice(0, 16)
          : null,
        sameAsFirst: validId && typeof baselineId === "string" ? nativeId === baselineId : null,
      },
      updates: integer(get(raw, "updates")),
      localUpdates: integer(get(raw, "localUpdates")),
      generationUpdates: integer(get(raw, "generationUpdates")),
      generationLocalUpdates: integer(get(raw, "generationLocalUpdates")),
      retiredUpdate: boolean(get(raw, "retiredUpdate")),
      bubble: {
        present: isDict(bubble),
        visibility: isDict(bubble)
          ? choice(get(bubble, "visibility"), ["visible", "hidden"])
          : null,
      },
      dialog: boolean(get(raw, "dialog")),
      caret: caretByAction.get(get(raw, "stage") as string) ?? null,
    };
  };

  const actions: Dict = {};
  for (const action of DIAGNOSTIC_ACTIONS) {
    actions[action] = sample(
      [...boundaries, ...critical].find((x) => get(x, "stage") === action) ?? null,
    );
  }
  const stageOf = (x: Dict) => stage(get(x, "stage"));
  const causal = critical.filter((x) =>
    startsWithAny(stageOf(x), ["keydown:Home:", "keyup:Home:", "transaction:", "provider:"]),
  );
  const mismatch = observedMismatches[0] ?? null;
  const mismatchAt = mismatch ? number(get(mismatch, "at")) : null;
  const indexes = (keep: (x: Dict) => boolean, limit: number) =>
    causal.flatMap((x, index) => (keep(x) ? [index] : [])).slice(0, limit);
  const home = indexes((x) => startsWithAny(stageOf(x), ["keydown:Home:", "keyup:Home:"]), 4);
  const near = indexes((x) => {
    const at = number(get(x, "at"));
    return (
      stageOf(x).startsWith("transaction:") &&
      mismatchAt !== null &&
      at !== null &&
      Math.abs((numeric(at) ?? 0) - (numeric(mismatchAt) ?? 0)) <= 25
    );
  }, 3);
  const admission = indexes((x) => stageOf(x).startsWith("provider:"), 2);
  const edges = [
    ...(causal.length > 0 ? [0] : []),
    ...[causal.length - 2, causal.length - 1].filter((index) => index >= 0),
  ];
  const selected = [...new Set([...home, ...near, ...admission, ...edges])].sort((a, b) => a - b);
  const result: Dict = {
    available: true,
    counts,
    updates,
    localUpdates,
    bindingGeneration: integer(get(data, "bindingGeneration")),
    firstObservedState: sample(first),
    actions,
    firstRetiredEvent: sample(get(data, "firstRetiredEvent")),
    missingActions: Object.entries(actions)
      .filter(([, value]) => value === null)
      .map(([action]) => action),
    firstIdentityChange: sample(ownerChanges[0] ?? null),
    firstMismatch: sample(mismatch),
    causalSamples: selected.map((index) => sample(causal[index])),
    causalSampleTotal: causal.length,
    causalSamplesTruncated: causal.length > selected.length,
    tail: frames.slice(-6).map(sample),
    tailTruncated: frames.length > 6,
    earlyBindingNotCaptured: true,
    navigationContinuity: "current-document-only",
    captureErrorsRetained: critical.filter((x) => has(x, "unknown")).length,
  };
  // Bounded as Python's default json.dumps (", " / ": ", ASCII) measured it.
  return Buffer.byteLength(pyJsonDumps(result)) <= 49152
    ? result
    : unavailable("output_size_limit");
}

/** A key that is absent, null or a boolean. */
function optionalBoolean(value: Dict, key: string): boolean {
  return !has(value, key) || value[key] === null || typeof value[key] === "boolean";
}

export function redact(text: string): string {
  return text
    .replace(DB_URL, "postgres://redacted")
    .replace(PATH_TOKEN, (_match, prefix: string) => `${prefix}<redacted>`)
    .replace(PARAM_TOKEN, (_match, key: string) => `${key}=<redacted>`)
    .replace(LONG_TOKEN, "<redacted>");
}

const LOCAL_HOSTS = new Set(["127.0.0.1", "localhost", "::1", ""]);

export function shortUrl(url: unknown): string {
  if (typeof url !== "string") throw new TypeError("url is not a string");
  let path: string;
  if (url === "") {
    path = "/";
  } else {
    let parts: URL;
    try {
      parts = new URL(url);
    } catch {
      return "<unparsable url>";
    }
    const host = parts.hostname.replace(/^\[(.*)\]$/u, "$1");
    const prefix = LOCAL_HOSTS.has(host) ? "" : `${parts.protocol.slice(0, -1)}://${host}`;
    path = redact(prefix + (parts.pathname || "/"));
    if (parts.search !== "") path += "?…";
  }
  return sliceCodePoints(path, 0, 120);
}

export function clip(text: unknown, limit = 600): string {
  const redacted = redact(display(text));
  return codePointLength(redacted) <= limit ? redacted : `${sliceCodePoints(redacted, 0, limit)}…`;
}

/** `dict.get(key, default)` on a value Python would call `.get` on: it must be a dict. */
function field(value: unknown, key: string, fallback: unknown): unknown {
  if (!isDict(value)) throw new TypeError("expected an object");
  return has(value, key) ? value[key] : fallback;
}

/** A value Python subtracts and formats: int, float or bool. */
function time(value: unknown): number {
  const amount = numeric(value);
  if (amount === undefined) throw new TypeError("time is not a number");
  return amount;
}

function status(value: unknown): string {
  if (!truthy(value)) return "ERR";
  if (value === true) return "True";
  const amount =
    typeof value === "number" || value instanceof JsonFloat ? numeric(value) : undefined;
  if (amount === undefined) throw new TypeError("status is not a number");
  return amount > 0 ? display(value) : "ERR";
}

function wallClock(value: unknown): string {
  if (typeof value === "string") return sliceCodePoints(value, 11, 23);
  if (Array.isArray(value)) return display(value.slice(11, 23));
  throw new TypeError("startedDateTime is not a string");
}

type Row = [number, string, string];

const SUMMARIZED_TYPES = new Set(["resource-snapshot", "console", "event"]);

/**
 * JSON records of one member, one per Python line; unparsable lines are skipped.
 * Records of other types are only type-checked, so they skip the int/float reviver.
 */
function jsonLines(archive: Uint8Array, name: string): unknown[] {
  const data = readMembers(archive, (member) => member.name === name).get(name);
  const text = new TextDecoder("utf-8", { ignoreBOM: true }).decode(data);
  return pySplitlines(text).flatMap((line) => {
    try {
      const plain: unknown = JSON.parse(line);
      if (isDict(plain) && !SUMMARIZED_TYPES.has(get(plain, "type") as string)) return [plain];
      return [parseJson(line)];
    } catch {
      return [];
    }
  });
}

export async function summarize(archive: Uint8Array): Promise<string> {
  const members = listMembers(archive);
  const diagnostic = diagnosticDigest(archive, members, await plainAttachmentFiles(archive));
  const names = members.map((member) => member.name).sort(compareCodePoints);
  const rows: Row[] = [];
  // One member inflated at a time, as Python read them.
  for (const name of names) {
    if (!name.endsWith(".network")) continue;
    for (const entry of jsonLines(archive, name)) {
      const snap =
        field(entry, "type", null) === "resource-snapshot" ? field(entry, "snapshot", null) : null;
      if (!truthy(snap)) continue;
      const request = field(snap, "request", {});
      const response = field(snap, "response", {});
      const failure = truthy(field(response, "_failureText", null))
        ? field(response, "_failureText", null)
        : "";
      const text =
        `${wallClock(field(snap, "startedDateTime", ""))} ${display(field(request, "method", "?"))} ` +
        `${status(field(response, "status", 0))} ${display(field(snap, "_resourceType", "-"))} ` +
        `${shortUrl(field(request, "url", ""))} ${formatFixed(time(field(snap, "time", -1)), 0)}ms ` +
        clip(failure, 120);
      rows.push([time(field(snap, "_monotonicTime", 0)), "request", pyRstrip(text)]);
    }
  }
  for (const name of names) {
    if (!name.endsWith(".trace") || name === "test.trace") continue;
    for (const event of jsonLines(archive, name)) {
      const kind = field(event, "type", null);
      const when = field(event, "time", 0);
      const messageType = field(event, "messageType", null);
      if (kind === "console" && (messageType === "error" || messageType === "warning")) {
        const locationValue = field(event, "location", null);
        const location = truthy(locationValue) ? locationValue : {};
        const where = truthy(field(location, "url", null))
          ? shortUrl(field(location, "url", ""))
          : "";
        const text = `${messageType} ${clip(field(event, "text", ""))} ${where}`;
        rows.push([time(when), "console", pyRstrip(text)]);
      } else if (kind === "event" && field(event, "method", null) === "pageError") {
        const paramsValue = field(event, "params", null);
        const params = truthy(paramsValue) ? paramsValue : {};
        const outer = field(params, "error", null);
        const errorValue = field(truthy(outer) ? outer : {}, "error", null);
        const error = truthy(errorValue) ? errorValue : {};
        const stack = field(error, "stack", null);
        const message = truthy(stack)
          ? stack
          : `${display(field(error, "name", "Error"))}: ${display(field(error, "message", field(params, "error", null)))}`;
        const lines = pySplitlines(display(message)).slice(0, 12);
        rows.push([time(when), "pageerror", clip(lines.join("\n      "), 2000)]);
      } else if (kind === "event" && field(event, "method", null) === "crash") {
        rows.push([time(when), "crash", "page crashed"]);
      }
    }
  }

  const out = [
    "browser summary: headers, cookies, bodies and query strings omitted; tokens redacted",
    "columns: +ms since first entry, kind, then for requests: wall clock UTC, method, status, type, path, duration",
  ];
  if (rows.length === 0) out.push("(no requests, console warnings/errors or page errors recorded)");
  const base = rows.reduce((low, row) => Math.min(low, row[0]), rows[0]?.[0] ?? 0);
  let ordered = [...rows].sort((a, b) => a[0] - b[0]);
  // The entries just before the failure matter most: keep the tail.
  if (ordered.length > MAX_LINES) {
    out.push(`… ${String(ordered.length - MAX_LINES)} earlier entries omitted`);
    ordered = ordered.slice(-MAX_LINES);
  }
  for (const [when, kind, text] of ordered) {
    out.push(`+${formatFixed(when - base, 1).padStart(9)} ${kind.padEnd(9)} ${text}`);
  }
  out.push(`w3-template-diagnostic ${pyJsonDumps(diagnostic, ",", ":")}`);
  return out.map((line) => `${line}\n`).join("");
}

if (import.meta.main) {
  const path = process.argv[2];
  if (path === undefined) {
    process.stderr.write("usage: trace-summary.ts <trace.zip>\n");
    process.exit(1);
  }
  let output: string;
  try {
    output = await summarize(readFileSync(path));
  } catch (error) {
    // Only the error class: a message could quote unredacted trace content.
    process.stderr.write(
      `trace-summary: failed (${error instanceof Error ? error.name : "unknown error"})\n`,
    );
    process.exit(1);
  }
  process.stdout.write(output);
}
