import { get, isMapping, sameKeys, triggersOf, type Mapping } from "./load.ts";
import { RELEASE_WRITE_SCOPES, verifyWorkflowWriteScopes } from "./scopes.ts";

export const RELEASE_WORKFLOW_FILE = "release.yml";

/** Only tag pushes and manual dispatch; read-only default token; scoped writes. */
export function verifyReleaseWorkflow(data: Mapping, name = RELEASE_WORKFLOW_FILE): string[] {
  const errors: string[] = [];
  const triggers = triggersOf(data);
  if (!sameKeys(triggers, ["push", "workflow_dispatch"])) {
    errors.push(`${name}: triggers must be exactly push (tags) and workflow_dispatch`);
  } else {
    const push = get(triggers, "push");
    const tags = get(push, "tags");
    if (
      !sameKeys(push, ["tags"]) ||
      !Array.isArray(tags) ||
      tags.length === 0 ||
      !tags.every((tag) => typeof tag === "string" && tag.startsWith("v0."))
    ) {
      errors.push(`${name}: push must list only v0.* tags`);
    }
  }
  // One queue for every tag: runs for two patch tags must not race on :0.y.
  const concurrency = get(data, "concurrency");
  const group = get(concurrency, "group");
  if (
    !isMapping(concurrency) ||
    typeof group !== "string" ||
    group.includes("${{") ||
    get(concurrency, "cancel-in-progress") !== false
  ) {
    errors.push(`${name}: concurrency must be one fixed group with cancel-in-progress: false`);
  }
  return [...errors, ...verifyWorkflowWriteScopes(data, name, RELEASE_WRITE_SCOPES)];
}
