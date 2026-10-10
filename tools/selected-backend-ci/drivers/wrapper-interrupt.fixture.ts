// SIGINT fixture for common.test.ts: a fixture wrapper that owns a resource and
// cleans it up from its EXIT trap, run as an owned command that waits for it.
import { writeFileSync, writeSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { command, trapInterrupts } from "./common.ts";

const [directory, shape, grace] = process.argv.slice(2) as [string, "trap" | "ignore", string];
const wrapper = join(directory, "wrapper.sh");
// "trap" is the trap shape of scripts/start-test-{postgres,meili}.sh; "ignore"
// ignores SIGINT, which its foreground command inherits. Either way bash runs
// no trap while the foreground command (recorded in foreground.pid) is alive.
writeFileSync(
  wrapper,
  `trap 'touch "$1/cleaned"' EXIT\ntrap '${shape === "trap" ? "exit 130" : ""}' INT\ntrap 'exit 143' TERM\n` +
    `sh -c 'echo $$ > "$1/foreground.pid"; echo ready >&2; exec sleep 30' sh "$1"\n`,
);
trapInterrupts();
const result: Record<string, unknown> = {};
const running = command(["bash", wrapper, directory], {
  log: join(directory, "wrapper.log"),
  required: false,
  waitOnInterrupt: true,
  interruptGrace: Number(grace),
});
writeSync(1, "started\n");
try {
  result.exit = (await running).returncode;
} catch (error) {
  result.error = error instanceof Error ? error.name : "NonError";
}
writeSync(1, JSON.stringify(result) + "\n");
