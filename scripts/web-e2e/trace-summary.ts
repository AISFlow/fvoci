// Digest of one Playwright trace.zip. Tokens, query strings, headers, cookies
// and bodies stay out of the summary. Usage: trace-summary.ts <trace.zip>

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { readZip, type ZipMember } from "./zip.ts";

const PATH_TOKEN = /(\/(?:s|invite|share|invitations|ics)\/)[^/?#\s"'<>]+/g;
const PARAM_TOKEN =
  /(?<![\w.-])([\w.-]*(?:token|code|secret|password|state|ticket|key))=[^&\s"'<>#]+/gi;
const LONG_TOKEN = /[A-Za-z0-9_-]{40,}/g;
const DB_URL = /postgres(?:ql)?:\/\/\S+/g;
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
const KEYDOWN =
  /^(?:keydown|keyup):(?:Home|End|Shift|Enter|Escape|ArrowLeft|ArrowRight|ArrowUp|ArrowDown|PageUp|PageDown):shift=(?:true|false):composing=(?:true|false):keyCode=\d{1,3}(?::microtask)?$/;
const TRANSACTION =
  /^(?:transaction:doc=(?:true|false):selection=(?:true|false)|Yupdate:local=(?:true|false))$/;

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };
type RecordJson = { [key: string]: Json };

function isRecord(value: Json | undefined): value is RecordJson {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function splitLines(text: string): string[] {
  if (text === "") return [];
  const lines = text.split(/\r\n|\n|\r/);
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

function decode(bytes: Uint8Array): string {
  return new TextDecoder("utf-8", { fatal: false }).decode(bytes);
}

function unavailable(reason: string) {
  return { available: false as const, reason };
}

function number(value: Json | undefined): number | null {
  return typeof value === "number" && value >= 0 && value <= 1e12 && Number.isFinite(value)
    ? value
    : null;
}

function integer(value: Json | undefined): number | null {
  return typeof value === "number" &&
    Number.isInteger(value) &&
    value >= 0 &&
    value <= 1_000_000_000
    ? value
    : null;
}

function boolean(value: Json | undefined): boolean | null {
  return typeof value === "boolean" ? value : null;
}

function enumeration(value: Json | undefined, allowed: readonly string[]): string {
  return typeof value === "string" && allowed.includes(value) ? value : "unknown";
}

function stage(value: Json | undefined): string {
  if (
    typeof value === "string" &&
    (DIAGNOSTIC_ACTIONS.includes(value as (typeof DIAGNOSTIC_ACTIONS)[number]) ||
      [
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
      ].includes(value) ||
      TRANSACTION.test(value) ||
      KEYDOWN.test(value))
  )
    return value;
  return "unknown";
}

function owner(value: Json | undefined): Json[] | null {
  if (typeof value !== "string" || value.length > 256) return null;
  let ids: Json;
  try {
    ids = JSON.parse(value) as Json;
  } catch {
    return null;
  }
  if (
    !Array.isArray(ids) ||
    ids.length !== 7 ||
    ids.some((item) => item !== null && integer(item) === null)
  )
    return null;
  return ids;
}

function positions(value: Json | undefined) {
  const record = isRecord(value) ? value : {};
  return { anchor: integer(record.anchor), head: integer(record.head) };
}

function fileType(member: ZipMember): number {
  return (member.externalAttr >>> 16) & 0o170000;
}

export function diagnosticDigest(
  members: ZipMember[],
): { available: false; reason: string } | Record<string, unknown> {
  if (members.length > 4096) return unavailable("member_limit");
  const named = members.filter((item) => item.filename === "test.trace");
  if (named.length !== 1) return unavailable("missing_or_duplicate_test_trace");
  const traceMember = named[0];
  if (!traceMember || traceMember.fileSize > 4 * 1024 * 1024)
    return unavailable("test_trace_size_limit");
  try {
    const lines = splitLines(decode(traceMember.data));
    if (lines.length > 10000) return unavailable("test_event_limit");
    const references: RecordJson[] = [];
    for (const line of lines) {
      const event = JSON.parse(line) as Json;
      if (!isRecord(event)) return unavailable("invalid_test_event");
      const attachments = event.attachments ?? [];
      if (!Array.isArray(attachments) || attachments.length > 32)
        return unavailable("invalid_attachment_list");
      for (const item of attachments) {
        if (isRecord(item) && item.name === DIAGNOSTIC_NAME) references.push(item);
      }
    }
    if (references.length !== 1) return unavailable("missing_or_duplicate_attachment");
    const reference = references[0];
    const path = reference?.file;
    if (
      reference?.contentType !== "application/json" ||
      typeof path !== "string" ||
      !/^attachments\/[a-f0-9]{40,64}$/.test(path)
    ) {
      return unavailable("invalid_attachment_reference");
    }
    const matching = members.filter((item) => item.filename === path);
    const attachment = matching[0];
    if (
      matching.length !== 1 ||
      !attachment ||
      attachment.isDir ||
      (attachment.flagBits & 1) !== 0 ||
      ![0, 0o100000].includes(fileType(attachment))
    ) {
      return unavailable("missing_or_invalid_attachment_member");
    }
    if (attachment.fileSize > 1024 * 1024) return unavailable("attachment_size_limit");
    const data = JSON.parse(decode(attachment.data)) as Json;
    if (!isRecord(data)) return unavailable("invalid_schema");
    return digestData(data);
  } catch {
    return unavailable("invalid_attachment_data");
  }
}

function digestData(data: RecordJson) {
  const keys = ["frames", "critical", "ownerChanges", "observedMismatches"] as const;
  const limits = [512, 2048, 2048, 4096];
  if (
    keys.some((key, index) => {
      const value = data[key];
      return (
        !Array.isArray(value) ||
        value.length > (limits[index] ?? 0) ||
        value.some((item) => !isRecord(item))
      );
    })
  )
    return unavailable("invalid_event_schema_or_limit");
  const boundaries = data.actionBoundaries ?? [];
  if (
    !Array.isArray(boundaries) ||
    boundaries.length > 16 ||
    boundaries.some((item) => !isRecord(item))
  ) {
    return unavailable("invalid_boundary_schema");
  }
  const caretBoundaries = data.caretBoundaries ?? [];
  if (!Array.isArray(caretBoundaries) || caretBoundaries.length > 2)
    return unavailable("invalid_caret_boundary_schema");
  const caretByAction: Record<string, unknown> = {};
  for (const boundary of caretBoundaries) {
    if (
      !isRecord(boundary) ||
      (boundary.stage !== "after-click" && boundary.stage !== "after-End")
    ) {
      return unavailable("invalid_caret_boundary_schema");
    }
    const action = `caret:${boundary.stage}`;
    const native = boundary.native;
    if (
      boundary.ownerRecordStage !== action ||
      action in caretByAction ||
      number(boundary.at) === null ||
      !isRecord(native)
    ) {
      return unavailable("invalid_caret_boundary_schema");
    }
    const endpoints = [native.anchor, native.head];
    if (endpoints.some((endpoint) => !isRecord(endpoint)))
      return unavailable("invalid_caret_boundary_schema");
    const checked: [RecordJson, readonly string[]][] = [[boundary, ["wide", "rich"]]];
    for (const endpoint of endpoints) {
      if (isRecord(endpoint)) checked.push([endpoint, ["inside", "noneditableLeaf"]]);
    }
    for (const [value, fields] of checked) {
      if (
        fields.some((key) => key in value && value[key] !== null && typeof value[key] !== "boolean")
      ) {
        return unavailable("invalid_boolean_schema");
      }
    }
    if (
      endpoints.some(
        (endpoint) =>
          isRecord(endpoint) &&
          "position" in endpoint &&
          endpoint.position !== null &&
          integer(endpoint.position) === null,
      )
    ) {
      return unavailable("invalid_position_schema");
    }
    const endpointView = (key: "anchor" | "head") => {
      const endpoint = native[key];
      const record = isRecord(endpoint) ? endpoint : {};
      return {
        inside: boolean(record.inside),
        noneditableLeaf: boolean(record.noneditableLeaf),
        position: integer(record.position),
      };
    };
    caretByAction[action] = {
      anchor: endpointView("anchor"),
      head: endpointView("head"),
      wide: boolean(boundary.wide),
      rich: boolean(boundary.rich),
    };
  }
  const frames = data.frames as RecordJson[];
  const critical = data.critical as RecordJson[];
  const ownerChanges = data.ownerChanges as RecordJson[];
  const observed = data.observedMismatches as RecordJson[];
  const events = [
    ...frames,
    ...critical,
    ...ownerChanges,
    ...observed,
    ...boundaries.filter(isRecord),
  ];
  for (const key of ["firstObservedState", "firstRetiredEvent"] as const) {
    if (key in data && data[key] !== null) {
      if (!isRecord(data[key])) return unavailable("invalid_first_state_schema");
      events.push(data[key]);
    }
  }
  for (const event of events) {
    if (number(event.at) === null || typeof event.stage !== "string" || event.stage.length > 512) {
      return unavailable("invalid_event_schema");
    }
    if (["owner", "previousOwner"].some((key) => key in event && owner(event[key]) === null)) {
      return unavailable("invalid_owner_schema");
    }
    if (["native", "pm", "focus", "auth"].some((key) => key in event && !isRecord(event[key]))) {
      return unavailable("invalid_snapshot_schema");
    }
    const native = isRecord(event.native) ? event.native : {};
    if (
      "text" in native &&
      native.text !== null &&
      (typeof native.text !== "string" || native.text.length > 4096)
    ) {
      return unavailable("invalid_native_text_schema");
    }
    for (const value of [native.positions, event.nativePositions, event.pm]) {
      if (
        value !== undefined &&
        value !== null &&
        (!isRecord(value) ||
          ["anchor", "head"].some((key) => key in value && integer(value[key]) === null))
      ) {
        return unavailable("invalid_position_schema");
      }
    }
    const checks: [RecordJson, readonly string[]][] = [
      [native, ["inside"]],
      [isRecord(event.pm) ? event.pm : {}, ["empty"]],
      [isRecord(event.focus) ? event.focus : {}, ["editor", "editorEditable", "composing"]],
      [isRecord(event.auth) ? event.auth : {}, ["authenticated", "synced"]],
    ];
    for (const [value, fields] of checks) {
      if (
        fields.some((key) => key in value && value[key] !== null && typeof value[key] !== "boolean")
      ) {
        return unavailable("invalid_boolean_schema");
      }
    }
  }
  if (
    ["updates", "localUpdates"].some((key) => integer(data[key]) === null) ||
    (integer(data.localUpdates) ?? 0) > (integer(data.updates) ?? 0)
  ) {
    return unavailable("invalid_update_counts");
  }
  const totals = data.totals ?? {};
  const dropped = data.dropped ?? {};
  if (!isRecord(totals) || !isRecord(dropped)) return unavailable("invalid_collection_counts");
  const counts: Record<string, unknown> = {};
  for (const key of [...keys, "actionBoundaries"] as const) {
    const retained = key === "actionBoundaries" ? boundaries.length : (data[key] as Json[]).length;
    const total = integer(totals[key]);
    const lost = integer(dropped[key]);
    if (total !== null && lost !== null && total !== retained + lost)
      return unavailable("inconsistent_collection_counts");
    counts[key] = { retained, total, dropped: lost, unknown: total === null || lost === null };
  }
  let first = isRecord(data.firstObservedState)
    ? data.firstObservedState
    : frames.find((frame) => "pm" in frame);
  const baselineId = first?.nativeId;
  const sample = (raw: Json | undefined) => {
    if (!isRecord(raw)) return null;
    const native = isRecord(raw.native) ? raw.native : {};
    const pm = isRecord(raw.pm) ? raw.pm : {};
    const focus = isRecord(raw.focus) ? raw.focus : {};
    const auth = isRecord(raw.auth) ? raw.auth : {};
    const text = native.text;
    const nativeId = raw.nativeId;
    const validId = typeof nativeId === "string" && nativeId.length > 0 && nativeId.length <= 256;
    const bubble = raw.bubble;
    const selection = {
      ...positions(pm),
      empty: boolean(pm.empty),
      type: enumeration(pm.type, ["text", "node", "cell", "all"]),
    };
    const nativePositions = isRecord(native.positions)
      ? native.positions
      : isRecord(raw.nativePositions)
        ? raw.nativePositions
        : undefined;
    return {
      at: number(raw.at),
      stage: stage(raw.stage),
      bindingGeneration: integer(raw.bindingGeneration),
      eventBindingGeneration: integer(raw.eventBindingGeneration),
      retiredEvent: boolean(raw.retiredEvent),
      owner: owner(raw.owner),
      previousOwner: owner(raw.previousOwner),
      unavailable:
        "unavailable" in raw
          ? enumeration(raw.unavailable, ["missing-editor", "destroyed-editor"])
          : null,
      captureUnknown: "unknown" in raw,
      native: {
        inside: boolean(native.inside),
        codePoints: typeof text === "string" ? [...text].length : null,
        equalsKnownFixtureCjkEmoji: typeof text === "string" ? text === "한글과 😀 링크" : null,
        positions: positions(nativePositions),
        mappingUnknown: isRecord(native.positions) && "unknown" in native.positions,
      },
      pm: selection,
      focus: {
        activeTag: enumeration(focus.activeTag, ["BODY", "DIV", "INPUT", "BUTTON", "TEXTAREA"]),
        editor: boolean(focus.editor),
        editorEditable: boolean(focus.editorEditable),
        composing: boolean(focus.composing),
        domEditable: enumeration(focus.domEditable, ["true", "false", "inherit"]),
      },
      auth: {
        authenticated: boolean(auth.authenticated),
        synced: boolean(auth.synced),
        scope: enumeration(auth.scope, ["read-write", "readonly"]),
        status: enumeration(auth.status, ["connected", "connecting", "disconnected"]),
      },
      nativeId: {
        present: validId,
        hash:
          validId && typeof nativeId === "string"
            ? createHash("sha256").update(nativeId).digest("hex").slice(0, 16)
            : null,
        sameAsFirst: validId && typeof baselineId === "string" ? nativeId === baselineId : null,
      },
      updates: integer(raw.updates),
      localUpdates: integer(raw.localUpdates),
      generationUpdates: integer(raw.generationUpdates),
      generationLocalUpdates: integer(raw.generationLocalUpdates),
      retiredUpdate: boolean(raw.retiredUpdate),
      bubble: {
        present: isRecord(bubble),
        visibility: isRecord(bubble) ? enumeration(bubble.visibility, ["visible", "hidden"]) : null,
      },
      dialog: boolean(raw.dialog),
      caret: typeof raw.stage === "string" ? (caretByAction[raw.stage] ?? null) : null,
    };
  };
  const actions: Record<string, ReturnType<typeof sample>> = {};
  for (const action of DIAGNOSTIC_ACTIONS) {
    actions[action] = sample(
      [...boundaries.filter(isRecord), ...critical].find((item) => item.stage === action),
    );
  }
  const causal = critical.filter(
    (item) =>
      stage(item.stage).startsWith("keydown:Home:") ||
      stage(item.stage).startsWith("keyup:Home:") ||
      stage(item.stage).startsWith("transaction:") ||
      stage(item.stage).startsWith("provider:"),
  );
  const mismatch = observed[0];
  const mismatchAt = mismatch ? number(mismatch.at) : null;
  const home = causal
    .flatMap((item, index) =>
      stage(item.stage).startsWith("keydown:Home:") || stage(item.stage).startsWith("keyup:Home:")
        ? [index]
        : [],
    )
    .slice(0, 4);
  const near = causal
    .flatMap((item, index) => {
      const at = number(item.at);
      return stage(item.stage).startsWith("transaction:") &&
        mismatchAt !== null &&
        at !== null &&
        Math.abs(at - mismatchAt) <= 25
        ? [index]
        : [];
    })
    .slice(0, 3);
  const admission = causal
    .flatMap((item, index) => (stage(item.stage).startsWith("provider:") ? [index] : []))
    .slice(0, 2);
  const selected = [
    ...new Set([
      ...home,
      ...near,
      ...admission,
      ...Array.from({ length: Math.min(1, causal.length) }, (_, index) => index),
      ...Array.from(
        { length: Math.min(2, causal.length) },
        (_, index) => causal.length - Math.min(2, causal.length) + index,
      ),
    ]),
  ].sort((a, b) => a - b);
  const result = {
    available: true,
    counts,
    updates: data.updates,
    localUpdates: data.localUpdates,
    bindingGeneration: integer(data.bindingGeneration),
    firstObservedState: sample(first),
    actions,
    firstRetiredEvent: sample(
      isRecord(data.firstRetiredEvent) ? data.firstRetiredEvent : undefined,
    ),
    missingActions: DIAGNOSTIC_ACTIONS.filter((action) => actions[action] === null),
    firstIdentityChange: sample(ownerChanges[0]),
    firstMismatch: sample(mismatch),
    causalSamples: selected.map((index) => sample(causal[index])),
    causalSampleTotal: causal.length,
    causalSamplesTruncated: causal.length > selected.length,
    tail: frames.slice(-6).map((frame) => sample(frame)),
    tailTruncated: frames.length > 6,
    earlyBindingNotCaptured: true,
    navigationContinuity: "current-document-only",
    captureErrorsRetained: critical.filter((item) => "unknown" in item).length,
  };
  return encoded(result).length <= 49152 ? result : unavailable("output_size_limit");
}

function encoded(value: unknown): string {
  return JSON.stringify(value).replace(
    /[^\u0000-\u007f]/g,
    (char) => `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`,
  );
}

export function redact(text: string): string {
  return text
    .replace(DB_URL, "postgres://redacted")
    .replace(PATH_TOKEN, "$1<redacted>")
    .replace(PARAM_TOKEN, "$1=<redacted>")
    .replace(LONG_TOKEN, "<redacted>");
}

export function shortUrl(url: string): string {
  let parts: URL;
  try {
    parts = new URL(url);
  } catch {
    return "<unparsable url>";
  }
  const host = parts.hostname || "";
  const prefix =
    host === "127.0.0.1" || host === "localhost" || host === "::1" || host === ""
      ? ""
      : `${parts.protocol}//${host}`;
  let path = redact(prefix + (parts.pathname || "/"));
  if (parts.search) path += "?…";
  return path.slice(0, 120);
}

function clip(text: string, limit = 600): string {
  const redacted = redact(String(text));
  return redacted.length <= limit ? redacted : `${redacted.slice(0, limit)}…`;
}

export function summarizeMembers(members: ZipMember[]): string {
  const diagnostic = diagnosticDigest(members);
  const rows: [number, string, string][] = [];
  for (const name of members.map((item) => item.filename).sort()) {
    if (!name.endsWith(".network")) continue;
    const member = members.find((item) => item.filename === name);
    if (!member) continue;
    for (const line of splitLines(decode(member.data))) {
      let entry: Json;
      try {
        entry = JSON.parse(line) as Json;
      } catch {
        continue;
      }
      if (!isRecord(entry)) continue;
      const snap =
        entry.type === "resource-snapshot" && isRecord(entry.snapshot) ? entry.snapshot : null;
      if (!snap) continue;
      const request = isRecord(snap.request) ? snap.request : {};
      const response = isRecord(snap.response) ? snap.response : {};
      const status = typeof response.status === "number" ? response.status : 0;
      const failure = typeof response._failureText === "string" ? response._failureText : "";
      const started = typeof snap.startedDateTime === "string" ? snap.startedDateTime : "";
      const wall = started.slice(11, 23);
      const method = typeof request.method === "string" ? request.method : "?";
      const kind = typeof snap._resourceType === "string" ? snap._resourceType : "-";
      const url = typeof request.url === "string" ? request.url : "";
      const time = typeof snap.time === "number" ? snap.time : -1;
      const monotonic = typeof snap._monotonicTime === "number" ? snap._monotonicTime : 0;
      rows.push([
        monotonic,
        "request",
        `${wall} ${method} ${status > 0 ? status : "ERR"} ${kind} ${shortUrl(url)} ${time.toFixed(0)}ms ${clip(failure, 120)}`.trimEnd(),
      ]);
    }
  }
  for (const name of members.map((item) => item.filename).sort()) {
    if (!name.endsWith(".trace") || name === "test.trace") continue;
    const member = members.find((item) => item.filename === name);
    if (!member) continue;
    for (const line of splitLines(decode(member.data))) {
      let event: Json;
      try {
        event = JSON.parse(line) as Json;
      } catch {
        continue;
      }
      if (!isRecord(event)) continue;
      const when = typeof event.time === "number" ? event.time : 0;
      if (
        event.type === "console" &&
        (event.messageType === "error" || event.messageType === "warning")
      ) {
        const location = isRecord(event.location) ? event.location : {};
        const where =
          typeof location.url === "string" && location.url ? shortUrl(location.url) : "";
        rows.push([
          when,
          "console",
          `${String(event.messageType)} ${clip(typeof event.text === "string" ? event.text : "")} ${where}`.trimEnd(),
        ]);
      } else if (event.type === "event" && event.method === "pageError") {
        const params = isRecord(event.params) ? event.params : {};
        const error =
          isRecord(params.error) && isRecord(params.error.error) ? params.error.error : {};
        const message =
          typeof error.stack === "string"
            ? error.stack
            : `${typeof error.name === "string" ? error.name : "Error"}: ${typeof error.message === "string" ? error.message : String(params.error ?? "")}`;
        rows.push([
          when,
          "pageerror",
          clip(
            String(message)
              .split(/\r\n|\n|\r/)
              .slice(0, 12)
              .join("\n      "),
            2000,
          ),
        ]);
      } else if (event.type === "event" && event.method === "crash") {
        rows.push([when, "crash", "page crashed"]);
      }
    }
  }
  const lines = [
    "browser summary: headers, cookies, bodies and query strings omitted; tokens redacted",
    "columns: +ms since first entry, kind, then for requests: wall clock UTC, method, status, type, path, duration",
  ];
  if (!rows.length) lines.push("(no requests, console warnings/errors or page errors recorded)");
  const base = rows.reduce((min, row) => Math.min(min, row[0]), rows[0]?.[0] ?? 0);
  let ordered = [...rows].sort((left, right) => left[0] - right[0]);
  if (ordered.length > MAX_LINES) {
    lines.push(`… ${ordered.length - MAX_LINES} earlier entries omitted`);
    ordered = ordered.slice(-MAX_LINES);
  }
  for (const [when, kind, text] of ordered) {
    lines.push(`+${(when - base).toFixed(1).padStart(9, " ")} ${kind.padEnd(9, " ")} ${text}`);
  }
  lines.push(`w3-template-diagnostic ${encoded(diagnostic)}`);
  return lines.join("\n") + "\n";
}

export function summarizeFile(path: string): string {
  return summarizeMembers(readZip(readFileSync(path)));
}

if (import.meta.main) {
  const path = process.argv[2];
  if (!path) {
    console.error("usage: trace-summary.ts <trace.zip>");
    process.exit(2);
  }
  try {
    process.stdout.write(summarizeFile(path));
  } catch (error) {
    console.error(error instanceof Error ? error.name : "trace summary failed");
    process.exit(1);
  }
}
