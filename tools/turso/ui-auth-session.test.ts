import { describe, expect, test } from "bun:test";
import type { Page, WebSocketRoute } from "@playwright/test";
import { activeRoom } from "../../apps/web/e2e-pending/auth-session-helpers.ts";
import {
  AUTH_CASES,
  assertAuthNoEffect,
  authSessionCases,
  authSessionWitnesses,
  authSessionEventEffects,
  authEventDelta,
  authEventRows,
  authSessionActor,
} from "./ui-auth-session.ts";

const source = "a".repeat(40);
const tree = "b".repeat(40);
const id = "019b0000-0000-7000-8000-000000000001";
const observation = () => ({
  targetId: id,
  body: {
    contentJson: { type: "doc", content: [{ type: "paragraph", attrs: { id } }] },
    version: 1,
  },
  ids: [id],
  nativeV1: "AQEBAA==",
  revisions: [{ id, targetId: id, ySnapshot: "AAA=" }],
  events: [{ id, targetId: id, verb: "document.updated" }],
});

describe("auth pending-command no-effect oracle", () => {
  test("accepts complete unchanged native/body/identity/revision/event observations", () => {
    assertAuthNoEffect(observation(), structuredClone(observation()));
  });
  for (const field of ["targetId", "body", "ids", "nativeV1", "revisions", "events"] as const) {
    test(`rejects changed ${field}`, () => {
      const after: Record<string, unknown> = structuredClone(observation());
      after[field] = field === "targetId" || field === "nativeV1" ? "changed" : [];
      expect(() => {
        assertAuthNoEffect(observation(), after);
      }).toThrow();
    });
    test(`rejects missing ${field} rather than counting zero`, () => {
      const incomplete: Record<string, unknown> = observation();
      Reflect.deleteProperty(incomplete, field);
      expect(() => {
        assertAuthNoEffect(incomplete, incomplete);
      }).toThrow();
    });
  }
});

function report() {
  return {
    config: {
      metadata: { authSession: true, selectedBackend: "sqlite", selectedFlow: "on", source, tree },
      workers: 1,
    },
    errors: [],
    suites: [
      {
        specs: AUTH_CASES.map((title) => ({
          file: "auth-session-selected-backend.acceptance.ts",
          title,
          ok: true,
          tests: [
            { expectedStatus: "passed", results: [{ status: "passed", retry: 0, errors: [] }] },
          ],
        })),
      },
    ],
  };
}

const binding = { backend: "sqlite", flow: "on", source, tree } as const;

test("passed browser titles without actual effect attachments cannot supply acceptance evidence", () => {
  expect(() => authSessionWitnesses(report(), binding)).toThrow();
});

function observedReport(extra: Record<string, unknown> = {}) {
  const actual = report();
  for (const spec of actual.suites[0]?.specs ?? []) {
    const result = spec.tests[0]?.results[0];
    if (!result) throw new Error("Missing observed report fixture");
    const witness = {
      schema: 1,
      scope: "document-effects",
      backend: binding.backend,
      flow: binding.flow,
      source,
      tree,
      actor: id,
      owner: "019b0000-0000-7000-8000-000000000002",
      workspaceId: "019b0000-0000-7000-8000-000000000003",
      targetId: id,
      before: observation(),
      after: observation(),
      refusal: { boundary: "pre-auth-http", method: "POST", status: 401, directStatus: 401 },
      wrongCurrentStatus: 400,
      oldPasswordStatus: 401,
      newPasswordStatus: 200,
      tamperedStatus: 400,
      spentStatus: 400,
      ...extra,
    };
    Reflect.set(result, "attachments", [
      {
        name: "auth-session-document-effects.json",
        contentType: "application/json",
        body: Buffer.from(JSON.stringify(witness)).toString("base64"),
      },
    ]);
  }
  return actual;
}

test("consumes the three complete source-bound document effect attachments", () => {
  expect(authSessionWitnesses(observedReport(), binding)).toHaveLength(3);
});

test("actual source and ON/OFF request binding cannot be replaced by report titles", () => {
  expect(() => authSessionWitnesses(observedReport(), { ...binding, flow: "off" })).toThrow();
});

test("every witness names one canonical workspace", () => {
  for (const workspaceId of [undefined, "", "019B0000-0000-7000-8000-000000000003", "workspace"])
    expect(() => authSessionWitnesses(observedReport({ workspaceId }), binding)).toThrow();
});

