import { repositoryRoot, runChecked, withDefaults } from "./web-lint-process.ts";

export const lintDefaults = [
  "apps/web/**/*.{js,mjs,cjs,ts,tsx,vue}",
  "packages/editor/**/*.{js,mjs,cjs,ts,tsx,vue}",
  "packages/i18n/**/*.{js,mjs,cjs,ts,tsx,vue}",
  "scripts/**/*.mjs",
  "scripts/run-selected-backend-e2e.ts",
  "tools/selected-backend-ci/**/*.ts",
  "eslint.config.mjs",
];

export function lintCommands(argv: readonly string[]): string[][] {
  return [
    ["bun", "--bun", "scripts/verify-web-tools.mjs"],
    ["bun", "--bun", "scripts/prepare-vue-lint-types.mjs"],
    [
      "bun",
      "--bun",
      "node_modules/eslint/bin/eslint.js",
      "--max-warnings=0",
      ...withDefaults(argv, lintDefaults),
    ],
  ];
}

if (import.meta.main) process.exit(runChecked(lintCommands(process.argv.slice(2)), repositoryRoot()));
