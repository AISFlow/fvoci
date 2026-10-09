import { createHash } from "node:crypto";
import { z } from "zod";
import {
  ACTIONS,
  browserReportSchema,
  integerSchema,
  observationSchema,
  reasonSchema,
} from "./typeddiagnostic-schema";
import type { ObservationEvent, Reason } from "./typeddiagnostic-schema";

export function unavailable(reason: Reason) {
  return { available: false as const, reason };
}
const integer = (value: unknown) => {
  const parsed = integerSchema().safeParse(value);
  return parsed.success ? parsed.data : null;
};
const boolean = (value: unknown) => (typeof value === "boolean" ? value : null);
const enumeration = (value: unknown, allowed: readonly string[]) =>
  typeof value === "string" && allowed.includes(value) ? value : "unknown";
const positions = (value: { anchor?: number | null; head?: number | null } | null | undefined) => ({
  anchor: value?.anchor ?? null,
  head: value?.head ?? null,
});
const fixedStages = [
  "frame",
  "provider:authenticated",
  "provider:status",
  "provider:synced",
  ...["selectionchange", "focusin", "focusout", "pointerdown", "pagehide"].flatMap((value) => [
    `${value}:`,
    `${value}::microtask`,
  ]),
];
function stage(value: string) {
  if (
    (ACTIONS as readonly string[]).includes(value) ||
    fixedStages.includes(value) ||
    /^(?:transaction:doc=(?:true|false):selection=(?:true|false)|Yupdate:local=(?:true|false))$/.test(
      value,
    ) ||
    /^(?:keydown|keyup):(?:Home|End|Shift|Enter|Escape|ArrowLeft|ArrowRight|ArrowUp|ArrowDown|PageUp|PageDown):shift=(?:true|false):composing=(?:true|false):keyCode=\d{1,3}(?::microtask)?$/.test(
      value,
    )
  )
    return value;
  return "unknown";
}

