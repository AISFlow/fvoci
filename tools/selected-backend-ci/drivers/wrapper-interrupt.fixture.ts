// SIGINT fixture for common.test.ts: a fixture wrapper that owns a resource and
// cleans it up from its EXIT trap, run as an owned command that waits for it.
import { writeFileSync, writeSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { command, trapInterrupts } from "./common.ts";

type Shape = "trap" | "ignore" | "escape" | "foreign";
const [directory, shape, grace] = process.argv.slice(2) as [string, Shape, string];
const wrapper = join(directory, "wrapper.sh");
// "trap" is the trap shape of scripts/start-test-{postgres,meili}.sh; the
// others ignore SIGINT, which their foreground command inherits. Either way
// bash runs no trap while the foreground command (recorded in foreground.pid)
// is alive. "escape" first starts workers that wait until the wrapper is
// stopped, then start a sleeper and exit, so each sleeper is orphaned while
// the tree is being walked; a worker polls with a timed read of a FIFO no one
// writes, a builtin wait. "foreign" leaves a root-owned orphan, recorded in
// foreign.pid, that the tree walk cannot signal; it ignores the SIGHUP of a
// sudo pty.
const tree = {
  trap: "",
  ignore: "",
  escape:
    'mkfifo "$1/idle"; exec 7<>"$1/idle"\n' +
    "for worker in $(seq 16); do (\n" +
    "  while read -r stat < /proc/$$/stat; do\n" +
    "    stat=${stat##*) }\n" +
    "    case ${stat%% *} in T | t | Z) sleep 300 & break ;; esac\n" +
    "    read -r -t 0.001 -u 7 || :\n" +
    "  done\n" +
    ") & done\n",
  foreign: 'sudo -n sh -c \'trap "" HUP; sleep 300 & echo $! > "$1/foreign.pid"\' sh "$1"\n',
}[shape];
writeFileSync(
  wrapper,
  `trap 'touch "$1/cleaned"' EXIT\ntrap '${shape === "trap" ? "exit 130" : ""}' INT\ntrap 'exit 143' TERM\n` +
    tree +
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
  result.message = error instanceof Error ? error.message : String(error);
}
writeSync(1, JSON.stringify(result) + "\n");
