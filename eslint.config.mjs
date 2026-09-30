import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import prettier from "eslint-config-prettier/flat";
import vue from "eslint-plugin-vue";
import globals from "globals";
import tseslint from "typescript-eslint";
import vueParser from "vue-eslint-parser";

const code = ["**/*.{js,mjs,cjs,ts,tsx,vue}"];
const typed = ["**/*.{ts,tsx,vue}"];
const nodeRuntimeGlobals = [
  "Bun",
  "process",
  "Buffer",
  "require",
  "__dirname",
  "__filename",
  "module",
  "exports",
  "setImmediate",
  "clearImmediate",
];

export default defineConfig(
  globalIgnores([
    "**/node_modules/**",
    "**/.codegraph/**",
    "vendor/**",
    "compat/fixtures/**",
    "apps/web/src/generated/api.ts",
    "src/http/api_docs_assets/swagger-ui-bundle.js",
  ]),
  {
    files: code,
    extends: [js.configs.recommended],
    linterOptions: { reportUnusedDisableDirectives: "error" },
  },
  {
    files: typed,
    extends: [tseslint.configs.strictTypeChecked],
    languageOptions: {
      parserOptions: {
        project: [
          "apps/web/tsconfig.eslint.json",
          "packages/editor/tsconfig.eslint.json",
          "packages/i18n/tsconfig.eslint.json",
        ],
        tsconfigRootDir: import.meta.dirname,
        extraFileExtensions: [".vue"],
      },
    },
    rules: { "@typescript-eslint/no-floating-promises": ["error", { ignoreVoid: false }] },
  },
  {
    files: ["**/*.vue"],
    extends: [vue.configs["flat/recommended"]],
    languageOptions: { parser: vueParser, parserOptions: { parser: tseslint.parser } },
  },
  {
    files: [
      "apps/web/src/**/*.{ts,tsx,vue}",
      "packages/editor/src/**/*.{ts,tsx,vue}",
      "packages/i18n/src/**/*.ts",
    ],
    ignores: [
      "**/*.test.ts",
      "apps/web/src/build/**",
      "apps/web/src/features/attachments/{hwp,pptx,xlsx}-worker.ts",
    ],
    languageOptions: { globals: globals.browser },
  },
  {
    // Runtime restrictions apply to browser libraries and Workers as well as UI.
    // Keep this separate from browser globals: Workers must not receive window.
    files: [
      "apps/web/src/**/*.{ts,tsx,vue}",
      "apps/web/public/sw.js",
      "packages/editor/src/**/*.{ts,tsx,vue}",
      "packages/i18n/src/**/*.ts",
    ],
    ignores: ["**/*.test.ts", "apps/web/src/build/**"],
    rules: {
      "no-restricted-globals": [
        "error",
        {
          globals: nodeRuntimeGlobals,
          checkGlobalObject: true,
        },
      ],
    },
  },
  {
    files: ["apps/web/src/features/attachments/{hwp,pptx,xlsx}-worker.ts", "apps/web/public/sw.js"],
    languageOptions: { globals: globals.worker },
    rules: {
      // Window APIs are unavailable in Workers; this also rejects window.process.
      "no-restricted-globals": [
        "error",
        { globals: [...nodeRuntimeGlobals, "window"], checkGlobalObject: true },
      ],
    },
  },
  {
    files: [
      "eslint.config.mjs",
      "scripts/**/*.{js,mjs,cjs}",
      "apps/web/*.ts",
      "apps/web/src/build/**/*.ts",
      "apps/web/vite-plugin-*.ts",
      "apps/web/e2e*/**/*.ts",
      "apps/web/test/**/*.ts",
      "**/*.test.ts",
      "packages/editor/test/**/*.ts",
    ],
    languageOptions: { globals: globals.node },
  },
  {
    files: ["**/*.test.ts", "packages/editor/test/**/*.ts", "apps/web/test/**/*.ts"],
    languageOptions: { globals: { Bun: "readonly" } },
  },
  prettier,
);
