/** Literal actor/input fixture shared by current normal ON and OFF specs.
 * Root owns real backend/native/role/FK/resource preparation; no fallback. */
import { execFileSync } from "node:child_process";
import { lstatSync, readFileSync } from "node:fs";
import { isAbsolute } from "node:path";
import { expect } from "@playwright/test";
import { admin, member, installCollabMember } from "./collab-helpers";
import { UUID_RE } from "./collab-wire";

export function requiredFixtureInput(name: string): string {
  const value = process.env[name];
  if (!value?.trim()) throw new Error(`${name} is required for the selected actor fixture`);
  return value;
}

export function installSelectedMember(selected: string): string | undefined {
  if (selected === "postgres") {
    installCollabMember();
    return undefined;
  }
  if (selected !== "sqlite" && selected !== "libsql-remote")
    throw new Error("unsupported selected actor fixture");
  const binary = requiredFixtureInput("FVOCI_E2E_SELECTED_FIXTURE_BIN");
  if (!isAbsolute(binary) || !lstatSync(binary).isFile()) {
    throw new Error("selected fixture binary must be an explicit absolute regular file");
  }
  // Pass only the owned test DB, synthetic actor and matching test keyring.
  // Do not inherit operator DB URLs, account credentials or global env writes.
  const remote = selected === "libsql-remote";
  if (remote) selectedSetupNeeded();
  const user = execFileSync(binary, [], {
    env: {
      PATH: process.env.PATH,
      LANG: process.env.LANG,
      E2E_DATABASE_BACKEND: selected,
      ...(remote
        ? {
            FVOCI_E2E_TURSO_PRIVATE_INPUT: requiredFixtureInput("FVOCI_E2E_TURSO_PRIVATE_INPUT"),
            FVOCI_E2E_TURSO_NAMESPACE: requiredFixtureInput("FVOCI_E2E_TURSO_NAMESPACE"),
          }
        : {
            FVOCI_E2E_SQLITE_RUN_ROOT: requiredFixtureInput("FVOCI_E2E_SQLITE_RUN_ROOT"),
            FVOCI_E2E_SQLITE_PATH: requiredFixtureInput("FVOCI_E2E_SQLITE_PATH"),
            PASSWORD_PEPPER_KEYS: requiredFixtureInput("PASSWORD_PEPPER_KEYS"),
            PASSWORD_PEPPER_ACTIVE_KEY_ID: requiredFixtureInput("PASSWORD_PEPPER_ACTIVE_KEY_ID"),
          }),
      E2E_USER_EMAIL: member.email,
      E2E_USER_PASSWORD: member.password,
      E2E_USER_GIVEN_NAME: member.givenName,
      E2E_USER_FAMILY_NAME: member.familyName,
      E2E_WORKSPACE_SLUG: admin.workspaceSlug,
      E2E_MEMBERSHIP_ROLE: "member",
    },
    encoding: "utf8",
    stdio: "pipe",
  }).trim();
  expect(user).toMatch(UUID_RE);
  return user;
}

export function selectedSetupNeeded(): boolean {
  if (process.env.FVOCI_E2E_SELECTED_BACKEND !== "libsql-remote") return true;
  const path = requiredFixtureInput("FVOCI_E2E_TURSO_ACTOR_BINDING");
  const metadata = lstatSync(path);
  if (
    !isAbsolute(path) ||
    !metadata.isFile() ||
    metadata.isSymbolicLink() ||
    metadata.nlink !== 1 ||
    (metadata.mode & 0o777) !== 0o600 ||
    metadata.uid !== process.getuid?.() ||
    metadata.size > 16384
  ) {
    throw new Error("remote actor binding must be an owned bounded private regular file");
  }
  const binding: unknown = JSON.parse(readFileSync(path, "utf8"));
  if (
    typeof binding !== "object" ||
    binding === null ||
    !("setupNeeded" in binding) ||
    typeof binding.setupNeeded !== "boolean"
  )
    throw new Error("remote setup binding must capture the actual boolean state");
  const needed = binding.setupNeeded;
  expect(binding).toMatchObject({
    schema: 1,
    backend: "libsql-remote",
    setupNeeded: needed,
    namespace: requiredFixtureInput("FVOCI_E2E_TURSO_NAMESPACE"),
    ownerEmail: admin.email,
    memberEmail: member.email,
    workspaceSlug: admin.workspaceSlug,
    source: requiredFixtureInput("FVOCI_E2E_TURSO_SOURCE"),
    tree: requiredFixtureInput("FVOCI_E2E_TURSO_TREE"),
    schemaCurrent: true,
    commit: needed ? "not-attempted" : "confirmed",
    lifecycleDrain: "confirmed",
    leases: 0,
  });
  return needed;
}
