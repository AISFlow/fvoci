import { execFileSync } from "node:child_process";
import path from "node:path";

export function createE2eUser(email: string, password: string, givenName: string): void {
  const root = path.resolve(import.meta.dirname, "../../..");
  const adminUrl = process.env.FVOCI_E2E_ADMIN_DATABASE_URL;
  if (!adminUrl) {
    throw new Error("FVOCI_E2E_ADMIN_DATABASE_URL is required for DB fixtures");
  }
  execFileSync(
    path.join(root, "target/debug/fvoci-e2e-fixture"),
    [],
    {
      env: {
        ...process.env,
        DATABASE_URL: adminUrl,
        PASSWORD_PEPPER_KEYS:
          process.env.PASSWORD_PEPPER_KEYS ??
          '{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}',
        PASSWORD_PEPPER_ACTIVE_KEY_ID: process.env.PASSWORD_PEPPER_ACTIVE_KEY_ID ?? "test",
        E2E_USER_EMAIL: email,
        E2E_USER_PASSWORD: password,
        E2E_USER_GIVEN_NAME: givenName,
      },
      stdio: "pipe",
    },
  );
}
