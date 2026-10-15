import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  loadKcConfig,
  type PreparedActors,
  type PreparedInstance,
} from "../../apps/web/e2e-keycloak/config.ts";
import {
  assertPreparedDelta,
  type PreparedSnapshot,
} from "../../apps/web/e2e-keycloak/observer.ts";
import {
  assertPreparedBinding,
  consumeKeycloak,
  validatePreparedInputs,
  type ConsumerInputs,
} from "./run.ts";
import { ENV } from "./fixtures.ts";
import { specConfig } from "./realm.ts";

let dir = "";
beforeAll(() => {
  dir = mkdtempSync(join(tmpdir(), "kc-spec-config-"));
});
afterAll(() => {
  rmSync(dir, { recursive: true, force: true });
});

function load(value: unknown) {
  const path = join(dir, "config.json");
  writeFileSync(path, JSON.stringify(value));
  return () => loadKcConfig(path);
}

const valid = () => specConfig("http://127.0.0.1:1/realms/fvoci-e2e", ENV);
const uuid = (n: number) => `00000000-0000-0000-0000-${String(n).padStart(12, "0")}`;
function actors(index = 1): PreparedActors {
  const prefix = `prepared-${String(index)}`;
  return {
    owner: {
      id: uuid(index),
      email: `${prefix}-owner@example.com`,
      password: ENV.KC_E2E_FVOCI_OWNER_PASSWORD ?? "",
    },
    member: {
      email: `${prefix}-member@example.com`,
      password: ENV.KC_E2E_FVOCI_MEMBER_PASSWORD ?? "",
    },
    workspace: { id: uuid(index + 10), slug: prefix, name: prefix },
    users: Object.fromEntries(
      ["alice", "bob", "carol", "mallory", "erin", "tina"].map((key) => [
        key,
        { username: `${prefix}-${key}`, email: `${prefix}-${key}@example.com` },
      ]),
    ) as PreparedActors["users"],
    mismatchEmail: `${prefix}-dave@example.com`,
    crossOriginEmail: `${prefix}-xorigin@example.com`,
  };
}
function prepared(): PreparedInstance {
  const a = actors();
  return {
    binding: {
      source: "a".repeat(40),
      tree: "b".repeat(40),
      target: "admitted-primary",
      group: "flows",
      issuer: String(valid().issuer),
    },
    actors: a,
    idpBefore: [],
    baseline: {
      users: [
        { id: a.owner.id, email: a.owner.email, rowFingerprint: "c".repeat(64) },
        { id: uuid(99), email: "foreign@example.com", rowFingerprint: "d".repeat(64) },
      ],
      links: [
        {
          user: "foreign@example.com",
          provider: "generic",
          subject: uuid(98),
          issuer: "original-issuer",
          email: "foreign@example.com",
        },
      ],
    },
  };
}
function preparedConfig() {
  const p = prepared(),
    config = valid();
  return {
    ...config,
    prepared: p,
    users: Object.fromEntries(
      Object.entries(config.users as Record<string, Record<string, string>>).map(([key, user]) => [
        key,
        { ...user, ...p.actors.users[key as keyof PreparedActors["users"]] },
      ]),
    ),
  };
}
function inputs(): ConsumerInputs {
  return {
    manifest: {
      sourceInputs: { source: "a".repeat(40), tree: "b".repeat(40) },
    } as ConsumerInputs["manifest"],
    directory: "unused",
    processObserver: {} as ConsumerInputs["processObserver"],
    checkInputs: () => {},
    prepare: () => {
      throw new Error("runtime touched");
    },
    prepared: {
      source: "a".repeat(40),
      tree: "b".repeat(40),
      target: "admitted-primary",
      groups: { flows: actors(1), failures: actors(2), "wrong-secret": actors(3) },
      observe: () => {
        throw new Error("runtime touched");
      },
      assertWholePrimary: async () => {},
    },
  };
}

