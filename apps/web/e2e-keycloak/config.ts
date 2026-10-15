import { readFileSync } from "node:fs";
import type { LinkRow } from "./observer";

const USER_KEYS = ["alice", "bob", "carol", "mallory", "erin", "tina"] as const;
export type KcUserKey = (typeof USER_KEYS)[number];
export type KcUser = { username: string; password: string; email: string };
export type KcMode = "flows" | "failures" | "wrong-secret";
export type PreparedBinding = {
  source: string;
  tree: string;
  target: string;
  group: KcMode;
  issuer: string;
};
export type PreparedActors = {
  owner: { id: string; email: string; password: string };
  member: { email: string; password: string };
  workspace: { id: string; slug: string; name: string };
  users: Record<KcUserKey, { username: string; email: string }>;
  mismatchEmail: string;
  crossOriginEmail: string;
};
export type PreparedUserRow = { id: string; email: string; rowFingerprint: string };
export type PreparedInstance = {
  binding: PreparedBinding;
  actors: PreparedActors;
  baseline: { users: PreparedUserRow[]; links: LinkRow[] };
  idpBefore: Array<[string, string, boolean, string[]]>;
};
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
  /** Projection of the admitted in-memory caller; this file grants no runtime authority. */
  prepared?: PreparedInstance;
};

const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/;
export function validatePreparedActors(value: unknown): asserts value is PreparedActors {
  try {
    const actors = value as PreparedActors;
    for (const [section, fields] of [
      ["owner", ["id", "email", "password"]],
      ["member", ["email", "password"]],
      ["workspace", ["id", "slug", "name"]],
    ] as const) for (const field of fields) requireString(value, section, field);
    for (const key of ["mismatchEmail", "crossOriginEmail"])
      requireString(value, key);
    for (const user of USER_KEYS) for (const key of ["username", "email"])
      requireString(value, "users", user, key);
    if (!uuid.test(actors.owner.id) || !uuid.test(actors.workspace.id) ||
      !/^[a-z0-9][a-z0-9-]{1,62}$/.test(actors.workspace.slug)) throw new Error();
    const emails = [actors.owner.email, actors.member.email, actors.mismatchEmail,
      actors.crossOriginEmail, ...USER_KEYS.map((key) => actors.users[key].email)];
    if (new Set(emails).size !== emails.length ||
      emails.some((email) => !/^[a-zA-Z0-9._+-]+@example\.com$/.test(email)) ||
      new Set(USER_KEYS.map((key) => actors.users[key].username)).size !== USER_KEYS.length ||
      USER_KEYS.some((key) => !/^[a-z0-9][a-z0-9._-]{1,80}$/.test(actors.users[key].username) ||
        USER_KEYS.includes(actors.users[key].username as KcUserKey)))
      throw new Error();
  } catch {
    throw new Error("Keycloak prepared instance actors are invalid");
  }
}
export function validatePreparedInstance(value: unknown): asserts value is PreparedInstance {
  try {
    for (const key of ["source", "tree", "target", "group", "issuer"])
      requireString(value, "binding", key);
    const p = value as PreparedInstance;
    if (!/^[a-f0-9]{40}$/.test(p.binding.source) || !/^[a-f0-9]{40}$/.test(p.binding.tree) ||
      !["flows", "failures", "wrong-secret"].includes(p.binding.group)) throw new Error();
    validatePreparedActors(p.actors);
    if (!Array.isArray(p.baseline.users) || !Array.isArray(p.baseline.links) ||
      !Array.isArray(p.idpBefore)) throw new Error();
    const emails = [p.actors.member.email, p.actors.mismatchEmail, p.actors.crossOriginEmail,
      ...USER_KEYS.map((key) => p.actors.users[key].email)];
    const rows = p.baseline.users;
    if (rows.some((row) => !uuid.test(row.id) || typeof row.email !== "string" ||
      !/^[a-f0-9]{64}$/.test(row.rowFingerprint)) ||
      new Set(rows.map((row) => row.id)).size !== rows.length ||
      new Set(rows.map((row) => row.email)).size !== rows.length ||
      rows.filter((row) => row.id === p.actors.owner.id && row.email === p.actors.owner.email).length !== 1 ||
      rows.some((row) => emails.includes(row.email))) throw new Error();
    if (p.baseline.links.some((row) => row.user === p.actors.owner.email)) throw new Error();
  } catch {
    throw new Error("Keycloak prepared instance is invalid");
  }
}

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
  if (Object.hasOwn(config as object, "prepared")) {
    const c = config as KcConfig;
    validatePreparedInstance(c.prepared);
    if (c.prepared.binding.issuer !== c.issuer ||
      c.fvoci.ownerPassword !== c.prepared.actors.owner.password ||
      c.fvoci.memberPassword !== c.prepared.actors.member.password ||
      USER_KEYS.some((key) => c.users[key].username !== c.prepared?.actors.users[key].username ||
        c.users[key].email !== c.prepared?.actors.users[key].email))
      throw new Error("Keycloak prepared instance config differs");
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
