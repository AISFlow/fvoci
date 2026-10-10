#!/usr/bin/env bun
// CI web-checks entry: verify the production shard plan, then the harness behavior tests.

import { join } from "node:path";
import { commandStatus } from "./proc.ts";

const root = join(import.meta.dir, "../..");
const verify = await commandStatus(
  [process.execPath, join(root, "tools/web-e2e/groups.ts"), "verify", "--shards", "8"],
  {
    cwd: root,
    stdout: "inherit",
    stderr: "inherit",
  },
);
if (verify !== 0) process.exit(verify);
const tests = await commandStatus([process.execPath, "test", "./tools/web-e2e"], {
  cwd: root,
  stdout: "inherit",
  stderr: "inherit",
});
if (tests !== 0) process.exit(tests);
console.log("test-web-e2e-groups: ok");
