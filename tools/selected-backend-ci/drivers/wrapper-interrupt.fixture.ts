// SIGINT fixture for common.test.ts: a fixture wrapper that owns a resource and
// cleans it up from its EXIT trap, run as an owned command that waits for it.
import { writeFileSync, writeSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { command, trapInterrupts } from "./common.ts";

const directory = process.argv[2] as string;
const wrapper = join(directory, "wrapper.sh");
// The same trap shape as scripts/start-test-{postgres,meili}.sh.
writeFileSync(
  wrapper,
  `trap 'touch "${directory}/cleaned"' EXIT\ntrap 'exit 130' INT\ntrap 'exit 143' TERM\necho ready >&2\nsleep 30\n`,
);
trapInterrupts();
const result: Record<string, unknown> = {};
const running = command(["bash", wrapper], {
  log: join(directory, "wrapper.log"),
  required: false,
  waitOnInterrupt: true,
});
writeSync(1, "started\n");
try {
  result.exit = (await running).returncode;
} catch (error) {
  result.error = error instanceof Error ? error.name : "NonError";
}
writeSync(1, JSON.stringify(result) + "\n");
