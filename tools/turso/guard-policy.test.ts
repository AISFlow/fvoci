import { describe, expect, test } from "bun:test";
import {
  AdmissionError,
  decide,
  eventInputs,
  parseMode,
  PHASES,
  requireImplemented,
  REVIEWED_REF,
  UI_REVIEWED_REF,
  validateDispatch,
  validateEnvironment,
  validateTarget,
  validateUiInputs,
  type DispatchContext,
  type Inputs,
} from "./guard-policy.ts";
import { parseEnvironmentBody } from "./guard-io.ts";

const A = "a".repeat(40);
const context: DispatchContext = {
  event_name: "workflow_dispatch",
  repository: "AISFlow/fvoci",
  ref: "refs/heads/main",
  sha: A,
};
const host = "isolated-owner.aws-us-east-1.turso.io";
const inputs: Inputs = { phase: "connection", destructive: false };
const settings = { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false" };
// Literal invented fixture values, never a credential lookup.
const secrets = {
  FVOCI_TEST_TURSO_DATABASE_URL: "libsql://" + host,
  FVOCI_TEST_TURSO_AUTH_TOKEN: "FAKE_FIXTURE_TOKEN",
};

export function denied(code: string, run: () => unknown): void {
  let caught: unknown;
  try {
    run();
  } catch (error) {
    caught = error;
  }
  expect(caught).toBeInstanceOf(AdmissionError);
  expect((caught as Error).message).toBe(code);
}

describe("dispatch admission", () => {
  test("trusted dispatch configuration", () => {
    expect(validateDispatch(context, inputs, A)).toBe("connection");
  });

  test("fixed reviewed branch bootstrap and manual-only consumption", () => {
    expect(validateDispatch({ ...context, event_name: "push", ref: REVIEWED_REF }, {}, A)).toBe(
      "connection",
    );
    expect(validateDispatch({ ...context, ref: REVIEWED_REF }, inputs, A)).toBe("connection");
    for (const [event_name, ref] of [
      ["push", "refs/heads/main"],
      ["push", "refs/heads/topic"],
      ["workflow_dispatch", "refs/heads/topic"],
    ]) {
      denied("UNTRUSTED_DISPATCH", () =>
        validateDispatch({ ...context, event_name, ref }, inputs, A),
      );
    }
  });

  test("product integration ref is manual UI only", () => {
    const ui = { ...context, ref: UI_REVIEWED_REF };
    expect(validateDispatch(ui, { phase: "ui-baseline", destructive: false }, A)).toBe(
      "ui-baseline",
    );
    expect(validateDispatch(ui, { phase: "ui-ack", destructive: true }, A)).toBe("ui-ack");
    denied("UNTRUSTED_DISPATCH", () => validateDispatch({ ...ui, event_name: "push" }, inputs, A));
    denied("UNTRUSTED_DISPATCH", () =>
      validateDispatch({ ...ui, event_name: "pull_request" }, inputs, A),
    );
    denied("UNTRUSTED_DISPATCH", () =>
      validateDispatch(
        { ...ui, repository: "attacker/fvoci" },
        { phase: "ui-baseline", destructive: false },
        A,
      ),
    );
    denied("CHECKOUT_MISMATCH", () =>
      validateDispatch(ui, { phase: "ui-baseline", destructive: false }, "b".repeat(40)),
    );
    for (const phase of PHASES) {
      if (phase === "ui-baseline" || phase === "ui-ack") continue;
      const destructive = phase !== "connection" && phase !== "inventory";
      denied("UI_REF_PHASE_REQUIRED", () => validateDispatch(ui, { phase, destructive }, A));
    }
  });

  test("denied event, repository and ref", () => {
    for (const [key, value] of [
      ["event_name", "pull_request"],
      ["event_name", "pull_request_target"],
      ["repository", "attacker/fvoci"],
      ["ref", "refs/heads/topic"],
      ["ref", "refs/tags/main"],
    ] as const) {
      denied("UNTRUSTED_DISPATCH", () => validateDispatch({ ...context, [key]: value }, inputs, A));
    }
  });

  test("empty, stale and arbitrary checkout", () => {
    for (const [sha, checkout] of [
      ["", ""],
      [A, "b".repeat(40)],
      ["main", "main"],
    ] as const) {
      denied("CHECKOUT_MISMATCH", () => validateDispatch({ ...context, sha }, inputs, checkout));
    }
    // An unset GITHUB_SHA is an internal failure, not a fixed refusal code.
    expect(() => validateDispatch({ ...context, sha: undefined }, inputs, A)).toThrow(TypeError);
  });

  test("boolean does not accept truthy strings", () => {
    for (const value of ["false", "true", 0, 1, null]) {
      denied("INVALID_BOOLEAN", () =>
        validateDispatch(context, { ...inputs, destructive: value }, A),
      );
    }
  });

  test("future phases require explicit dispatch confirmation", () => {
    for (const phase of PHASES.filter(
      (p) => !["connection", "inventory", "ui-baseline"].includes(p),
    )) {
      denied("DESTRUCTIVE_CONFIRMATION_REQUIRED", () =>
        validateDispatch(context, { ...inputs, phase }, A),
      );
    }
  });

  test("migration requires manual dispatch and both destructive flags", () => {
    const migration = { phase: "migration", destructive: true };
    expect(validateDispatch(context, migration, A)).toBe("migration");
    denied("SECRET_MODE_REQUIRES_MANUAL", () =>
      validateDispatch({ ...context, event_name: "push", ref: REVIEWED_REF }, migration, A),
    );
    denied("DESTRUCTIVE_CONFIRMATION_REQUIRED", () =>
      validateDispatch(context, { ...migration, destructive: false }, A),
    );
    for (const gate of [{}, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "false" }]) {
      denied("DESTRUCTIVE_NOT_ALLOWED", () => validateTarget(migration, gate, secrets));
    }
    expect(validateTarget(migration, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" }, secrets)).toBe(
      "migration",
    );
  });

  test("inventory manual dispatch routes without untrusted or false escape", () => {
    const inventory = { phase: "inventory", destructive: false };
    expect(() => {
      requireImplemented("inventory");
    }).not.toThrow();
    for (const ref of ["refs/heads/main", REVIEWED_REF]) {
      expect(validateDispatch({ ...context, ref }, inventory, A)).toBe("inventory");
    }
    for (const [changed, given, code] of [
      [{ ...context, repository: "attacker/fvoci" }, inventory, "UNTRUSTED_DISPATCH"],
      [{ ...context, event_name: "pull_request" }, inventory, "UNTRUSTED_DISPATCH"],
      [{ ...context, event_name: "pull_request_target" }, inventory, "UNTRUSTED_DISPATCH"],
      [{ ...context, ref: "refs/heads/topic" }, inventory, "UNTRUSTED_DISPATCH"],
      [
        { ...context, event_name: "push", ref: REVIEWED_REF },
        inventory,
        "SECRET_MODE_REQUIRES_MANUAL",
      ],
      [{ ...context, sha: "b".repeat(40) }, inventory, "CHECKOUT_MISMATCH"],
      [context, { ...inventory, destructive: true }, "INVENTORY_MUST_BE_READ_ONLY"],
    ] as const) {
      denied(code, () => validateDispatch(changed, given, A));
    }
  });

  test("reset requires manual confirmation", () => {
    const reset = { phase: "reset", destructive: true };
    expect(validateDispatch(context, reset, A)).toBe("reset");
    for (const changed of [
      { ...context, event_name: "push", ref: REVIEWED_REF },
      { ...context, repository: "attacker/fvoci" },
      { ...context, ref: "refs/heads/topic" },
    ]) {
      expect(() => validateDispatch(changed, reset, A)).toThrow(AdmissionError);
    }
  });

  test("unknown phase is not a shell command", () => {
    denied("UNKNOWN_PHASE", () => validateDispatch(context, { ...inputs, phase: "echo FAKE" }, A));
    denied("UNKNOWN_PHASE", () => {
      requireImplemented("echo FAKE");
    });
  });

  test("every fixed unimplemented phase refuses a successful no-op", () => {
    for (const phase of ["crud", "transactions", "persistence", "restore"]) {
      denied("NOT_IMPLEMENTED", () => {
        requireImplemented(phase);
      });
    }
    // Permission to select a real fixture is not its execution or PASS.
    expect(() => {
      requireImplemented("connection");
    }).not.toThrow();
    expect(() => {
      requireImplemented("migration");
    }).not.toThrow();
  });
});

describe("target configuration", () => {
  test("connection has no host or database name requirement", () => {
    expect(validateDispatch(context, inputs, A)).toBe("connection");
    expect(validateTarget(inputs, {}, secrets)).toBe("connection");
  });

  test("connection refuses destructive", () => {
    denied("CONNECTION_MUST_BE_READ_ONLY", () =>
      validateTarget({ ...inputs, destructive: true }, settings, secrets),
    );
  });

  test("inventory read-only admission is exact and other phases stay destructive", () => {
    const inventory = { phase: "inventory", destructive: false };
    const original = structuredClone([inventory, settings, secrets]);
    expect(validateDispatch(context, inventory, A)).toBe("inventory");
    expect(validateTarget(inventory, settings, secrets)).toBe("inventory");
    expect([inventory, settings, secrets]).toEqual(original);
    for (const value of ["false", "true", 0, 1, null]) {
      denied("INVALID_BOOLEAN", () =>
        validateDispatch(context, { ...inventory, destructive: value }, A),
      );
      denied("INVALID_BOOLEAN", () =>
        validateTarget({ ...inventory, destructive: value }, settings, secrets),
      );
    }
    for (const phase of PHASES) {
      if (phase === "connection" || phase === "inventory" || phase === "ui-baseline") continue;
      const given = { phase, destructive: false };
      denied("DESTRUCTIVE_CONFIRMATION_REQUIRED", () => validateDispatch(context, given, A));
      for (const [flag, allow] of [
        [false, "false"],
        [false, "true"],
        [true, "false"],
        [true, ""],
      ] as const) {
        denied("DESTRUCTIVE_NOT_ALLOWED", () =>
          validateTarget(
            { ...given, destructive: flag },
            { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow },
            secrets,
          ),
        );
      }
      const confirmed = { ...given, destructive: true };
      expect(validateDispatch(context, confirmed, A)).toBe(phase);
      expect(
        validateTarget(confirmed, { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" }, secrets),
      ).toBe(phase);
      if (!["migration", "reset", "ui-ack"].includes(phase))
        denied("NOT_IMPLEMENTED", () => {
          requireImplemented(phase);
        });
    }
  });

  test("remote UI baseline keeps both read-only gates", () => {
    const baseline = { phase: "ui-baseline", destructive: false };
    expect(validateDispatch(context, baseline, A)).toBe("ui-baseline");
    expect(validateTarget(baseline, settings, secrets)).toBe("ui-baseline");
    for (const [flag, allow] of [
      [true, "false"],
      [false, "true"],
      [true, "true"],
    ] as const) {
      expect(() =>
        validateTarget(
          { ...baseline, destructive: flag },
          { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow },
          secrets,
        ),
      ).toThrow(AdmissionError);
    }
  });

  test("TLS primary configuration and input unchanged", () => {
    const original = structuredClone([inputs, settings, secrets]);
    expect(validateTarget(inputs, settings, secrets)).toBe("connection");
    expect([inputs, settings, secrets]).toEqual(original);
    const https = { ...secrets, FVOCI_TEST_TURSO_DATABASE_URL: "https://" + host + "/" };
    expect(validateTarget(inputs, settings, https)).toBe("connection");
    // The scheme is case-folded exactly as before; the host is not.
    expect(
      validateTarget(inputs, settings, {
        ...secrets,
        FVOCI_TEST_TURSO_DATABASE_URL: "LIBSQL://" + host,
      }),
    ).toBe("connection");
  });

  test("both missing secrets fail closed", () => {
    for (const key of Object.keys(secrets)) {
      for (const value of ["", null, undefined]) {
        denied("MISSING_SECRET", () =>
          validateTarget(inputs, settings, { ...secrets, [key]: value }),
        );
      }
    }
  });

  test("no URL credentials, query, fragment, port or local fallback", () => {
    for (const url of [
      "http://" + host,
      "file:///tmp/database",
      "libsql://localhost",
      "libsql://127.0.0.1",
      "libsql://other.example.org",
      "libsql://user:password@" + host,
      "https://" + host + ":443",
      "https://" + host + "/replica",
      "https://" + host + "?token=FAKE",
      "https://" + host + "#FAKE",
      "https://" + host + "?",
      "https://" + host + "#",
      "https://" + host + "\\@wrong.turso.io",
      "https://[broken",
      "libsql:" + host,
      "libsql://@" + host,
      "libsql://" + host + ":",
      "libsql://" + "a".repeat(250) + ".turso.io",
    ]) {
      denied("INVALID_PRIMARY_URL", () =>
        validateTarget(inputs, settings, { ...secrets, FVOCI_TEST_TURSO_DATABASE_URL: url }),
      );
    }
  });

  test("non-primary host shape denied", () => {
    for (const name of [
      "localhost",
      "127.0.0.1",
      "owner.example.org",
      "owner.turso.io.evil.org",
      "OWNER.turso.io",
      "turso.io",
    ]) {
      denied("INVALID_PRIMARY_URL", () =>
        validateTarget(inputs, settings, {
          ...secrets,
          FVOCI_TEST_TURSO_DATABASE_URL: "https://" + name,
        }),
      );
    }
  });

  test("control characters and oversize are not truncated", () => {
    for (const token of ["FAKE\nTOKEN", " FAKE", "FAKE\x7f", "x".repeat(16385)]) {
      denied("INVALID_SECRET_FORMAT", () =>
        validateTarget(inputs, settings, { ...secrets, FVOCI_TEST_TURSO_AUTH_TOKEN: token }),
      );
    }
    // Limits count code points, not UTF-16 units.
    const astral = String.fromCodePoint(0x1f600).repeat(16384);
    expect(
      validateTarget(inputs, settings, { ...secrets, FVOCI_TEST_TURSO_AUTH_TOKEN: astral }),
    ).toBe("connection");
  });

  test("destructive requires both gates", () => {
    for (const [flag, allow] of [
      [false, "true"],
      [true, "false"],
      [true, "TRUE"],
      [true, ""],
    ] as const) {
      denied("DESTRUCTIVE_NOT_ALLOWED", () =>
        validateTarget(
          { ...inputs, phase: "crud", destructive: flag },
          { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: allow },
          secrets,
        ),
      );
    }
    expect(
      validateTarget(
        { ...inputs, phase: "crud", destructive: true },
        { FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE: "true" },
        secrets,
      ),
    ).toBe("crud");
  });

  test("two secrets: connection needs no host name or allow variable", () => {
    expect(validateTarget(inputs, {}, secrets)).toBe("connection");
    denied("DESTRUCTIVE_NOT_ALLOWED", () =>
      validateTarget({ ...inputs, phase: "crud", destructive: true }, {}, secrets),
    );
  });

  test("pure validation does not print inputs", () => {
    const writes: string[] = [];
    const out = process.stdout.write.bind(process.stdout);
    const err = process.stderr.write.bind(process.stderr);
    process.stdout.write = (chunk: string) => writes.push(chunk) > 0;
    process.stderr.write = (chunk: string) => writes.push(chunk) > 0;
    try {
      denied("INVALID_PRIMARY_URL", () =>
        validateTarget(inputs, settings, {
          ...secrets,
          FVOCI_TEST_TURSO_DATABASE_URL: "https://FAKE_SECRET_SENTINEL@wrong.turso.io",
        }),
      );
    } finally {
      process.stdout.write = out;
      process.stderr.write = err;
    }
    expect(writes).toEqual([]);
  });
});

describe("environment metadata", () => {
  test("preexisting environment with the current null policy is allowed", () => {
    const environment = { name: "fvoci-turso-test", id: 123, deployment_branch_policy: null };
    expect(validateEnvironment(environment, "123")).toBe("123");
    // Python printed the exact int; no rounding beyond 2^53.
    expect(validateEnvironment({ ...environment, id: 2 ** 64 }, "12345678901234567891")).toBe(
      "12345678901234567891",
    );
    for (const [changed, token] of [
      [{}, null],
      [{ ...environment, id: 0 }, "0"],
      [{ ...environment, id: -5 }, "-5"],
      [{ ...environment, name: "other" }, "123"],
      [{ ...environment, id: true }, null],
    ] as const) {
      denied("ENVIRONMENT_POLICY_DENIED", () => validateEnvironment(changed, token));
    }
  });

  test("only an integer id literal is an integer", () => {
    const body = (text: string) => parseEnvironmentBody(new TextEncoder().encode(text));
    expect(body('{"name":"fvoci-turso-test","id":123}').idText).toBe("123");
    expect(body('{"name":"fvoci-turso-test","id":12345678901234567891}').idText).toBe(
      "12345678901234567891",
    );
    expect(body('{"name":"fvoci-turso-test","id":123.0}').idText).toBeNull();
    expect(body('{"name":"fvoci-turso-test","id":1e3}').idText).toBeNull();
    expect(body('{"name":"x","nested":{"id":5},"id":"5"}').idText).toBeNull();
    denied("ENVIRONMENT_POLICY_DENIED", () => {
      const parsed = body('{"name":"fvoci-turso-test","id":5.0}');
      validateEnvironment(parsed.value, parsed.idText);
    });
    expect(() => body("[1]")).toThrow();
    expect(() => parseEnvironmentBody(new Uint8Array([0x7b, 0xff, 0x7d]))).toThrow();
  });
});

describe("mode routing", () => {
  test("only the four explicit modes", () => {
    for (const mode of ["--admit", "--freeze", "--diagnostic-unit", "--consume"])
      expect<string>(parseMode([mode])).toBe(mode);
    for (const argv of [[], ["--bogus"], ["--admit", "--consume"], ["consume"]]) {
      denied("EXPLICIT_MODE_REQUIRED", () => parseMode(argv));
    }
  });

  test("explicit unit mode routes before any secret consumer", () => {
    const route = (event_name: string, sha: string) =>
      decide(
        "--diagnostic-unit",
        { ...context, event_name, ref: REVIEWED_REF, sha },
        { phase: "connection", destructive: false },
        A,
      );
    expect(route("workflow_dispatch", A)).toEqual({ kind: "diagnostic-unit" });
    denied("SECRET_MODE_REQUIRES_MANUAL", () => route("push", A));
    denied("CHECKOUT_MISMATCH", () => route("workflow_dispatch", "b".repeat(40)));
  });

  test("push admits source only", () => {
    const push = { ...context, event_name: "push", ref: REVIEWED_REF };
    expect(decide("--admit", push, {}, A)).toEqual({ kind: "bootstrap" });
    for (const mode of ["--freeze", "--diagnostic-unit", "--consume"] as const) {
      denied("SECRET_MODE_REQUIRES_MANUAL", () => decide(mode, push, {}, A));
    }
  });

  test("inventory manual dispatch routes to the inventory consumer only", () => {
    for (const [event_name, repository, ref, sha, destructive, routed] of [
      ["workflow_dispatch", "AISFlow/fvoci", "refs/heads/main", A, "false", true],
      ["workflow_dispatch", "AISFlow/fvoci", REVIEWED_REF, A, "false", true],
      ["push", "AISFlow/fvoci", REVIEWED_REF, A, "false", false],
      ["pull_request", "AISFlow/fvoci", "refs/heads/main", A, "false", false],
      ["workflow_dispatch", "attacker/fvoci", "refs/heads/main", A, "false", false],
      ["workflow_dispatch", "AISFlow/fvoci", "refs/heads/topic", A, "false", false],
      ["workflow_dispatch", "AISFlow/fvoci", "refs/heads/main", "b".repeat(40), "false", false],
      ["workflow_dispatch", "AISFlow/fvoci", "refs/heads/main", A, "true", false],
    ] as const) {
      const given = eventInputs({ inputs: { phase: "inventory", destructive } });
      const route = () => decide("--consume", { event_name, repository, ref, sha }, given, A);
      if (routed) expect(route()).toEqual({ kind: "consume", phase: "inventory" });
      else expect(route).toThrow(AdmissionError);
    }
  });

  test("event inputs: string booleans only, dict() semantics otherwise", () => {
    expect(eventInputs({ inputs: { phase: "inventory", destructive: "true" } })).toEqual({
      phase: "inventory",
      destructive: true,
    });
    expect(eventInputs({})).toEqual({ destructive: false });
    expect(eventInputs({ inputs: [["phase", "reset"]] })).toEqual({
      phase: "reset",
      destructive: false,
    });
    expect(eventInputs({ inputs: "" })).toEqual({ destructive: false });
    // A "__proto__" pair is an inert key, never an inherited ui_source_sha.
    const smuggled = eventInputs({
      inputs: [
        ["__proto__", { ui_source_sha: A }],
        ["phase", "ui-baseline"],
      ],
    });
    denied("UI_REVIEWED_SOURCE_REQUIRED", () => {
      validateUiInputs("ui-baseline", smuggled, A);
    });
    for (const value of [true, "TRUE", null, 1]) {
      denied("INVALID_BOOLEAN", () => eventInputs({ inputs: { destructive: value } }));
    }
    for (const event of [
      null,
      [],
      "x",
      { inputs: null },
      { inputs: 5 },
      { inputs: "ab" },
      { inputs: [["a"]] },
    ]) {
      expect(() => eventInputs(event)).toThrow(TypeError);
    }
  });
});
