// SIGINT fixture for common.test.ts: a body command is interrupted, a later
// body command refuses to start, and a cleanup command still runs.
import { writeSync } from "node:fs";
import { cleanupScope, command, trapInterrupts } from "./common.ts";

trapInterrupts();
const outcome = (error: unknown) => (error instanceof Error ? error.name : "NonError");
const result: Record<string, unknown> = {};
writeSync(1, "started\n");
try {
  await command(["sleep", "30"]);
  result.body = "completed";
} catch (error) {
  result.body = outcome(error);
}
try {
  await command(["true"]);
  result.laterBody = "completed";
} catch (error) {
  result.laterBody = outcome(error);
}
result.cleanup = await cleanupScope(async () => (await command(["true"])).returncode);
writeSync(1, JSON.stringify(result) + "\n");
