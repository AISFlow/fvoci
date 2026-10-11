// Shared fixtures of the helper's tests: per-run secrets, the fake admin
// token, and the CLI runner.
import { join } from "node:path";

export const HELPER = join(import.meta.dir, "kc-e2e.ts");
export const ENV: Record<string, string> = {
  KC_BOOTSTRAP_ADMIN_PASSWORD: "admin-pass-0123456789",
  KC_E2E_CLIENT_SECRET: "client-secret-0123456789",
  KC_E2E_WRONG_SECRET: "wrong-secret-0123456789",
  KC_E2E_FVOCI_OWNER_PASSWORD: "owner-pass-0123",
  KC_E2E_FVOCI_MEMBER_PASSWORD: "member-pass-0123",
  KC_E2E_SSO_A_CLIENT_SECRET: "sso-a-secret-0123",
  KC_E2E_SSO_A_PASSWORD: "sso-a-pass-0123",
  KC_E2E_SSO_B_CLIENT_SECRET: "sso-b-secret-0123",
  KC_E2E_SSO_B_PASSWORD: "sso-b-pass-0123",
};
for (const user of ["ALICE", "BOB", "CAROL", "MALLORY", "ERIN", "TINA"]) {
  ENV[`KC_E2E_PASSWORD_${user}`] = `${user.toLowerCase()}-pass-0123`;
}

export const secret = (name: string) => ENV[name] ?? "";
// The admin access token the fake Keycloaks issue (a credential the helper
// learns at run time, not from its config).
export const ADMIN_TOKEN = "admin-access-token-0123456789";
export const KNOWN_REALMS = new Set(["fvoci-e2e", "fvoci-e2e-ws-a", "fvoci-e2e-ws-b"]);

/** The message a promise rejects with ("" when it resolves). */
export async function failure(promise: Promise<unknown>): Promise<string> {
  try {
    await promise;
    return "";
  } catch (error) {
    return error instanceof Error ? error.message : String(error);
  }
}

export async function cli(
  args: string[],
  options: { env?: Record<string, string>; stdin?: string } = {},
) {
  const child = Bun.spawn([process.execPath, HELPER, ...args], {
    env: { PATH: process.env.PATH ?? "", ...options.env },
    stdin: options.stdin === undefined ? "ignore" : new TextEncoder().encode(options.stdin),
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { stdout, stderr, code };
}