function nativeEvents() {
  const actor = id.replaceAll("-", "");
  const allocations: Record<string, unknown> = {};
  const fingerprints: Record<string, number> = {};
  for (const [index, kind] of [
    "actor-logout",
    "actor-password-change",
    "actor-password-reset",
  ].entries()) {
    const hash = String(index + 1).repeat(64);
    allocations[hash] = {
      actors: [actor],
      workspaces: [],
      eventWitness: {
        class: kind,
        sequence: ["integer", String(index + 1)],
        ...(kind === "actor-logout" ? { actor } : { target: actor }),
      },
    };
    fingerprints[hash] = 1;
  }
  return {
    backend: "libsql-remote",
    fingerprints: { events: fingerprints },
    allocations: { events: allocations },
  };
}

const beforeEvents = { backend: "libsql-remote", fingerprints: { events: {} } };

test("normal auth mutations occur exactly once in the native event audit", () => {
  authSessionEventEffects(beforeEvents, nativeEvents(), id);
});

test("missing, repeated and foreign auth events refuse instead of becoming zero effects", () => {
  const missing = nativeEvents();
  Reflect.deleteProperty(missing.allocations.events, "1".repeat(64));
  expect(() => {
    authSessionEventEffects(beforeEvents, missing, id);
  }).toThrow();
  const repeated = nativeEvents();
  repeated.allocations.events["4".repeat(64)] = repeated.allocations.events["1".repeat(64)];
  repeated.fingerprints.events["4".repeat(64)] = 1;
  expect(() => {
    authSessionEventEffects(beforeEvents, repeated, id);
  }).toThrow();
  expect(() => {
    authSessionEventEffects(beforeEvents, nativeEvents(), "019b0000-0000-7000-8000-000000000002");
  }).toThrow();
  expect(() => {
    authSessionEventEffects({}, {}, id);
  }).toThrow();
});

test("accepts exactly the three dedicated browser cases at the bound source/tree/backend/flow", () => {
  expect(authSessionCases(report(), binding)).toHaveLength(3);
});

for (const key of ["backend", "flow", "source", "tree"] as const) {
  test(`refuses a different ${key} binding`, () => {
    expect(() => authSessionCases(report(), { ...binding, [key]: "wrong" })).toThrow();
  });
}

test("refuses missing, duplicate, skipped, failed and retried browser cases", () => {
  function first(r: ReturnType<typeof report>) {
    const suite = r.suites[0];
    const spec = suite?.specs[0];
    const result = spec?.tests[0]?.results[0];
    if (!suite || !spec || !result) throw new Error("Missing direct control fixture");
    return { suite, spec, result };
  }
  for (const mutate of [
    (r: ReturnType<typeof report>) => {
      r.suites[0]?.specs.pop();
    },
    (r: ReturnType<typeof report>) => {
      const { suite, spec } = first(r);
      suite.specs.push(spec);
    },
    (r: ReturnType<typeof report>) => {
      first(r).result.status = "skipped";
    },
    (r: ReturnType<typeof report>) => {
      first(r).spec.ok = false;
    },
    (r: ReturnType<typeof report>) => {
      first(r).result.retry = 1;
    },
  ]) {
    const invalid = report();
    mutate(invalid);
    expect(() => authSessionCases(invalid, binding)).toThrow();
  }
});

