#!/usr/bin/env bun
// One hosted current-primary UI consumer; credentials never enter browsers.
// Local preparation (--record-before, --freeze) is credential-free. Runtime
// admission belongs to the guard and ROOT's exact current dataset binding.
// No reset/restore cleanup exists.
import process from "node:process";
import { stdio, UiError, type Output } from "./ui-common.ts";
import { lease } from "./ui-container.ts";
import { actor } from "./ui-flow.ts";
import { withProcesses } from "./ui-processes.ts";
import { freeze, recordBefore } from "./ui-record.ts";

export { UiError } from "./ui-common.ts";
export { consume } from "./ui-flow.ts";

export async function main(argv: string[], output: Output = stdio): Promise<number> {
  try {
    const mode = argv.join("\0");
    if (mode === "--record-before") recordBefore(lease.load);
    else if (mode === "--freeze") freeze(lease.load);
    else if (mode === "--actor") await withProcesses((scope) => actor(scope));
    else throw new UiError("UI_EXPLICIT_MODE_REQUIRED");
    return 0;
  } catch (error) {
    output.err(error instanceof UiError ? error.message : "UI_CONSUMER_FAILED");
    return 78;
  }
}

if (import.meta.main) process.exitCode = await main(process.argv.slice(2));
