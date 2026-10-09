import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { repositoryRoot, type DiscoveryOptions } from "./groups";

export const fixtureConfig = resolve(import.meta.dir, "discovery.config.ts");
export const fixtureTest = 'import { test } from "@playwright/test";\ntest("fixture", () => {});\n';
export function groupFixture(files: Readonly<Record<string, string>>): {
  directory: string;
  options: DiscoveryOptions;
  cleanup: () => void;
} {
  const parent = join(repositoryRoot, "target/harness-web-e2e-groups-ts-1/fixtures");
  mkdirSync(parent, { recursive: true });
  const directory = mkdtempSync(join(parent, "groups-"));
  for (const [file, content] of Object.entries(files)) {
    const path = join(directory, file);
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, content);
  }
  return {
    directory,
    options: { config: fixtureConfig, env: { FVOCI_GROUP_FIXTURE_DIR: directory } },
    cleanup: () => {
      rmSync(directory, { recursive: true, force: true });
    },
  };
}
export function pairFiles(extra: readonly string[] = []): Record<string, string> {
  return Object.fromEntries(
    ["workspace-flow.spec.ts", "workspace-wiki-flow.spec.ts", ...extra].map((name) => [
      name,
      fixtureTest,
    ]),
  );
}
