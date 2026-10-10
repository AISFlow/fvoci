import { describe, expect, test } from "bun:test";
import { summarizeMembers } from "./trace-summary.ts";
import { writeZip, type ZipMember, readZip } from "./zip.ts";

const secret = "DIAGNOSTIC_PRIVATE_VALUE";
const member = `attachments/${"a".repeat(40)}`;

function frame(stage = "ShiftHome:original-return") {
  return {
    at: 10,
    stage,
    owner: "[1,2,1,3,4,5,6]",
    bindingGeneration: 1,
    nativeId: "NATIVE_ID_PRIVATE",
    native: { inside: true, text: "한글과 😀 링크", positions: { anchor: 9, head: 1 } },
    pm: { anchor: 9, head: 1, empty: false, type: "text", marks: [{ href: secret }] },
    focus: {
      activeTag: "DIV",
      activeLabel: secret,
      editor: true,
      editorEditable: true,
      domEditable: "true",
      composing: false,
    },
    auth: {
      authenticated: true,
      synced: true,
      scope: "read-write",
      status: "connected",
      password: secret,
    },
    updates: 0,
    localUpdates: 0,
    generationUpdates: 0,
    generationLocalUpdates: 0,
    bubble: { visibility: "visible", opacity: "1" },
    dialog: false,
    pmDocument: { type: "doc", attrs: { rawFuture: secret }, text: secret },
    unknown: `https://private.example/${secret}?password=${secret}`,
  };
}

const actions = [
  "openDoc:original",
  "ShiftHome:original-return",
  "bubble:original-visible",
  "popup:original-open-focus",
  "compositionEnter:original-no-link",
  "Cancel:original-native-text",
  "Apply:original-selected-text",
  "save:original",
  "finally",
];

function payload() {
  const base = frame();
  const boundary = actions.map((stage) => ({ ...structuredClone(base), stage }));
  return {
    frames: [base],
    critical: boundary,
    actionBoundaries: boundary.slice(0, -1),
    ownerChanges: [
      {
        at: 20,
        stage: "provider:status",
        previousOwner: base.owner,
        owner: "[11,12,11,13,14,15,16]",
        bindingGeneration: 2,
      },
    ],
    observedMismatches: [base],
    firstObservedState: base,
    updates: 1,
    localUpdates: 1,
    totals: { frames: 1, critical: 9, actionBoundaries: 8, ownerChanges: 1, observedMismatches: 1 },
    dropped: {
      frames: 0,
      critical: 0,
      actionBoundaries: 0,
      ownerChanges: 0,
      observedMismatches: 0,
    },
  };
}

function archive(
  options: {
    payload?: unknown;
    ref?: Record<string, string>;
    test?: string;
    extra?: [string, string][];
    duplicate?: boolean;
  } = {},
) {
  const ref = options.ref ?? {
    name: "w3-template-native-selection-observation.json",
    contentType: "application/json",
    file: member,
  };
  const entries = [
    {
      name: "0-trace.network",
      data: JSON.stringify({
        type: "resource-snapshot",
        snapshot: {
          request: { method: "GET", url: "http://localhost/assets/fixture.js" },
          response: { status: 200 },
          _resourceType: "script",
          time: 1,
          _monotonicTime: 1,
        },
      }),
    },
    {
      name: "test.trace",
      data: options.test ?? JSON.stringify({ type: "after", attachments: [ref] }),
    },
    {
      name: member,
      data:
        typeof options.payload === "string"
          ? options.payload
          : JSON.stringify(options.payload ?? payload()),
    },
  ];
  for (const [name, data] of options.extra ?? []) entries.push({ name, data });
  if (options.duplicate) entries.push({ name: member, data: JSON.stringify(payload()) });
  return readZip(writeZip(entries));
}

function diagnostic(members: ZipMember[]) {
  const output = summarizeMembers(members);
  expect(output).toContain("GET 200 script /assets/fixture.js");
  for (const forbidden of [
    secret,
    "NATIVE_ID_PRIVATE",
    "private.example",
    "한글과",
    "😀",
    "rawFuture",
    "activeLabel",
    "pmDocument",
    "password",
  ]) {
    expect(output).not.toContain(forbidden);
  }
  const line = output.split("\n").find((item) => item.startsWith("w3-template-diagnostic "));
  if (!line) throw new Error("missing diagnostic");
  const record = JSON.parse(line.slice("w3-template-diagnostic ".length)) as {
    available: boolean;
    reason?: string;
  };
  expect(Buffer.byteLength(JSON.stringify(record))).toBeLessThanOrEqual(49152);
  return record;
}

