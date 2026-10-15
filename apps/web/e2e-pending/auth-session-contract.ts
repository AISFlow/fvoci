import { isDeepStrictEqual } from "node:util";

export const AUTH_SPEC = "auth-session-selected-backend.acceptance.ts";
export const AUTH_CASES = [
  "auth session: logout revokes only ending session",
  "auth session: password change keeps current and revokes others",
  "auth session: captured reset revokes all user sessions and rejects tampered/spent tokens",
] as const;

export interface AuthNoEffect {
  targetId: string;
  body: unknown;
  ids: string[];
  nativeV1: string;
  revisions: unknown[];
  events: unknown[];
}

const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/i;
const object = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

function observed(value: unknown): asserts value is AuthNoEffect {
  if (!(
    object(value) &&
    typeof value.targetId === "string" &&
    uuid.test(value.targetId) &&
    object(value.body) &&
    object(value.body.contentJson) &&
    Array.isArray(value.ids) &&
    value.ids.length > 0 &&
    value.ids.every((id) => typeof id === "string" && uuid.test(id)) &&
    new Set(value.ids).size === value.ids.length &&
    typeof value.nativeV1 === "string" &&
    value.nativeV1.length > 4 &&
    Array.isArray(value.revisions) &&
    value.revisions.length > 0 &&
    value.revisions.every(
      (r) =>
        object(r) &&
        r.targetId === value.targetId &&
        typeof r.id === "string" &&
        uuid.test(r.id) &&
        typeof r.ySnapshot === "string",
    ) &&
    Array.isArray(value.events) &&
    value.events.length > 0 &&
    value.events.every(
      (e) =>
        object(e) && e.targetId === value.targetId && typeof e.id === "string" && uuid.test(e.id),
    )
  ))
    throw new Error("UI_AUTH_EFFECT_OBSERVATION_MISSING");
}

/** Complete fresh observations only; undefined/empty never means zero effects. */
export function assertAuthNoEffect(before: unknown, after: unknown): void {
  observed(before);
  observed(after);
  if (!isDeepStrictEqual(before, after)) throw new Error("UI_AUTH_REFUSED_MUTATION_EFFECT");
}
