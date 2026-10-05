/** Literal actor/input fixture shared by current normal ON and OFF specs.
 * Root owns real backend/native/role/FK/resource preparation; no fallback. */
import { execFileSync } from "node:child_process";
import { lstatSync } from "node:fs";
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
  if (selected !== "sqlite") throw new Error("unsupported selected actor fixture");
  const binary = requiredFixtureInput("FVOCI_E2E_SELECTED_FIXTURE_BIN");
  if (!isAbsolute(binary) || !lstatSync(binary).isFile()) {
    throw new Error("selected fixture binary must be an explicit absolute regular file");
  }
  // Pass only the owned test DB, synthetic actor and matching test keyring.
  // Do not inherit operator DB URLs, account credentials or global env writes.
  const user = execFileSync(binary, [], {
    env: {
      PATH: process.env.PATH,
      LANG: process.env.LANG,
      E2E_DATABASE_BACKEND: "sqlite",
      FVOCI_E2E_SQLITE_RUN_ROOT: requiredFixtureInput("FVOCI_E2E_SQLITE_RUN_ROOT"),
      FVOCI_E2E_SQLITE_PATH: requiredFixtureInput("FVOCI_E2E_SQLITE_PATH"),
      PASSWORD_PEPPER_KEYS: requiredFixtureInput("PASSWORD_PEPPER_KEYS"),
      PASSWORD_PEPPER_ACTIVE_KEY_ID: requiredFixtureInput("PASSWORD_PEPPER_ACTIVE_KEY_ID"),
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