describe("trace summary", () => {
  test("redacts paths, parameters, console text and page errors", () => {
    const share = "S".repeat(43);
    const bare = "B".repeat(45);
    const snap = (url: string, status: number, failure = "", kind = "document", t = 1) => ({
      type: "resource-snapshot",
      snapshot: {
        request: { method: "GET", url },
        response: { status, _failureText: failure },
        _resourceType: kind,
        startedDateTime: "2026-09-29T00:00:00.000Z",
        time: 12,
        _monotonicTime: t,
      },
    });
    const members = readZip(
      writeZip([
        {
          name: "0-trace.network",
          data: [
            snap(`http://127.0.0.1:4000/s/${share}?code=QUERYSECRET`, 404),
            snap(
              "http://127.0.0.1:4000/assets/index-DiwrgTda.js",
              0,
              "net::ERR_ABORTED",
              "script",
              1.5,
            ),
            snap(
              "http://127.0.0.1:4000/api/v1/invitations/PATHSECRET/accept",
              404,
              "",
              "fetch",
              1.7,
            ),
          ]
            .map((entry) => JSON.stringify(entry))
            .join("\n"),
        },
        {
          name: "0-trace.trace",
          data: [
            JSON.stringify({
              type: "console",
              messageType: "error",
              time: 2,
              text: `fetch failed access_token=PARAMSECRET1 inviteToken=PARAMSECRET2 ${bare} postgresql://u:DBSECRET@h/db`,
              location: { url: "http://127.0.0.1:4000/assets/app.js" },
            }),
            JSON.stringify({
              type: "event",
              method: "pageError",
              time: 3,
              params: {
                error: {
                  error: { name: "Error", message: `${"a".repeat(200000)} state=PARAMSECRET3` },
                },
              },
            }),
          ].join("\n"),
        },
      ]),
    );
    const output = summarizeMembers(members);
    expect(output.startsWith("browser summary: ")).toBe(true);
    for (const leaked of [
      "PATHSECRET",
      "QUERYSECRET",
      "PARAMSECRET1",
      "PARAMSECRET2",
      "PARAMSECRET3",
      "DBSECRET",
      "S".repeat(20),
      "B".repeat(20),
    ]) {
      expect(output).not.toContain(leaked);
    }
    expect(output).toContain("/s/<redacted>?…");
    expect(output).toContain("/api/v1/invitations/<redacted>/accept");
    expect(output).toContain("GET ERR script /assets/index-DiwrgTda.js");
    expect(output).toContain("net::ERR_ABORTED");
    expect(output).toContain("console   error fetch failed");
    expect(output).toContain("pageerror");
  });

  test("diagnostic allowlist keeps fixed fields and drops private ones", () => {
    const valid = diagnostic(archive()) as {
      available: true;
      counts: { frames: unknown };
      actions: Record<
        string,
        {
          native: unknown;
          pm: unknown;
          auth: unknown;
          nativeId: { present: boolean; hash: string };
          caret: unknown;
        }
      >;
      firstIdentityChange: { previousOwner: number[]; owner: number[] };
      firstMismatch: { native: { positions: unknown } };
      missingActions: string[];
      earlyBindingNotCaptured: boolean;
    };
    expect(valid.available).toBe(true);
    expect(valid.counts.frames).toEqual({ retained: 1, total: 1, dropped: 0, unknown: false });
    expect(valid.actions["ShiftHome:original-return"]?.native).toEqual({
      inside: true,
      codePoints: 8,
      equalsKnownFixtureCjkEmoji: true,
      positions: { anchor: 9, head: 1 },
      mappingUnknown: false,
    });
    expect(valid.actions["ShiftHome:original-return"]?.pm).toEqual({
      anchor: 9,
      head: 1,
      empty: false,
      type: "text",
    });
    expect(valid.actions["ShiftHome:original-return"]?.auth).toEqual({
      authenticated: true,
      synced: true,
      scope: "read-write",
      status: "connected",
    });
    expect(valid.actions["ShiftHome:original-return"]?.nativeId.present).toBe(true);
    expect(valid.actions["ShiftHome:original-return"]?.nativeId.hash).toHaveLength(16);
    expect(valid.firstIdentityChange.previousOwner).toEqual([1, 2, 1, 3, 4, 5, 6]);
    expect(valid.firstIdentityChange.owner).toEqual([11, 12, 11, 13, 14, 15, 16]);
    expect(valid.firstMismatch.native.positions).toEqual({ anchor: 9, head: 1 });
    expect(valid.missingActions).toEqual(["caret:after-click", "caret:after-End"]);
    expect(valid.earlyBindingNotCaptured).toBe(true);
    expect(valid.actions["ShiftHome:original-return"]?.caret).toBeNull();
  });

  test("hostile stages and unknown enums stay on the allowlist", () => {
    const data = payload();
    const first = data.frames[0] as {
      stage: string;
      auth: { scope: string };
      focus: { activeTag: string };
      native: { positions: unknown; text: string };
    };
    first.stage = `keydown:${secret}`;
    first.auth.scope = secret;
    first.focus.activeTag = secret;
    first.native.positions = { unknown: secret };
    first.native.text = secret;
    const safe = diagnostic(archive({ payload: data })) as {
      tail: {
        stage: string;
        auth: { scope: string };
        native: { equalsKnownFixtureCjkEmoji: boolean; mappingUnknown: boolean };
        at: number;
      }[];
    };
    expect(safe.tail[0]?.stage).toBe("unknown");
    expect(safe.tail[0]?.auth.scope).toBe("unknown");
    expect(safe.tail[0]?.native.equalsKnownFixtureCjkEmoji).toBe(false);
    expect(safe.tail[0]?.native.mappingUnknown).toBe(true);
    expect(safe.tail[0]?.at).toBe(10);
  });

  test("invalid attachment inputs fail closed with fixed reasons", () => {
    const cases: [string, Parameters<typeof archive>[0]][] = [
      [
        "invalid_attachment_reference",
        {
          ref: {
            name: "w3-template-native-selection-observation.json",
            contentType: "application/json",
            file: `../${secret}`,
          },
        },
      ],
      [
        "invalid_attachment_reference",
        {
          ref: {
            name: "w3-template-native-selection-observation.json",
            contentType: "text/plain",
            file: member,
          },
        },
      ],
      [
        "missing_or_invalid_attachment_member",
        {
          ref: {
            name: "w3-template-native-selection-observation.json",
            contentType: "application/json",
            file: `attachments/${"b".repeat(40)}`,
          },
        },
      ],
      ["invalid_attachment_data", { payload: `{${secret}` }],
      ["attachment_size_limit", { payload: " ".repeat(1024 * 1024 + 1) }],
      ["invalid_event_schema_or_limit", { payload: { frames: secret } }],
      ["missing_or_invalid_attachment_member", { duplicate: true }],
      ["test_event_limit", { test: Array.from({ length: 10001 }, () => "{}").join("\n") }],
      ["invalid_attachment_data", { test: `{${secret}` }],
      [
        "invalid_attachment_data",
        { payload: JSON.stringify(payload()).replace('"at":10', '"at":NaN') },
      ],
      ["missing_or_duplicate_attachment", { test: "{}" }],
      [
        "member_limit",
        { extra: Array.from({ length: 4096 }, (_, index) => [`unrelated/${index}`, ""]) },
      ],
    ];
    for (const [reason, options] of cases) {
      expect(diagnostic(archive(options))).toEqual({ available: false, reason });
    }
    const inconsistent = payload();
    inconsistent.totals.frames = 2;
    expect(diagnostic(archive({ payload: inconsistent }))).toEqual({
      available: false,
      reason: "inconsistent_collection_counts",
    });
    const nested = payload();
    (nested.frames[0] as { native: unknown }).native = secret;
    expect(diagnostic(archive({ payload: nested }))).toEqual({
      available: false,
      reason: "invalid_snapshot_schema",
    });
  });
});
