import { readFileSync } from "node:fs";

const USER_KEYS = ["alice", "bob", "carol", "mallory", "erin", "tina"] as const;
export type KcUserKey = (typeof USER_KEYS)[number];
export type KcUser = { username: string; password: string; email: string };
export type KcConfig = {
  issuer: string;
  realm: string;
  clientId: string;
  label: string;
  keycloakOrigin: string;
  admin: { username: string; password: string };
  users: Record<KcUserKey, KcUser>;
  /** Passwords of the FVOCI accounts this spec creates (the runner's redactor knows them). */
  fvoci: { ownerPassword: string; memberPassword: string };
};

export function loadKcConfig(configPath: string | undefined): KcConfig | null {
  if (!configPath) return null;
  let bytes: Uint8Array;
  try {
    bytes = readFileSync(configPath);
  } catch {
    // Paths and parser messages can contain credentials; report fixed labels only.
    throw new Error("FVOCI_KC_E2E_CONFIG cannot be read");
  }
  let config: unknown;
  try {
    config = JSON.parse(new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(bytes));
  } catch {
    throw new Error("FVOCI_KC_E2E_CONFIG must contain valid UTF-8 JSON");
  }
  for (const field of ["issuer", "realm", "clientId", "label", "keycloakOrigin"]) {
    requireString(config, field);
  }
  for (const field of ["username", "password"]) requireString(config, "admin", field);
  for (const field of ["ownerPassword", "memberPassword"]) requireString(config, "fvoci", field);
  for (const user of USER_KEYS) {
    for (const field of ["username", "password", "email"]) {
      requireString(config, "users", user, field);
    }
  }
  return config as KcConfig;
}

function requireString(config: unknown, ...path: string[]): void {
  let member = config;
  for (const key of path) {
    member =
      member !== null &&
      typeof member === "object" &&
      !Array.isArray(member) &&
      Object.hasOwn(member, key)
        ? (member as Record<string, unknown>)[key]
        : undefined;
  }
  if (typeof member !== "string" || member.length === 0) {
    throw new Error(`FVOCI_KC_E2E_CONFIG requires a nonempty string at ${path.join(".")}`);
  }
}
