import { describe, expect, test } from "bun:test";
import {
  AUTH_CASES,
  assertAuthNoEffect,
  authSessionCases,
  authSessionWitnesses,
  authSessionEventEffects,
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

function observedReport() {
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
      targetId: id,
      before: observation(),
      after: observation(),
      refusal: { boundary: "pre-auth-http", method: "POST", status: 401, directStatus: 401 },
      wrongCurrentStatus: 400,
      oldPasswordStatus: 401,
      newPasswordStatus: 200,
      tamperedStatus: 400,
      spentStatus: 400,
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
