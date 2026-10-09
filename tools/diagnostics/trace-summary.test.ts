import { describe, expect, test } from "bun:test";
import {
  browserDigest,
  diagnosticDigest,
  diagnosticFromBody,
  asciiJson,
  publicRoute,
} from "./trace-summary";
import {
  browserFixture,
  caretFixture,
  fixture,
  schemaCases,
  SENTINELS,
} from "./trace-summary-fixtures";

describe("trace summary schema", () => {
  for (const scenario of schemaCases)
    test(scenario.name, () => {
      const result = diagnosticDigest(scenario.input);
      if (scenario.reason) expect(result).toEqual({ available: false, reason: scenario.reason });
      else expect(result.available).toBe(true);
      const output = asciiJson(result);
      expect(output.length).toBeLessThanOrEqual(49152);
      for (const sentinel of SENTINELS)
        expect(JSON.stringify(result).includes(sentinel)).toBe(false);
    });
  test("diagnostic base/private/bounded and exact positive fields", () => {
    const result = diagnosticDigest(fixture());
    if (!result.available) throw new Error("Expected available fixture diagnostic");
    expect(result.counts.frames).toEqual({ retained: 1, total: 1, dropped: 0, unknown: false });
    const selected = result.actions["ShiftHome:original-return"];
    expect(selected?.native).toEqual({
      inside: true,
      codePoints: 8,
      equalsKnownFixtureCjkEmoji: true,
      positions: { anchor: 9, head: 1 },
      mappingUnknown: false,
    });
    expect(selected?.pm).toEqual({ anchor: 9, head: 1, empty: false, type: "text" });
    expect(selected?.auth).toEqual({
      authenticated: true,
      synced: true,
      scope: "read-write",
      status: "connected",
    });
    expect(selected?.nativeId.present).toBe(true);
    expect(selected?.nativeId.hash).toHaveLength(16);
    expect(result.firstIdentityChange?.previousOwner).toEqual([1, 2, 1, 3, 4, 5, 6]);
    expect(result.firstIdentityChange?.owner).toEqual([11, 12, 11, 13, 14, 15, 16]);
    expect(result.firstMismatch?.native.positions).toEqual({ anchor: 9, head: 1 });
    expect(result.missingActions).toEqual(["caret:after-click", "caret:after-End"]);
    expect(result.earlyBindingNotCaptured).toBe(true);
    for (const [action, value] of Object.entries(result.actions))
      if (!action.startsWith("caret:")) expect(value).not.toBeNull();
  });
  test("caret exact endpoint/owner/PM/focus projection", () => {
    const result = diagnosticDigest(caretFixture());
    if (!result.available) throw new Error("Expected available caret diagnostic");
    const click = result.actions["caret:after-click"],
      end = result.actions["caret:after-End"];
    expect(result.missingActions).toEqual([]);
    expect(click?.caret).toEqual({
      anchor: { inside: true, noneditableLeaf: false, position: 5 },
      head: { inside: true, noneditableLeaf: false, position: 5 },
      wide: true,
      rich: true,
    });
    expect(end?.caret).toEqual({
      anchor: { inside: false, noneditableLeaf: true, position: 9 },
      head: { inside: true, noneditableLeaf: false, position: 9 },
      wide: false,
      rich: false,
    });
    expect(click?.owner).toEqual([1, 2, 1, 3, 4, 5, 6]);
    expect(end?.owner).toEqual(click?.owner);
    expect(click?.pm).toEqual({ anchor: 5, head: 5, empty: true, type: "text" });
    expect(end?.native.positions).toEqual({ anchor: 9, head: 9 });
    expect(end?.focus.editor).toBe(true);
    expect(result.actions["ShiftHome:original-return"]?.caret).toBeNull();
  });
  test("caret nullable endpoints all null", () => {
    const result = diagnosticDigest(
      schemaCases.find((value) => value.name === "caret nullable")?.input,
    );
    if (!result.available) throw new Error("Expected available nullable diagnostic");
    expect(result.actions["caret:after-End"]?.caret).toEqual({
      anchor: { inside: null, noneditableLeaf: null, position: null },
      head: { inside: null, noneditableLeaf: null, position: null },
      wide: null,
      rich: null,
    });
  });
  test("hostile enums and truncated/legacy counts keep their meaning", () => {
    const hostile = diagnosticDigest(
      schemaCases.find((value) => value.name === "hostile enum/shape")?.input,
    );
    if (!hostile.available) throw new Error("Expected available hostile diagnostic");
    expect(hostile.tail[0]?.stage).toBe("unknown");
    expect(hostile.tail[0]?.auth.scope).toBe("unknown");
    expect(hostile.tail[0]?.native.equalsKnownFixtureCjkEmoji).toBe(false);
    expect(hostile.tail[0]?.native.mappingUnknown).toBe(true);
    expect(hostile.tail[0]?.at).toBe(10);
    const partial = diagnosticDigest(
      schemaCases.find((value) => value.name === "truncated counts")?.input,
    );
    const legacy = diagnosticDigest(
      schemaCases.find((value) => value.name === "legacy unknown counts")?.input,
    );
    if (!partial.available || !legacy.available)
      throw new Error("Expected available count diagnostics");
    expect(partial.counts.frames?.dropped).toBe(500);
    expect(legacy.counts.frames?.unknown).toBe(true);
  });
  test("diagnostic event bounds and strict scalar negative controls", () => {
    for (const [key, limit] of [
      ["frames", 512],
      ["critical", 2048],
      ["ownerChanges", 2048],
      ["observedMismatches", 4096],
      ["actionBoundaries", 16],
    ] as const) {
      expect(
        diagnosticDigest({
          ...fixture(),
          [key]: Array.from({ length: limit + 1 }, () => fixture().firstObservedState),
        }).available,
      ).toBe(false);
    }
    for (const at of [-1, 1e12 + 1, Infinity, NaN, true, "10"])
      expect(
        diagnosticDigest({ ...fixture(), frames: [{ ...fixture().firstObservedState, at }] }),
      ).toEqual({ available: false, reason: "invalid_event_schema" });
    for (const owner of ["[]", "[1,2,3,4,5,6,true]", "[1,2,3,4,5,6,-1]", "{", PRIVATE_OWNER])
      expect(
        diagnosticDigest({ ...fixture(), frames: [{ ...fixture().firstObservedState, owner }] })
          .available,
      ).toBe(false);
    expect(diagnosticDigest({ ...fixture(), localUpdates: 2 })).toEqual({
      available: false,
      reason: "invalid_update_counts",
    });
    expect(diagnosticDigest({ ...fixture(), updates: true })).toEqual({
      available: false,
      reason: "invalid_update_counts",
    });
    expect(
      diagnosticDigest({
        ...fixture(),
        frames: [{ ...fixture().firstObservedState, native: { text: "a".repeat(4097) } }],
      }),
    ).toEqual({ available: false, reason: "invalid_native_text_schema" });
  });
  test("malformed/non-JSON/UTF8/oversize attachments refuse without raw echo", () => {
    for (const value of ["{" + String(SENTINELS[0]), '{"at":NaN}', ""])
      expect(diagnosticFromBody(new TextEncoder().encode(value))).toEqual({
        available: false,
        reason: "invalid_attachment_data",
      });
    expect(diagnosticFromBody(new Uint8Array([255]))).toEqual({
      available: false,
      reason: "invalid_attachment_data",
    });
    expect(diagnosticFromBody(new Uint8Array(1024 * 1024 + 1))).toEqual({
      available: false,
      reason: "attachment_size_limit",
    });
  });
  test("output allowlist bounded causal selection and tail", () => {
    const data = fixture();
    data.frames = Array.from({ length: 7 }, (_, i) => ({ ...data.firstObservedState, at: i }));
    data.critical = Array.from({ length: 40 }, (_, i) => ({
      ...data.firstObservedState,
      at: i,
      stage: "transaction:doc=true:selection=false",
    }));
    data.totals.frames = 7;
    data.totals.critical = 40;
    const result = diagnosticDigest(data);
    if (!result.available) throw new Error("Expected available causal diagnostic");
    expect(result.tail).toHaveLength(6);
    expect(result.tailTruncated).toBe(true);
    expect(result.causalSampleTotal).toBe(40);
    expect(result.causalSamplesTruncated).toBe(true);
    expect(result.causalSamples.length).toBeLessThanOrEqual(12);
  });
});
const PRIVATE_OWNER = "a".repeat(257);