describe("PostgreSQL/SQLite auth event oracle", () => {
  const member = id;
  const other = "019b0000-0000-7000-8000-000000000002";
  const pgRow = (seq: number, verb: string, overrides: Record<string, unknown> = {}) => ({
    seq: String(seq),
    verb,
    channel: "web",
    workspace: null,
    actor: member,
    targetType: verb === "auth.logout" ? null : "user",
    target: verb === "auth.logout" ? null : member,
    payload: verb === "auth.logout" ? {} : { userId: member },
    ...overrides,
  });
  const normal = () => [
    pgRow(3, "auth.login", { payload: { userId: member, method: "password" } }),
    pgRow(4, "auth.logout"),
    pgRow(5, "auth.password_changed"),
    pgRow(6, "auth.password_reset"),
  ];
  const prior = () => [pgRow(1, "auth.login", { actor: other, target: other })];
  const delta = (before: unknown[], after: unknown[], actor = member) => {
    authEventDelta(authEventRows(before, "postgres"), authEventRows(after, "postgres"), actor);
  };

  test("the member's three normal auth mutations occur exactly once around retained history", () => {
    delta(prior(), [...prior(), ...normal()]);
  });

  test("SQLite raw blob/text columns normalize to the same rows as PostgreSQL", () => {
    const blob = (value: string | null) =>
      value === null ? null : new Uint8Array(Buffer.from(value.replaceAll("-", ""), "hex"));
    const raw = normal().map((row) => [
      Number(row.seq),
      row.verb,
      row.channel,
      blob(row.workspace as string | null),
      blob(row.actor),
      row.targetType,
      blob(row.target),
      JSON.stringify(row.payload),
    ]);
    expect(authEventRows(raw, "sqlite")).toEqual(authEventRows(normal(), "postgres"));
    authEventDelta([], authEventRows(raw, "sqlite"), member);
  });

  test("missing, repeated, extra, foreign or reshaped auth events refuse", () => {
    const refusals: [unknown[], unknown[], string?][] = [
      [[], normal().filter((row) => row.verb !== "auth.logout")],
      [[], [...normal(), pgRow(7, "auth.password_reset")]],
      [[], [...normal(), pgRow(7, "auth.mfa_disabled")]],
      [[], [...normal(), pgRow(7, "auth.unknown")]],
      [[], normal(), other],
      [[], normal().map((row) => (row.seq === "4" ? { ...row, target: member } : row))],
      [[], normal().map((row) => (row.seq === "5" ? { ...row, channel: "api" } : row))],
      [[], normal().map((row) => (row.seq === "6" ? { ...row, payload: {} } : row))],
      [
        [],
        normal().map((row) =>
          row.seq === "5" ? { ...row, workspace: "019b0000-0000-7000-8000-000000000003" } : row,
        ),
      ],
      // Earlier history cannot disappear or change behind the new rows.
      [prior(), normal()],
      [prior(), [{ ...pgRow(1, "auth.login", { actor: other }) }, ...normal()]],
    ];
    for (const [before, after, actor] of refusals)
      expect(() => {
        delta(before, after, actor);
      }).toThrow();
  });

  test("a bounded observation at the limit and malformed rows refuse instead of truncating", () => {
    const full = Array.from({ length: 10001 }, (_, index) => pgRow(index + 1, "auth.login"));
    expect(() => authEventRows(full, "postgres")).toThrow();
    expect(() => authEventRows([{ ...pgRow(1, "auth.login"), extra: 1 }], "postgres")).toThrow();
    expect(() => authEventRows([pgRow(1, "document.updated")], "postgres")).toThrow();
    expect(() => authEventRows([pgRow(0, "auth.login")], "postgres")).toThrow();
    expect(() =>
      authEventRows([[1, "auth.login", "web", null, null, null, null, "{"]], "sqlite"),
    ).toThrow();
    expect(() => authEventRows({}, "postgres")).toThrow();
  });

  test("every case must witness the same auth member", () => {
    const witnesses = authSessionWitnesses(observedReport(), binding);
    expect(authSessionActor(witnesses)).toBe(id);
    const mixed = witnesses.map((witness, index) =>
      index === 2 ? { ...witness, actor: other } : witness,
    );
    expect(() => authSessionActor(mixed)).toThrow();
    expect(() => authSessionActor(witnesses.slice(1))).toThrow();
  });
});

async function routedRoom() {
  let closed: (code?: number, reason?: string) => Promise<void> = () =>
    Promise.reject(new Error("Server close observer was not registered"));
  const forwarded: { code?: number; reason?: string }[] = [];
  const socket = {
    connectToServer: () => ({
      onClose(handler: typeof closed) {
        closed = handler;
      },
    }),
    close(options: { code?: number; reason?: string }) {
      forwarded.push(options);
      return Promise.resolve();
    },
  } as unknown as WebSocketRoute;
  const page = {
    async routeWebSocket(pattern: string, handler: (route: WebSocketRoute) => unknown) {
      expect(pattern).toBe("**/collab");
      await handler(socket);
    },
  } as unknown as Page;
  const actual = await activeRoom(page);
  return { actual, forwarded, close: (code?: number, reason?: string) => closed(code, reason) };
}

test("a code-less navigation closure is observed and forwarded without inventing revocation", async () => {
  const room = await routedRoom();
  await room.close(undefined, undefined);
  expect(room.actual.connections).toBe(1);
  expect(room.actual.closedCodes).toEqual([undefined]);
  expect(room.actual.closedCodes).not.toContain(1008);
  const projection = JSON.parse(JSON.stringify(room.actual)) as { closedCodes: (number | null)[] };
  expect(projection.closedCodes).toEqual([null]);
  expect(room.forwarded).toEqual([{ code: undefined, reason: undefined }]);
});

test("only an observed server 1008 supplies revocation evidence and forwards its actual reason", async () => {
  const room = await routedRoom();
  await room.close(1000, "normal departure");
  expect(room.actual.closedCodes).not.toContain(1008);
  await room.close(1008, "session revoked");
  expect(room.actual.closedCodes).toEqual([1000, 1008]);
  expect(room.forwarded).toEqual([
    { code: 1000, reason: "normal departure" },
    { code: 1008, reason: "session revoked" },
  ]);
});
