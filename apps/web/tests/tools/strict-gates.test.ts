/**
 * Unit tests for the strict web static gates:
 * - browser-native modal dialogs (`alert`, `confirm`, `prompt`) in `dialogs.ts`
 *   and wired through `eslint.config.js`
 * - static viewport-height anti-patterns (`100vh`, `h-screen`, `min-h-screen`,
 *   `max-h-screen`) in `viewport.ts` and `check-viewport.ts`
 * - `@sveltejs/mcp` `svelteAutofixer` verification (in-process and multi-worker)
 *   in `autofixer.ts`, `autofixer-worker.ts`, and `check-autofixer.ts`
 */
import { spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { ESLint, Linter } from 'eslint';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import ts from 'typescript-eslint';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import eslintConfig from '../../eslint.config.js';
import svelteConfig from '../../svelte.config.js';
import { packageDirectory } from '../../tools/licenses/packages.ts';
import {
  checkAutofixer,
  checkSvelteCode,
  readSvelteVersion,
  svelteFiles,
} from '../../tools/lint/autofixer.ts';
import {
  checkDialogs,
  findDialogDisableViolations,
  noBrowserDialogs,
} from '../../tools/lint/dialogs.ts';
import { checkViewport, findViewportViolations, viewportFiles } from '../../tools/lint/viewport.ts';

const ROOT = path.resolve(import.meta.dirname, '../..');
const SVELTE_VERSION = readSvelteVersion(ROOT);
const SLOW = 120_000;

const dialogLintConfig: Linter.Config[] = [
  ...svelte.configs.base,
  {
    files: ['**/*.ts'],
    ignores: ['**/*.svelte.ts'],
    languageOptions: { globals: { ...globals.browser }, parser: ts.parser },
  },
  {
    files: ['**/*.svelte', '**/*.svelte.ts'],
    languageOptions: {
      globals: { ...globals.browser },
      parserOptions: { parser: ts.parser, svelteConfig },
    },
  },
  noBrowserDialogs,
];

function lintDialogs(file: string, code: string): string[] {
  const linter = new Linter({ cwd: ROOT });
  return linter
    .verify(code, dialogLintConfig, { filename: path.join(ROOT, file) })
    .map((entry) => `${entry.ruleId ?? 'parse'}:${entry.line}: ${entry.message}`);
}

describe('browser modal dialog gate (noBrowserDialogs + eslint.config.js)', () => {
  it('forbids bare alert, confirm, and prompt calls in TypeScript and Svelte', () => {
    expect(lintDialogs('src/lib/features/card/Card.ts', "alert('saved');")).toEqual([
      expect.stringContaining('no-restricted-globals:1:'),
    ]);
    expect(lintDialogs('src/lib/features/card/Card.ts', "if (confirm('delete?')) {}")).toEqual([
      expect.stringContaining('Browser `confirm()` is forbidden'),
    ]);
    expect(lintDialogs('src/lib/features/card/Card.ts', "const v = prompt('title');")).toEqual([
      expect.stringContaining('Browser `prompt()` is forbidden'),
    ]);

    const svelteSample = [
      '<script lang="ts">',
      "  function remove(): void { alert('gone'); }",
      '</script>',
      "<button onclick={() => confirm('really?')}>{prompt('name')}</button>",
    ].join('\n');
    expect(lintDialogs('src/lib/features/card/Card.svelte', svelteSample)).toHaveLength(3);
  });

  it('forbids global receivers, computed properties, destructuring, and eslint-disable bypasses', () => {
    const code = [
      "window.alert('one');",
      "window.confirm('two');",
      "window.prompt('three');",
      "window?.confirm('four');",
      "window['confirm']('five');",
      'const { prompt: ask } = window;',
      "globalThis.alert('seven');",
      "self.prompt('eight');",
      "top?.confirm('nine');",
      "parent.alert('ten');",
      "frames.prompt('eleven');",
    ].join('\n');
    const found = lintDialogs('src/app/App.ts', code);
    expect(found).toHaveLength(11);
    for (const item of found) {
      expect(item).toContain('no-restricted-properties:');
    }

    const attemptedBypass = [
      '// eslint-disable-next-line no-restricted-globals',
      "confirm('still caught');",
    ].join('\n');
    expect(lintDialogs('src/app/App.ts', attemptedBypass)).toEqual(
      expect.arrayContaining([expect.stringContaining('fvoci-dialogs/no-dialog-disable:1:')]),
    );

    // Inline /* eslint ... */ rule reconfiguration comments are also blocked.
    const inlineConfigBypass = [
      '/* eslint no-restricted-globals: "off", no-restricted-properties: 0 */',
      "confirm('caught by inline config check');",
    ].join('\n');
    expect(lintDialogs('src/app/App.ts', inlineConfigBypass)).toEqual(
      expect.arrayContaining([expect.stringContaining('fvoci-dialogs/no-dialog-disable:1:')]),
    );

    // Out-of-band pure text check also blocks silencing `fvoci-dialogs/no-dialog-disable`
    // itself, inline `/* eslint ... */` overrides, multiline disables, or bare `eslint-disable`.
    const selfDisableBypass = [
      '// eslint-disable-line fvoci-dialogs/no-dialog-disable, no-restricted-globals',
      "confirm('blocked outside eslint too');",
      '/* eslint-disable */',
      '<!-- eslint-disable -->',
      '/* eslint fvoci-dialogs/no-dialog-disable: "off", no-restricted-globals: "off" */',
      '/* eslint-disable no-console,\n   no-restricted-properties */',
    ].join('\n');
    expect(findDialogDisableViolations('src/app/App.svelte', selfDisableBypass)).toEqual([
      'src/app/App.svelte:1: do not silence `fvoci-dialogs/no-dialog-disable` with `eslint-disable`; replace native browser dialogs with app UI',
      'src/app/App.svelte:1: do not silence `no-restricted-globals` with `eslint-disable`; replace native browser dialogs with app UI',
      'src/app/App.svelte:3: bare `eslint-disable` without rule names is forbidden; name the exact non-protected rule being disabled',
      'src/app/App.svelte:4: bare `eslint-disable` without rule names is forbidden; name the exact non-protected rule being disabled',
      'src/app/App.svelte:5: do not reconfigure `fvoci-dialogs/no-dialog-disable` with inline `eslint` config; replace native browser dialogs with app UI',
      'src/app/App.svelte:5: do not reconfigure `no-restricted-globals` with inline `eslint` config; replace native browser dialogs with app UI',
      'src/app/App.svelte:6: do not silence `no-restricted-properties` with `eslint-disable`; replace native browser dialogs with app UI',
    ]);
    expect(checkDialogs(ROOT).problems).toEqual([]);
  });

  it('allows local functions, props, parameters, and methods named confirm, alert, or prompt', () => {
    const confirmApp = [
      '<script lang="ts">',
      '  interface EmailChange { confirm(token: string): Promise<void>; }',
      '  let { change, prompt }: { change: EmailChange; prompt: (q: string) => void } = $props();',
      '  async function confirm(token: string): Promise<void> {',
      '    await change.confirm(token);',
      '  }',
      '  function alert(label: string): string {',
      '    return label;',
      '  }',
      '</script>',
      '<button onclick={() => { void confirm("tok"); prompt(alert("ok")); }}>go</button>',
    ].join('\n');
    expect(lintDialogs('src/app/ConfirmEmailApp.svelte', confirmApp)).toEqual([]);
    expect(lintDialogs('tools/script.ts', "alert('outside src is not in scope');")).toEqual([]);
  });

  it(
    'is wired into production eslint.config.js across both app code and GENERATED_UI',
    async () => {
      const eslint = new ESLint({
        cwd: ROOT,
        overrideConfigFile: true,
        overrideConfig: [
          ...(eslintConfig as Linter.Config[]),
          {
            languageOptions: { parserOptions: { projectService: false } },
            rules: { ...ts.configs.disableTypeChecked.rules },
          },
        ],
      });
      const appResult = await eslint.lintText(
        '<script lang="ts">\n  window.confirm(\'nope\');\n</script>\n<p>x</p>\n',
        { filePath: path.join(ROOT, 'src/app/ConfirmEmailApp.svelte') },
      );
      expect(appResult[0]?.messages.map((m) => m.ruleId)).toContain('no-restricted-properties');

      const uiResult = await eslint.lintText(
        '<script lang="ts">\n  alert(\'nope\');\n</script>\n<button>ok</button>\n',
        { filePath: path.join(ROOT, 'src/lib/ui/button/button.svelte') },
      );
      expect(uiResult[0]?.messages.map((m) => m.ruleId)).toContain('no-restricted-globals');
    },
    SLOW,
  );
});

describe('viewport height gate (viewport.ts)', () => {
  it('flags 100vh/100lvh, h-screen/h-lvh variants, and static vh/lvh inside CSS math functions', () => {
    expect(
      findViewportViolations(
        'src/styles/app.css',
        '.shell { height: 100vh; min-height: 100.0vh; max-height: 100lvh; }\n',
      ),
    ).toEqual([
      'src/styles/app.css:1: `100vh` uses the static viewport height; use `100dvh` (or `h-dvh`) instead',
      'src/styles/app.css:1: `100.0vh` uses the static viewport height; use `100dvh` (or `h-dvh`) instead',
      'src/styles/app.css:1: `100lvh` uses the static viewport height; use `100dvh` (or `h-dvh`) instead',
    ]);
    expect(
      findViewportViolations(
        'src/app/Shell.svelte',
        '<div class="flex h-screen md:min-h-screen !max-h-screen h-lvh min-h-lvh max-h-lvh h-[100VH] h-[calc(100vh_-_2rem)] min-h-[min(40rem,_100vh)] max-h-[calc(80vh_-_1rem)] h-[clamp(10rem,_90lvh,_40rem)]"></div>\n',
      ),
    ).toEqual([
      'src/app/Shell.svelte:1: `h-screen` maps to `100vh`; use `h-dvh` (`100dvh`) instead',
      'src/app/Shell.svelte:1: `min-h-screen` maps to `100vh`; use `min-h-dvh` (`100dvh`) instead',
      'src/app/Shell.svelte:1: `max-h-screen` maps to `100vh`; use `max-h-dvh` (`100dvh`) instead',
      'src/app/Shell.svelte:1: `h-lvh` maps to `100lvh`; use `h-dvh` (`100dvh`) instead',
      'src/app/Shell.svelte:1: `min-h-lvh` maps to `100lvh`; use `min-h-dvh` (`100dvh`) instead',
      'src/app/Shell.svelte:1: `max-h-lvh` maps to `100lvh`; use `max-h-dvh` (`100dvh`) instead',
      'src/app/Shell.svelte:1: `100VH` uses the static viewport height; use `100dvh` (or `h-dvh`) instead',
      'src/app/Shell.svelte:1: `100vh` uses the static viewport height; use `100dvh` (or `h-dvh`) instead',
      'src/app/Shell.svelte:1: `100vh` uses the static viewport height; use `100dvh` (or `h-dvh`) instead',
      'src/app/Shell.svelte:1: `80vh` uses a static viewport height inside a CSS math expression; use `80dvh` instead',
      'src/app/Shell.svelte:1: `90lvh` uses a static viewport height inside a CSS math expression; use `90dvh` instead',
    ]);
  });

  it('allows dynamic viewport units and unrelated -screen names', () => {
    const clean = [
      '<main class="grid min-h-dvh h-dvh max-h-dvh w-screen touch-screen splash-screen search-screen">',
      '  <div style="height: 100dvh; min-height: 100svh; max-height: min(18rem, 50dvh); width: 2100vh; top: 0.100vh"></div>',
      '</main>',
    ].join('\n');
    expect(findViewportViolations('src/app/Clean.svelte', clean)).toEqual([]);
  });

  describe('walking a directory tree', () => {
    let root = '';
    beforeEach(() => {
      root = mkdtempSync(path.join(tmpdir(), 'fvoci-viewport-'));
    });
    afterEach(() => {
      rmSync(root, { recursive: true, force: true });
    });
    const write = (name: string, text: string): void => {
      mkdirSync(path.dirname(path.join(root, name)), { recursive: true });
      writeFileSync(path.join(root, name), text);
    };

    it('checks src sources and HTML entry points, skips generated paths, and refuses symbolic links', () => {
      write('index.html', '<!doctype html><html class="min-h-dvh"></html>\n');
      write('static/viewer-frame.html', '<body style="height: 100vh"></body>\n');
      write('src/app/Page.svelte', '<main class="min-h-dvh"></main>\n');
      write('src/app/Bad.svelte', '<div class="h-screen"></div>\n');
      write('src/styles/bad.css', '.pane { max-height: 100vh; }\n');
      write('src/lib/i18n/paraglide/messages.js', 'export const x = "100vh h-screen";\n');
      write('src/lib/api/contract.ts', 'export type X = "h-screen";\n');
      write('src/styles/fonts/fonts.css', '.font { height: 100vh; }\n');

      expect(viewportFiles(root)).toEqual([
        'index.html',
        'src/app/Bad.svelte',
        'src/app/Page.svelte',
        'src/styles/bad.css',
        'static/viewer-frame.html',
      ]);
      expect(checkViewport(root).problems).toEqual([
        'src/app/Bad.svelte:1: `h-screen` maps to `100vh`; use `h-dvh` (`100dvh`) instead',
        'src/styles/bad.css:1: `100vh` uses the static viewport height; use `100dvh` (or `h-dvh`) instead',
        'static/viewer-frame.html:1: `100vh` uses the static viewport height; use `100dvh` (or `h-dvh`) instead',
      ]);

      symlinkSync(path.join(root, 'src/app/Page.svelte'), path.join(root, 'src/app/Link.svelte'));
      expect(() => viewportFiles(root)).toThrow('src/app/Link.svelte is a symbolic link');
    });
  });

  it(
    'check-viewport.ts refuses unexpected CLI arguments',
    () => {
      const result = spawnSync(process.execPath, ['tools/lint/check-viewport.ts', '--fix'], {
        cwd: ROOT,
        encoding: 'utf8',
      });
      expect(result.status).toBe(2);
      expect(result.stderr).toContain('no arguments');
    },
    SLOW,
  );
});

describe('svelte-autofixer gate (autofixer.ts)', () => {
  it(
    'catches rune misuse and compiler issues, even behind eslint-disable comments',
    async () => {
      const storePrefixOnRune = [
        '<!-- eslint-disable -->',
        '<script lang="ts">',
        '  /* eslint-disable */',
        '  let count = $state(0);',
        '</script>',
        '<p>{$count}</p>',
      ].join('\n');
      const runeIssues = await checkSvelteCode(
        'src/lib/components/BadCounter.svelte',
        storePrefixOnRune,
        SVELTE_VERSION,
      );
      expect(runeIssues).toEqual(
        expect.arrayContaining([
          expect.stringContaining('reading the stateful variable "$count" with a "$" prefix'),
        ]),
      );

      const brokenSyntax = '<script lang="ts">\n  $state count = 0;\n</script>\n<p>{count}</p>\n';
      const syntaxIssues = await checkSvelteCode(
        'src/lib/components/Broken.svelte',
        brokenSyntax,
        SVELTE_VERSION,
      );
      expect(syntaxIssues.length).toBeGreaterThan(0);

      const localStateWarning = [
        '<script lang="ts">',
        '  let { initial }: { initial: number } = $props();',
        '  let doubled = initial * 2;',
        '</script>',
        '<p>{doubled}</p>',
      ].join('\n');
      const warningIssues = await checkSvelteCode(
        'src/lib/components/StaleProp.svelte',
        localStateWarning,
        SVELTE_VERSION,
      );
      expect(warningIssues).toEqual(
        expect.arrayContaining([expect.stringContaining('state_referenced_locally')]),
      );
    },
    SLOW,
  );

  it(
    'passes clean Svelte 5 components and rune modules, including project eslint-disable comments',
    async () => {
      const cleanComponent = [
        '<script lang="ts">',
        '  let { value }: { value: number } = $props();',
        '  let doubled = $derived(value * 2);',
        '</script>',
        '<p>{doubled}</p>',
      ].join('\n');
      expect(
        await checkSvelteCode('src/lib/components/Clean.svelte', cleanComponent, SVELTE_VERSION),
      ).toEqual([]);

      const cleanRuneModule = [
        '// eslint-disable-next-line @typescript-eslint/no-explicit-any -- generic wrapper',
        'export class Holder<T = any> {',
        '  current = $state.raw<T | undefined>(undefined);',
        '}',
      ].join('\n');
      expect(
        await checkSvelteCode('src/lib/state/holder.svelte.ts', cleanRuneModule, SVELTE_VERSION),
      ).toEqual([]);
    },
    SLOW,
  );

  describe('walking and checking a directory tree', () => {
    let root = '';
    beforeEach(() => {
      root = mkdtempSync(path.join(tmpdir(), 'fvoci-autofixer-'));
      writeFileSync(
        path.join(root, 'package.json'),
        JSON.stringify({ devDependencies: { svelte: SVELTE_VERSION } }),
      );
    });
    afterEach(() => {
      rmSync(root, { recursive: true, force: true });
    });
    const write = (name: string, text: string): void => {
      mkdirSync(path.dirname(path.join(root, name)), { recursive: true });
      writeFileSync(path.join(root, name), text);
    };

    it(
      'reports issues across .svelte and .svelte.ts files in both single-process and worker-pool modes',
      async () => {
        write(
          'src/lib/Good.svelte',
          '<script lang="ts">\n  let n = $state(1);\n</script>\n<button onclick={() => (n += 1)}>{n}</button>\n',
        );
        write(
          'src/lib/BadA.svelte',
          '<script lang="ts">\n  let n = $state(1);\n</script>\n<p>{$n}</p>\n',
        );
        write(
          'src/lib/BadB.svelte.ts',
          'export let reassigned = $state(0);\nexport function bump(): void {\n  reassigned += 1;\n}\n',
        );
        write('src/lib/i18n/paraglide/runtime.js', 'export const ignored = 1;\n');

        expect(svelteFiles(root)).toEqual([
          'src/lib/BadA.svelte',
          'src/lib/BadB.svelte.ts',
          'src/lib/Good.svelte',
        ]);

        const inProcess = await checkAutofixer(root, { workers: 1 });
        const pooled = await checkAutofixer(root, { workers: 2 });
        expect(pooled.problems).toEqual(inProcess.problems.slice().sort());
        expect(pooled.problems).toEqual([
          expect.stringContaining('src/lib/BadA.svelte:'),
          expect.stringContaining('src/lib/BadB.svelte.ts:'),
        ]);

        symlinkSync(path.join(root, 'src/lib/Good.svelte'), path.join(root, 'src/lib/Link.svelte'));
        expect(() => svelteFiles(root)).toThrow('src/lib/Link.svelte is a symbolic link');
      },
      SLOW,
    );
  });

  it(
    'check-autofixer.ts refuses unexpected CLI arguments',
    () => {
      const result = spawnSync(process.execPath, ['tools/lint/check-autofixer.ts', '--write'], {
        cwd: ROOT,
        encoding: 'utf8',
      });
      expect(result.status).toBe(2);
      expect(result.stderr).toContain('no arguments');
    },
    SLOW,
  );
});

describe('scoped minimatch security override compatibility', () => {
  it('keeps CJS/default-export consumers in @sveltejs/mcp and openapi-typescript working', () => {
    const mcpDir = realpathSync(packageDirectory('@sveltejs/mcp'));
    const eslintDir = realpathSync(packageDirectory('eslint', mcpDir));
    const eslintRequire = createRequire(path.join(eslintDir, 'package.json'));
    expect(() => {
      eslintRequire('@eslint/eslintrc');
    }).not.toThrow();

    const openapiDir = realpathSync(packageDirectory('openapi-typescript'));
    const redoclyDir = realpathSync(packageDirectory('@redocly/openapi-core', openapiDir));
    const redoclyRequire = createRequire(path.join(redoclyDir, 'package.json'));
    const redoclyMinimatch = redoclyRequire('minimatch') as unknown;
    expect(typeof redoclyMinimatch).toBe('function');
  });
});