describe("direct browser summary", () => {
  test("safe trace basic diagnostics preserves fixed route/status/error categories", () => {
    const result = browserDigest(browserFixture);
    if (!result.available) throw new Error("Expected available browser fixture");
    const output = JSON.stringify(result);
    for (const value of [
      "PATHSECRET",
      "QUERYSECRET",
      "PARAMSECRET1",
      "PARAMSECRET2",
      "PARAMSECRET3",
      "DBSECRET",
      "S".repeat(20),
      "B".repeat(20),
      "127.0.0.1",
      "postgresql://",
    ])
      expect(output.includes(value)).toBe(false);
    expect(output).toContain("/s/<redacted>?…");
    expect(output).toContain("/api/v1/invitations/<redacted>/accept");
    expect(result.rows[1]).toMatchObject({
      kind: "request",
      method: "GET",
      status: "ERR",
      resourceType: "script",
      route: "/assets/index-DiwrgTda.js",
      failure: "net::ERR_ABORTED",
    });
    expect(result.rows[3]).toMatchObject({
      kind: "console",
      level: "error",
      message: "fetch failed",
    });
    expect(result.rows[4]).toMatchObject({ kind: "pageerror" });
  });
  test("summary privacy/clip/order uses bounded fixed projection instead of raw text", () => {
    expect(publicRoute(`https://u:secret@private.example/${"a".repeat(200000)}?token=secret`)).toBe(
      "/<unknown>?…",
    );
    const events = Array.from({ length: 2001 }, (_, i) => ({ kind: "crash", at: 2001 - i }));
    const result = browserDigest({ source: "fvoci-playwright", events });
    if (!result.available) throw new Error("Expected available tail fixture");
    expect(result.rows).toHaveLength(2000);
    expect(result.total).toBe(2001);
    expect(result.dropped).toBe(1);
    expect(result.rows[0]?.at).toBe(2);
    expect(result.rows.at(-1)?.at).toBe(2001);
  });
  test("blank-invalid-foreign-report and finite time/status/exit allowlists", () => {
    for (const input of [
      null,
      {},
      "",
      { source: "foreign", events: [] },
      { source: "fvoci-playwright", events: ["private"] },
      {
        source: "fvoci-playwright",
        events: Array.from({ length: 10001 }, () => ({ kind: "crash", at: 1 })),
      },
    ])
      expect(browserDigest(input).available).toBe(false);
    const input = {
      source: "fvoci-playwright",
      events: [
        {
          kind: "request",
          at: 1,
          duration: 2,
          method: SENTINELS[0],
          status: 999,
          resourceType: SENTINELS[0],
          url: "https://private.example/?password=sentinel",
          failure: SENTINELS[0],
        },
      ],
    };
    const result = browserDigest(input);
    if (!result.available) throw new Error("Expected available projected unknown fields");
    expect(result.rows[0]).toMatchObject({
      method: "unknown",
      status: "ERR",
      resourceType: "unknown",
      route: "/?…",
      failure: "request failed",
    });
  });
});