export function diagnosticDigest(input: unknown) {
  const parsed = observationSchema.safeParse(input);
  if (!parsed.success) {
    const issue = parsed.error.issues[0];
    const reason = reasonSchema.safeParse(issue?.message);
    if (reason.success) return unavailable(reason.data);
    if (issue?.path.includes("owner") || issue?.path.includes("previousOwner"))
      return unavailable("invalid_owner_schema");
    if (issue?.path[0] === "caretBoundaries") return unavailable("invalid_caret_boundary_schema");
    if (issue?.path[0] === "firstObservedState" || issue?.path[0] === "firstRetiredEvent")
      return unavailable("invalid_first_state_schema");
    return unavailable("invalid_schema");
  }
  const data = parsed.data;
  const counts: Record<
    string,
    { retained: number; total: number | null; dropped: number | null; unknown: boolean }
  > = {};
  for (const key of [
    "frames",
    "critical",
    "ownerChanges",
    "observedMismatches",
    "actionBoundaries",
  ] as const) {
    const retained = data[key].length,
      total = integer(data.totals[key]),
      dropped = integer(data.dropped[key]);
    if (total !== null && dropped !== null && total !== retained + dropped)
      return unavailable("inconsistent_collection_counts");
    counts[key] = { retained, total, dropped, unknown: total === null || dropped === null };
  }
  const first = data.firstObservedState ?? data.frames.find((value) => value.pm !== undefined);
  const baseline = first?.nativeId;
  function sample(raw: ObservationEvent | null | undefined) {
    if (!raw) return null;
    const native = raw.native,
      pm = raw.pm,
      focus = raw.focus,
      auth = raw.auth;
    const id = raw.nativeId,
      text = native?.text;
    const validId = typeof id === "string" && id.length > 0 && Array.from(id).length <= 256;
    const bubble = z.object({ visibility: z.unknown().optional() }).safeParse(raw.bubble);
    const caret = data.caretBoundaries.find((value) => `caret:${value.stage}` === raw.stage);
    return {
      at: raw.at,
      stage: stage(raw.stage),
      bindingGeneration: integer(raw.bindingGeneration),
      eventBindingGeneration: integer(raw.eventBindingGeneration),
      retiredEvent: boolean(raw.retiredEvent),
      owner: raw.owner ?? null,
      previousOwner: raw.previousOwner ?? null,
      unavailable: Object.hasOwn(raw, "unavailable")
        ? enumeration(raw.unavailable, ["missing-editor", "destroyed-editor"])
        : null,
      captureUnknown: Object.hasOwn(raw, "unknown"),
      native: {
        inside: native?.inside ?? null,
        codePoints: typeof text === "string" ? Array.from(text).length : null,
        equalsKnownFixtureCjkEmoji: typeof text === "string" ? text === "한글과 😀 링크" : null,
        positions: positions(
          native && Object.hasOwn(native, "positions") ? native.positions : raw.nativePositions,
        ),
        mappingUnknown: !!native?.positions && Object.hasOwn(native.positions, "unknown"),
      },
      pm: {
        ...positions(pm),
        empty: pm?.empty ?? null,
        type: enumeration(pm?.type, ["text", "node", "cell", "all"]),
      },
      focus: {
        activeTag: enumeration(focus?.activeTag, ["BODY", "DIV", "INPUT", "BUTTON", "TEXTAREA"]),
        editor: focus?.editor ?? null,
        editorEditable: focus?.editorEditable ?? null,
        composing: focus?.composing ?? null,
        domEditable: enumeration(focus?.domEditable, ["true", "false", "inherit"]),
      },
      auth: {
        authenticated: auth?.authenticated ?? null,
        synced: auth?.synced ?? null,
        scope: enumeration(auth?.scope, ["read-write", "readonly"]),
        status: enumeration(auth?.status, ["connected", "connecting", "disconnected"]),
      },
      nativeId: {
        present: validId,
        hash: validId ? createHash("sha256").update(id).digest("hex").slice(0, 16) : null,
        sameAsFirst: validId && typeof baseline === "string" ? id === baseline : null,
      },
      updates: integer(raw.updates),
      localUpdates: integer(raw.localUpdates),
      generationUpdates: integer(raw.generationUpdates),
      generationLocalUpdates: integer(raw.generationLocalUpdates),
      retiredUpdate: boolean(raw.retiredUpdate),
      bubble: {
        present: bubble.success,
        visibility: bubble.success
          ? enumeration(bubble.data.visibility, ["visible", "hidden"])
          : null,
      },
      dialog: boolean(raw.dialog),
      caret: caret
        ? {
            anchor: {
              inside: caret.native.anchor.inside ?? null,
              noneditableLeaf: caret.native.anchor.noneditableLeaf ?? null,
              position: caret.native.anchor.position ?? null,
            },
            head: {
              inside: caret.native.head.inside ?? null,
              noneditableLeaf: caret.native.head.noneditableLeaf ?? null,
              position: caret.native.head.position ?? null,
            },
            wide: caret.wide ?? null,
            rich: caret.rich ?? null,
          }
        : null,
    };
  }
  const actions = Object.fromEntries(
    ACTIONS.map((action) => [
      action,
      sample([...data.actionBoundaries, ...data.critical].find((value) => value.stage === action)),
    ]),
  );
  const causal = data.critical.filter((value) =>
    /^(keydown:Home:|keyup:Home:|transaction:|provider:)/.test(stage(value.stage)),
  );
  const mismatch = data.observedMismatches[0];
  const indexes = (pattern: RegExp, limit: number, near = false) =>
    causal
      .map((value, i) => ({ value, i }))
      .filter(
        ({ value }) =>
          pattern.test(stage(value.stage)) &&
          (!near || (mismatch !== undefined && Math.abs(value.at - mismatch.at) <= 25)),
      )
      .slice(0, limit)
      .map(({ i }) => i);
  const selected = [
    ...new Set([
      ...indexes(/^(keydown:Home:|keyup:Home:)/, 4),
      ...indexes(/^transaction:/, 3, true),
      ...indexes(/^provider:/, 2),
      ...causal.slice(0, 1).map((_, i) => i),
      ...causal.slice(-2).map((_, i) => Math.max(0, causal.length - 2) + i),
    ]),
  ].sort((a, b) => a - b);
  const result = {
    available: true as const,
    counts,
    updates: data.updates,
    localUpdates: data.localUpdates,
    bindingGeneration: integer(data.bindingGeneration),
    firstObservedState: sample(first),
    actions,
    firstRetiredEvent: sample(data.firstRetiredEvent),
    missingActions: ACTIONS.filter((action) => actions[action] === null),
    firstIdentityChange: sample(data.ownerChanges[0]),
    firstMismatch: sample(mismatch),
    causalSamples: selected.map((i) => sample(causal[i])),
    causalSampleTotal: causal.length,
    causalSamplesTruncated: causal.length > selected.length,
    tail: data.frames.slice(-6).map(sample),
    tailTruncated: data.frames.length > 6,
    earlyBindingNotCaptured: true,
    navigationContinuity: "current-document-only",
    captureErrorsRetained: data.critical.filter((value) => Object.hasOwn(value, "unknown")).length,
  };
  // ASCII JSON has the same conservative bound as the legacy public diagnostic.
  return asciiJson(result).length + separatorSpaces(result) <= 49152
    ? result
    : unavailable("output_size_limit");
}
function separatorSpaces(value: unknown): number {
  if (!value || typeof value !== "object") return 0;
  const values: unknown[] = Object.values(value);
  return (
    Math.max(0, values.length - 1) +
    (Array.isArray(value) ? 0 : values.length) +
    values.reduce<number>((total, item) => total + separatorSpaces(item), 0)
  );
}
export function asciiJson(value: unknown) {
  return JSON.stringify(value).replace(
    /[\u007f-\uffff]/g,
    (value) => `\\u${value.charCodeAt(0).toString(16).padStart(4, "0")}`,
  );
}
export function diagnosticFromBody(body: Uint8Array) {
  if (body.byteLength > 1024 * 1024) return unavailable("attachment_size_limit");
  try {
    const data: unknown = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(body));
    return diagnosticDigest(data);
  } catch {
    return unavailable("invalid_attachment_data");
  }
}

