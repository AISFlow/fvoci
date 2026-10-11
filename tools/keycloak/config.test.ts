import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { loadKcConfig } from "../../apps/web/e2e-keycloak/config.ts";
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
