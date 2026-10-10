import { repositoryRoot, runChecked, withDefaults } from "./web-lint-process.ts";

export const formatDefaults = [
  "apps/web",
  "packages/editor",
  "packages/i18n",
  "scripts/document-convert",
  "scripts/generate-emoji-shortcodes.mjs",
  "scripts/install-smoke-collab.mjs",
  "scripts/verify-web-tools.mjs",
  "scripts/WEB_LINT.md",
  "scripts/run-selected-backend-e2e.ts",
  "tools/selected-backend-ci/**/*.ts",
  "eslint.config.mjs",
  "package.json",
  ".prettierrc.json",
];

export function formatCommands(argv: readonly string[]): string[][] {
  return [
    ["bun", "--bun", "scripts/verify-web-tools.mjs"],
    [
      "bun",
      "--bun",
      "node_modules/prettier/bin/prettier.cjs",
      "--check",
      "--ignore-path",
      ".prettierignore",
      ...withDefaults(argv, formatDefaults),
    ],
  ];
}

if (import.meta.main) process.exit(runChecked(formatCommands(process.argv.slice(2)), repositoryRoot()));