export function publicRoute(url: string) {
  try {
    const value = new URL(url);
    // No host, credentials, fragments, arbitrary route components or query values.
    const path = value.pathname;
    let route = "/<unknown>";
    if (["/assets/fixture.js", "/assets/index-DiwrgTda.js", "/assets/app.js", "/"].includes(path))
      route = path;
    else if (/^\/assets\/[a-zA-Z0-9_.-]+$/.test(path)) route = "/assets/<asset>";
    else if (/^\/(s|invite|share|invitations|ics)\/[^/]+$/.test(path))
      route = `/${path.split("/")[1] ?? "s"}/<redacted>`;
    else if (/^\/api\/v1\/invitations\/[^/]+\/accept$/.test(path))
      route = "/api/v1/invitations/<redacted>/accept";
    return route + (value.search ? "?…" : "");
  } catch {
    return "<unparsable url>";
  }
}
export function browserDigest(input: unknown) {
  const parsed = browserReportSchema.safeParse(input);
  if (!parsed.success) return unavailable("invalid_report");
  const rows = parsed.data.events
    .map((event) => {
      if (event.kind === "request")
        return {
          at: event.at,
          kind: event.kind,
          method: enumeration(event.method, [
            "GET",
            "POST",
            "PUT",
            "PATCH",
            "DELETE",
            "HEAD",
            "OPTIONS",
          ]),
          status:
            typeof event.status === "number" &&
            Number.isInteger(event.status) &&
            event.status >= 100 &&
            event.status <= 599
              ? event.status
              : "ERR",
          resourceType: enumeration(event.resourceType, [
            "document",
            "script",
            "fetch",
            "xhr",
            "stylesheet",
            "image",
            "font",
            "websocket",
            "other",
          ]),
          route: publicRoute(event.url),
          duration: event.duration,
          failure:
            event.failure === "net::ERR_ABORTED"
              ? "net::ERR_ABORTED"
              : event.failure
                ? "request failed"
                : null,
        };
      if (event.kind === "console")
        return {
          at: event.at,
          kind: event.kind,
          level: event.level,
          message:
            typeof event.text === "string" && event.text.startsWith("fetch failed")
              ? "fetch failed"
              : "<redacted>",
        };
      return { at: event.at, kind: event.kind };
    })
    .sort((a, b) => a.at - b.at);
  return {
    available: true as const,
    total: rows.length,
    dropped: Math.max(0, rows.length - 2000),
    rows: rows.slice(-2000),
  };
}