describe("Keycloak spec config", () => {
  test("only an unset or empty path disables the opt-in suite", () => {
    expect(loadKcConfig(undefined)).toBeNull();
    expect(loadKcConfig("")).toBeNull();
  });

  test("a configured missing or unreadable file fails before tests can skip", () => {
    const directory = join(dir, "directory");
    mkdirSync(directory);
    for (const path of [join(dir, "missing.json"), directory, " "]) {
      expect(() => loadKcConfig(path)).toThrow("FVOCI_KC_E2E_CONFIG cannot be read");
    }
  });

  test("invalid JSON and UTF-8 report no file path or config contents", () => {
    const canary = "config-content-canary-012345";
    const path = join(dir, `${canary}.json`);
    for (const content of [`{"password":"${canary}",`, new Uint8Array([0xff])]) {
      writeFileSync(path, content);
      let message = "";
      try {
        loadKcConfig(path);
      } catch (error) {
        message = error instanceof Error ? error.message : String(error);
      }
      expect(message).toStartWith("FVOCI_KC_E2E_CONFIG");
      expect(message).not.toContain(canary);
      expect(message).not.toContain(path);
    }
  });

  test("a runner-generated config supplies every field consumed by the spec", () => {
    const actual: unknown = load(valid())();
    expect(actual).toEqual(valid());
  });

  test("a prepared config cannot silently fall back to fresh setup", () => {
    for (const prepared of [
      null,
      true,
      {},
      { verified: true },
      { binding: { group: "foreign" } },
    ]) {
      expect(load({ ...valid(), prepared })).toThrow("Keycloak prepared instance");
    }
  });

  test("prepared actor/issuer bindings and whole foreign rows survive config transport", () => {
    expect(load(preparedConfig())()).toEqual(preparedConfig());
    for (const mutate of [
      (p: PreparedInstance) => {
        p.actors.owner.id = uuid(77);
      },
      (p: PreparedInstance) => {
        p.actors.workspace.slug = "../foreign";
      },
      (p: PreparedInstance) => {
        p.actors.users.alice.email = p.actors.owner.email;
      },
      (p: PreparedInstance) => {
        p.binding.issuer = "foreign-issuer";
      },
      (p: PreparedInstance) => {
        p.binding.group = "unknown" as PreparedInstance["binding"]["group"];
      },
      (p: PreparedInstance) => {
        p.baseline.users.push({ ...p.baseline.users[0]! });
      },
    ]) {
      const c = preparedConfig();
      mutate(c.prepared);
      expect(load(c)).toThrow("Keycloak prepared instance");
    }
  });

  test("source/tree/missing native callback and cross-group identity errors fail before runtime", async () => {
    expect(() => validatePreparedInputs(inputs())).not.toThrow();
    for (const mutate of [
      (p: NonNullable<ConsumerInputs["prepared"]>) => {
        p.source = "f".repeat(40);
      },
      (p: NonNullable<ConsumerInputs["prepared"]>) => {
        p.tree = "f".repeat(40);
      },
      (p: NonNullable<ConsumerInputs["prepared"]>) => {
        p.target = "";
      },
      (p: NonNullable<ConsumerInputs["prepared"]>) => {
        p.observe = undefined as unknown as typeof p.observe;
      },
      (p: NonNullable<ConsumerInputs["prepared"]>) => {
        p.groups.failures = p.groups.flows;
      },
    ]) {
      const c = inputs();
      mutate(c.prepared!);
      await expect(
        consumeKeycloak(null as unknown as Parameters<typeof consumeKeycloak>[0], c),
      ).rejects.toThrow("Keycloak prepared");
    }
  });

  test("the current prepared backend must match its admitted target, source, group and issuer", () => {
    const binding = prepared().binding;
    expect(() => assertPreparedBinding(binding, structuredClone(binding))).not.toThrow();
    expect(() => assertPreparedBinding(binding, undefined)).toThrow("Keycloak prepared");
    for (const key of ["source", "tree", "target", "group", "issuer"] as const) {
      expect(() => assertPreparedBinding(binding, { ...binding, [key]: "foreign" })).toThrow(
        "Keycloak prepared",
      );
    }
  });

  test("exact native global delta rejects foreign mutation, replacement, hidden extra rows and states", () => {
    const p = prepared();
    const before: PreparedSnapshot = {
      binding: p.binding,
      ...p.baseline,
      memberships: [],
      states: 0,
      wholePrimary: { allTables: "unfiltered" },
    };
    const member = {
      id: uuid(7),
      email: p.actors.users.carol.email,
      rowFingerprint: "e".repeat(64),
    };
    const link = {
      user: member.email,
      provider: "generic",
      subject: uuid(8),
      issuer: p.binding.issuer,
      email: member.email,
    };
    const delta = { emails: [member.email], links: [link] };
    const after = {
      ...structuredClone(before),
      users: [...before.users, member],
      links: [...before.links, link],
    };
    expect(() => assertPreparedDelta(before, after, delta)).not.toThrow();
    for (const mutate of [
      (a: PreparedSnapshot) => {
        a.binding.target = "another-primary";
      },
      (a: PreparedSnapshot) => {
        a.binding.group = "failures";
      },
      (a: PreparedSnapshot) => {
        a.users[1]!.rowFingerprint = "f".repeat(64);
      },
      (a: PreparedSnapshot) => {
        a.users[1]!.id = uuid(88);
      },
      (a: PreparedSnapshot) => {
        a.users.push({ ...member, id: uuid(9), email: "extra@example.com" });
      },
      (a: PreparedSnapshot) => {
        a.links.shift();
      },
      (a: PreparedSnapshot) => {
        a.links[0]!.subject = uuid(66);
      },
      (a: PreparedSnapshot) => {
        a.states = 1;
      },
    ]) {
      const changed = structuredClone(after);
      mutate(changed);
      expect(() => assertPreparedDelta(before, changed, delta)).toThrow(
        "Keycloak prepared global delta differs",
      );
    }
  });

  test("non-object configs and missing nested sections fail at load time", () => {
    for (const value of [null, [], 1, "config", {}, { ...valid(), admin: [] }]) {
      expect(load(value)).toThrow("FVOCI_KC_E2E_CONFIG");
    }
  });

  test("every consumed field is required to be a nonempty string", () => {
    const fields = [
      "issuer",
      "realm",
      "clientId",
      "label",
      "keycloakOrigin",
      "admin.username",
      "admin.password",
      "fvoci.ownerPassword",
      "fvoci.memberPassword",
      ...["alice", "bob", "carol", "mallory", "erin", "tina"].flatMap((user) =>
        ["username", "password", "email"].map((key) => `users.${user}.${key}`),
      ),
    ];
    for (const field of fields) {
      for (const invalid of [undefined, null, 1, false, "", [], {}]) {
        const config = valid();
        const keys = field.split(".");
        const key = keys.pop() ?? "";
        let object = config;
        for (const part of keys) object = object[part] as Record<string, unknown>;
        if (invalid === undefined) Reflect.deleteProperty(object, key);
        else object[key] = invalid;
        expect(load(config)).toThrow(`FVOCI_KC_E2E_CONFIG requires a nonempty string at ${field}`);
      }
    }
  });
});
