// SIGINT fixture for sqlite.test.ts: the real sqlite main with the real
// interrupt trap and command. Every docker call is scripted except `docker
// start`, which runs a SIGINT-resistant child that interrupts this driver.
import { appendFileSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import process from "node:process";
import { sha } from "../io.ts";
import type { Current } from "./binding.ts";
import { command } from "./common.ts";
import { main, sqliteSeam } from "./sqlite.ts";

const directory = process.argv[2] as string;
mkdirSync(join(directory, "runtime"));
const binaries: Record<string, unknown> = {};
for (const name of ["fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "collab-engine"]) {
  const path = join(directory, name);
  writeFileSync(path, name);
  binaries[path] = { sha256: sha(path), target: { name } };
}
const driver = join(directory, "driver.ts");
writeFileSync(driver, "synthetic driver");
const before = {
  head: "a".repeat(40),
  tree: "b".repeat(40),
  status: "",
  tracked: {},
  external: {},
  untracked: {},
};
const current = {
  manifest: { source: before.head, tree: before.tree, compiledSource: before.head },
  manifestPath: driver,
  grant: { runId: "1", runAttempt: "1" },
  run: join(directory, "runtime", "root-current-sqlite-0123456789ab"),
  before,
  sourceBefore: structuredClone(before),
  build: { binaries },
  assets: { source: before.head, dist_files: {} },
  abi: { host_runtime_files: {} },
  flow: "on",
} as unknown as Current;
const blocker = join(import.meta.dir, "../interrupt-child.fixture.ts");
const code = await main(
  {
    ...sqliteSeam,
    loadCurrent: () => Promise.resolve(current),
    inputCheck: () => Promise.resolve(structuredClone(before)),
    treeHashes: () => ({}),
    ownedRows: () => Promise.resolve([]),
    command(args, options) {
      appendFileSync(join(directory, "calls.jsonl"), JSON.stringify(args) + "\n");
      if (args[1] === "start")
        return command([process.execPath, blocker, "interrupt-parent"], {
          ...options,
          log: join(directory, "blocker.log"),
        });
      return Promise.resolve(
        args[1] === "inspect"
          ? { returncode: 1, stdout: "", stderr: "No such container: owned" }
          : { returncode: 0, stdout: "", stderr: "" },
      );
    },
  },
  driver,
);
process.exit(code);
