// Adapted from packages/contracts/src/primitives.ts at source SHA
// 393795261322b916e588043cf94feca999175843. Keep equivalent to that
// RFC 4122/9562 contract rather than a guessed local schema.
import { z } from "zod";

/** API id — RFC 4122/9562 (`z.uuid()`). */
export const uuid = z.uuid();
/** WHY: 8-4-4-4-12 hex 정본. 패키지 밖 재선언은 check-id-regex 예산이다. */
export const UUID_SOURCE =
	"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}";
export const UUID_RE = new RegExp(`^${UUID_SOURCE}$`, "i");
