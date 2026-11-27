import js from '@eslint/js';
import prettier from 'eslint-config-prettier';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import ts from 'typescript-eslint';

import svelteConfig from './svelte.config.js';
import { noBrowserDialogs } from './tools/lint/dialogs.ts';
import layers from './tools/lint/layers.ts';

/** Code written by the shadcn-svelte CLI. See src/lib/ui/README.md. */
const GENERATED_UI = ['src/lib/ui/**', 'src/lib/hooks/**', 'src/lib/utils.ts'];

export default ts.config(
  {
    ignores: [
      'dist/',
      'storybook-static/',
      'test-results/',
      'playwright-report/',
      // What `wrangler dev` keeps while it serves Storybook (wrangler.jsonc).
      '.wrangler/',
      'src/lib/i18n/paraglide/',
      'src/styles/fonts/',
      // Made from the server's contract (`pnpm run api:types`).
      'src/lib/api/contract.ts',
    ],
  },
  js.configs.recommended,
  ...ts.configs.strictTypeChecked,
  ...ts.configs.stylisticTypeChecked,
  ...svelte.configs.recommended,
  prettier,
  ...svelte.configs.prettier,
  {
    languageOptions: {
      globals: { ...globals.browser, ...globals.node },
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
        extraFileExtensions: ['.svelte'],
      },
    },
    rules: {
      eqeqeq: ['error', 'always'],
      'no-console': ['error', { allow: ['warn', 'error'] }],
      '@typescript-eslint/consistent-type-imports': 'error',
      '@typescript-eslint/no-unused-vars': [
        'error',
        { argsIgnorePattern: '^_', varsIgnorePattern: '^_' },
      ],
      // `a || b` on text and numbers means "or, when empty": an empty title, a
      // zero timestamp. `??` would let those through.
      '@typescript-eslint/prefer-nullish-coalescing': [
        'error',
        { ignorePrimitives: { string: true, number: true, boolean: true } },
      ],
      // A `type` is needed where an object has to fit an index signature
      // (the `fields` of a block), so the choice is left to the author.
      '@typescript-eslint/consistent-type-definitions': 'off',
      // Numbers and booleans in template literals are intended.
      '@typescript-eslint/restrict-template-expressions': [
        'error',
        { allowNumber: true, allowBoolean: true },
      ],
    },
  },
  {
    // See src/lib/features/README.md.
    files: ['src/lib/**'],
    plugins: { fvoci: layers },
    rules: { 'fvoci/feature-boundaries': 'error', 'fvoci/layer-order': 'error' },
  },
  {
    // Screens reach data through lib/state. See src/lib/README.md.
    files: ['src/**'],
    ignores: [
      'src/lib/state/**',
      'src/lib/api/**',
      // Probes live index.html via fetch() on dynamic chunk failure to detect new deployments.
      'src/lib/chunk-reload.svelte.ts',
      'src/app/backend.ts',
    ],
    plugins: { fvoci: layers },
    rules: { 'fvoci/no-direct-server': 'error' },
  },
  noBrowserDialogs,
  {
    // The logic takes the reader's time zone as an argument. See src/lib/logic/dates.ts.
    files: ['src/lib/logic/**'],
    rules: { 'fvoci/no-device-time': 'error' },
  },
  {
    files: ['**/*.svelte', '**/*.svelte.ts', '**/*.svelte.js'],
    languageOptions: {
      parserOptions: { parser: ts.parser, svelteConfig },
    },
  },
  {
    // Command-line tools report on the console.
    files: ['tools/**'],
    rules: { 'no-console': 'off' },
  },
  {
    // A test stands in for an asynchronous function with one that has nothing to wait for.
    files: ['tests/**', 'e2e/**', 'e2e-live/**'],
    rules: { '@typescript-eslint/require-await': 'off' },
  },
  {
    // Imported code: only what would be a defect is checked, not its style.
    files: GENERATED_UI,
    linterOptions: { reportUnusedDisableDirectives: 'off' },
    rules: {
      '@typescript-eslint/consistent-type-definitions': 'off',
      '@typescript-eslint/no-empty-function': 'off',
      ...ts.configs.disableTypeChecked.rules,
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-unused-vars': 'off',
      '@typescript-eslint/no-unnecessary-type-parameters': 'off',
      '@typescript-eslint/no-unnecessary-condition': 'off',
      '@typescript-eslint/no-non-null-assertion': 'off',
      '@typescript-eslint/no-empty-object-type': 'off',
      '@typescript-eslint/no-invalid-void-type': 'off',
      '@typescript-eslint/no-dynamic-delete': 'off',
      '@typescript-eslint/consistent-type-imports': 'off',
      '@typescript-eslint/no-unused-expressions': 'off',
      'svelte/no-navigation-without-resolve': 'off',
      'svelte/require-each-key': 'off',
      'svelte/no-useless-children-snippet': 'off',
      'svelte/prefer-svelte-reactivity': 'off',
      'svelte/prefer-writable-derived': 'off',
      'no-useless-assignment': 'off',
    },
  },
  {
    files: ['*.config.js', '*.config.ts'],
    rules: { ...ts.configs.disableTypeChecked.rules },
  },
);
