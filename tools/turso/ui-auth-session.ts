// Dedicated auth acceptance consumer. The existing ON1/restart1/OFF8 consumer
// retains its own discovery and counts. No process or database access here.
import { isDeepStrictEqual } from "node:util";
import { basename } from "node:path";
import { get, isRecord, list, record, require, type Record_ } from "./ui-common.ts";
import { attachment } from "./ui-audit.ts";

import {
  AUTH_SPEC,
  AUTH_CASES,
  assertAuthNoEffect,
} from "../../apps/web/e2e-pending/auth-session-contract";
export {
  AUTH_SPEC,
  AUTH_CASES,
  assertAuthNoEffect,
  type AuthNoEffect,
} from "../../apps/web/e2e-pending/auth-session-contract";

export interface AuthSessionBinding {
  backend: "postgres" | "sqlite" | "libsql-remote";
  flow: "on" | "off";
  source: string;
  tree: string;
}

const hex40 = /^[a-f0-9]{40}$/;

export function authSessionCases(report: unknown, binding: AuthSessionBinding): Record_[] {
  require(["postgres", "sqlite", "libsql-remote"].includes(binding.backend) &&
    ["on", "off"].includes(binding.flow) &&
    hex40.test(binding.source) &&
    hex40.test(binding.tree), "UI_AUTH_BINDING_REFUSED");
  require(get(report, "config", "workers") === 1 &&
    isDeepStrictEqual(get(report, "errors"), []) &&
    get(report, "config", "metadata", "authSession") === true &&
    get(report, "config", "metadata", "selectedBackend") === binding.backend &&
    get(report, "config", "metadata", "selectedFlow") === binding.flow &&
    get(report, "config", "metadata", "source") === binding.source &&
    get(report, "config", "metadata", "tree") === binding.tree, "UI_AUTH_BROWSER_BINDING_FAILED");
  const cases: Record_[] = [];
  const titles: string[] = [];
  function visit(suites: unknown[]): void {
    for (const suite of suites) {
      require(isRecord(suite), "UI_AUTH_BROWSER_REPORT_REFUSED");
      for (const spec of Object.hasOwn(suite, "specs") ? list(suite, "specs") : []) {
        require(isRecord(spec) &&
          typeof spec.file === "string" &&
          basename(spec.file) === AUTH_SPEC &&
          spec.ok === true &&
          typeof spec.title === "string" &&
          list(spec, "tests").length === 1, "UI_AUTH_BROWSER_CASE_FAILED");
        const test = get(spec, "tests", 0);
        require(get(test, "expectedStatus") === "passed" &&
          list(test, "results").length === 1, "UI_AUTH_BROWSER_CASE_FAILED");
        const result = get(test, "results", 0);
        require(isRecord(result) &&
          result.status === "passed" &&
          result.retry === 0 &&
          isDeepStrictEqual(result.errors, []), "UI_AUTH_BROWSER_CASE_FAILED");
        titles.push(spec.title);
        cases.push(result);
      }
      visit(Object.hasOwn(suite, "suites") ? list(suite, "suites") : []);
    }
  }
  visit(list(report, "suites"));
  require(isDeepStrictEqual(titles, [...AUTH_CASES]), "UI_AUTH_BROWSER_COUNTS_FAILED");
  return cases;
}

/** Browser effects are document-scoped. Global auth-event effects additionally
 * require the native primary audit below; a browser report alone is not PASS. */
export function authSessionWitnesses(report: unknown, binding: AuthSessionBinding): Record_[] {
  return authSessionCases(report, binding).map((result, index) => {
    const title = AUTH_CASES[index];
    require(title !== undefined, "UI_AUTH_BROWSER_COUNTS_FAILED");
    const witness = attachment([[title, result]], "auth-session-document-effects.json");
    require(isRecord(witness) &&
      witness.schema === 1 &&
      witness.scope === "document-effects" &&
      witness.backend === binding.backend &&
      witness.flow === binding.flow &&
      witness.source === binding.source &&
      witness.tree === binding.tree &&
      typeof witness.actor === "string" &&
      typeof witness.owner === "string" &&
      witness.actor !== witness.owner &&
      get(witness, "refusal", "boundary") === "pre-auth-http" &&
      get(witness, "refusal", "method") === (binding.flow === "on" ? "POST" : "PUT") &&
      get(witness, "refusal", "status") === 401 &&
      get(witness, "refusal", "directStatus") === 401, "UI_AUTH_WITNESS_REFUSED");
    require(witness.targetId === get(witness, "before", "targetId"), "UI_AUTH_WITNESS_REFUSED");
    assertAuthNoEffect(witness.before, witness.after);
    if (index > 0)
      require(witness.oldPasswordStatus === 401 &&
        witness.newPasswordStatus === 200, "UI_AUTH_PASSWORD_CONTROL_MISSING");
    if (index === 1)
      require(witness.wrongCurrentStatus === 400, "UI_AUTH_PASSWORD_CONTROL_MISSING");
    if (index === 2)
      require(witness.tamperedStatus === 400 &&
        witness.spentStatus === 400, "UI_AUTH_RESET_CONTROL_MISSING");
    return witness;
  });
}

/** Count exact new auth producer witnesses in the maintained remote baseline.
 * Full row fingerprints and preservation still belong to the existing audit. */
export function authSessionEventEffects(before: unknown, after: unknown, actorId: string): void {
  const actor = actorId.replaceAll("-", "").toLowerCase();
  require(/^[a-f0-9]{32}$/.test(actor) &&
    get(before, "backend") === "libsql-remote" &&
    get(after, "backend") === "libsql-remote", "UI_AUTH_EVENT_OBSERVATION_REFUSED");
  const old = record(before, "fingerprints", "events");
  const fresh = record(after, "fingerprints", "events");
  const allocations = record(after, "allocations", "events");
  const counts = { "actor-logout": 0, "actor-password-change": 0, "actor-password-reset": 0 };
  for (const [hash, refs] of Object.entries(allocations)) {
    if (Object.hasOwn(old, hash)) continue;
    if (!isRecord(refs) || !isRecord(refs.eventWitness)) continue;
    const witness = refs.eventWitness;
    if (typeof witness.class !== "string" || !Object.hasOwn(counts, witness.class)) continue;
    const identity = witness.class === "actor-logout" ? witness.actor : witness.target;
    require(/^[a-f0-9]{64}$/.test(hash) &&
      fresh[hash] === 1 &&
      identity === actor &&
      isDeepStrictEqual(refs.actors, [actor]) &&
      isDeepStrictEqual(refs.workspaces, []) &&
      Array.isArray(witness.sequence) &&
      witness.sequence[0] === "integer" &&
      typeof witness.sequence[1] === "string" &&
      /^[1-9][0-9]*$/.test(witness.sequence[1]), "UI_AUTH_EVENT_OBSERVATION_REFUSED");
    counts[witness.class as keyof typeof counts]++;
  }
  require(isDeepStrictEqual(counts, {
    "actor-logout": 1,
    "actor-password-change": 1,
    "actor-password-reset": 1,
  }), "UI_AUTH_EVENT_EFFECT_COUNTS_FAILED");
}
