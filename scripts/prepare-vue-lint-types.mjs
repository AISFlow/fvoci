import { spawnSync } from "node:child_process";
import { rmSync } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";
import "./verify-web-tools.mjs";

const require = createRequire(import.meta.url);
const vueTsc = require.resolve("vue-tsc/bin/vue-tsc.js");
if (require("vue-tsc/package.json").version !== "3.3.12") {
  throw new Error("Vue lint declarations require pinned vue-tsc 3.3.12");
}

export function prepareVueLintTypes(project, output) {
  // Clear this disposable type layer first: a failed source check cannot reuse it.
  rmSync(output, { recursive: true, force: true });
  const projects = Array.isArray(project) ? project : [project];
  try {
    if (projects.length === 0) throw new Error("Vue lint declarations require a project");
    for (const sourceProject of projects) {
      const result = spawnSync(
        process.execPath,
        [
          "--bun",
          vueTsc,
          "--project",
          sourceProject,
          "--declaration",
          "--emitDeclarationOnly",
          "--noEmit",
          "false",
          "--noEmitOnError",
          "true",
          "--outDir",
          output,
        ],
        { stdio: "inherit" },
      );
      if (result.error) throw result.error;
      if (result.status !== 0) {
        throw new Error(
          `Vue lint declaration generation failed: project=${sourceProject}, exit=${result.status}, signal=${result.signal}`,
        );
      }
    }
  } catch (error) {
    // A later project failure must also discard earlier successful emissions.
    rmSync(output, { recursive: true, force: true });
    throw error;
  }
}

if (import.meta.main) {
  prepareVueLintTypes(
    [
      fileURLToPath(
        new URL("../packages/editor/tsconfig.eslint-declarations.json", import.meta.url),
      ),
      fileURLToPath(new URL("../apps/web/tsconfig.eslint-declarations.json", import.meta.url)),
    ],
    fileURLToPath(new URL("../node_modules/.cache/fvoci-vue-lint/types", import.meta.url)),
  );
}
