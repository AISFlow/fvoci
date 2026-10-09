import { test } from "bun:test";
import { runToolchainCase, toolchainCases, type ToolchainCaseName } from "./toolchain";

for (const name of Object.keys(toolchainCases) as ToolchainCaseName[]) {
  test(name, async () => {
    await runToolchainCase(name);
  });
}
