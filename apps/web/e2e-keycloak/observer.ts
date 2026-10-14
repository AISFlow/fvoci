// Fixed native read-only observations. Database credentials reach only this
// native child, never a browser or the normal FVOCI server's environment.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

export type Observation =
  | { operation: "users" | "states" | "links" }
  | { operation: "user" | "sessions" | "invitation"; email: string }
  | { operation: "membership"; email: string; slug: string };

export type LinkRow = {
  user: string;
  provider: string;
  subject: string;
  issuer: string | null;
  email: string | null;
};
type Values = {
  users: number;
  states: number;
  links: LinkRow[];
  user: string | null;
  sessions: number;
  invitation: boolean | null;
  membership: string | null;
};
export function observeDatabase<K extends keyof Values>(
  input: Observation & { operation: K },
): Values[K] {
  try {
    const binary = process.env.FVOCI_KC_OBSERVER_BIN;
    const capsule = process.env.FVOCI_KC_OBSERVER_ENV;
    if (!binary || !capsule) throw new Error();
    const environment = JSON.parse(readFileSync(capsule, "utf8")) as Record<string, string>;
    const bytes = execFileSync(binary, ["oidc-observe"], {
      env: environment,
      input: JSON.stringify(input),
      encoding: "utf8",
      stdio: ["pipe", "pipe", "ignore"],
      timeout: 30_000,
      maxBuffer: 1024 * 1024,
    });
    const result = JSON.parse(bytes) as {
      value: Values[K];
      rollback: string;
      lifecycleDrain: string;
      leases: number;
    };
    if (
      result.rollback !== "confirmed" ||
      result.lifecycleDrain !== "confirmed" ||
      result.leases !== 0
    )
      throw new Error();
    return result.value;
  } catch {
    // exec errors quote argv, output and environment; none are diagnostic text.
    throw new Error("Keycloak native read-only observer failed");
  }
}
