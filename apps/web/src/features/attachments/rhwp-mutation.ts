/**
 * What `@rhwp/core` `replaceOne` / `replaceAll` returned (source
 * `rhwp-mutation.ts`). Both answer a JSON string: `{"ok":true,"count":n}`
 * from `replaceAll`, `{"ok":true,...position}` from `replaceOne`, and
 * `{"ok":false}` when nothing was replaced (0.8.6 `replaceOne` with no match).
 */
export type RhwpMutationResult = { ok: true; count?: number } | { ok: false; reason: string };

function readMutationBody(value: unknown): RhwpMutationResult | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
  const body = value as { ok?: unknown; count?: unknown };
  if (body.ok !== true) return { ok: false, reason: "not_ok" };
  if (body.count === undefined) return { ok: true };
  if (typeof body.count !== "number" || !Number.isInteger(body.count) || body.count < 0) {
    return { ok: false, reason: "invalid_count" };
  }
  return { ok: true, count: body.count };
}

export function parseRhwpMutation(json: string): RhwpMutationResult {
  try {
    return readMutationBody(JSON.parse(json)) ?? { ok: false, reason: "invalid_shape" };
  } catch {
    return { ok: false, reason: "invalid_json" };
  }
}

/** A `replaceAll` count of 0 left the document as it was: not an edit to keep. */
export function rhwpMutationChanged(result: RhwpMutationResult): boolean {
  if (!result.ok) return false;
  return result.count === undefined || result.count > 0;
}
