/**
 * ESLint configuration and out-of-band source check forbidding browser-native
 * modal dialogs (`alert`, `confirm`, `prompt`) and any inline `eslint-disable`
 * comment that could silence them under `src/**`.
 *
 * Because ESLint's own inline comment processor lets bare `/* eslint-disable * /`
 * or `// eslint-disable-line fvoci-dialogs/no-dialog-disable, no-restricted-globals`
 * suppress an ESLint rule from inside the linted file, `findDialogDisableViolations`
 * and `checkDialogs` also scan raw source files outside ESLint (`check-dialogs.ts`).
 */
import { lstatSync, readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

import type { Linter, Rule } from 'eslint';

export const BROWSER_DIALOGS = ['alert', 'confirm', 'prompt'] as const;
export const GLOBAL_DIALOG_RECEIVERS = [
  'window',
  'globalThis',
  'self',
  'frames',
  'parent',
  'top',
] as const;

export const DIALOG_CHECKED_DIR = 'src';
export const DIALOG_EXTENSIONS = ['.ts', '.svelte', '.js'] as const;
export const DIALOG_GENERATED = [
  'src/lib/i18n/paraglide/',
  'src/styles/fonts/',
  'src/lib/api/contract.ts',
] as const;

function dialogMessage(call: string): string {
  return `Browser \`${call}()\` is forbidden in app code; render inline feedback, a toast, or a Dialog / AlertDialog component instead.`;
}

const BLOCK_DISABLE_COMMENT =
  /(?:\/\*|<!--)\s*eslint-disable(?:-next-line|-line)?\b([\s\S]*?)(?:\*\/|-->|$)/g;
const LINE_DISABLE_COMMENT = /\/\/\s*eslint-disable(?:-next-line|-line)?\b([^\r\n]*)/g;
const INLINE_CONFIG_COMMENT = /(?:\/\*|<!--)\s*eslint\s+([\s\S]*?)(?:\*\/|-->|$)/g;
export const PROTECTED_DIALOG_RULES = [
  'fvoci-dialogs/no-dialog-disable',
  'no-restricted-globals',
  'no-restricted-properties',
] as const;

function lineNumberAt(source: string, index: number): number {
  let line = 1;
  for (let i = 0; i < index; i += 1) {
    if (source.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

interface DirectiveHit {
  readonly index: number;
  readonly kind: 'bareDisable' | 'protectedDisable' | 'protectedConfig';
  readonly rule?: string;
}

function scanDialogDirectiveHits(source: string): DirectiveHit[] {
  const hits: DirectiveHit[] = [];
  for (const pattern of [BLOCK_DISABLE_COMMENT, LINE_DISABLE_COMMENT]) {
    for (const match of source.matchAll(pattern)) {
      const body = ((match[1] ?? '').split('--')[0] ?? '').trim();
      if (body === '') {
        hits.push({ index: match.index, kind: 'bareDisable' });
        continue;
      }
      const tokens = body
        .split(/[,\s]+/)
        .map((token) => token.trim())
        .filter((token) => token !== '');
      for (const rule of PROTECTED_DIALOG_RULES) {
        if (tokens.includes(rule)) {
          hits.push({ index: match.index, kind: 'protectedDisable', rule });
        }
      }
    }
  }
  for (const match of source.matchAll(INLINE_CONFIG_COMMENT)) {
    const body = match[1] ?? '';
    for (const rule of PROTECTED_DIALOG_RULES) {
      if (body.includes(rule)) {
        hits.push({ index: match.index, kind: 'protectedConfig', rule });
      }
    }
  }
  return hits.sort((a, b) => a.index - b.index);
}

/**
 * Pure text scan catching bare `eslint-disable` directives, `eslint-disable`
 * naming a protected dialog rule, and inline `/* eslint ... * /` rule overrides.
 */
export function findDialogDisableViolations(file: string, source: string): string[] {
  return scanDialogDirectiveHits(source).map((hit) => {
    const line = lineNumberAt(source, hit.index);
    if (hit.kind === 'bareDisable') {
      return `${file}:${line}: bare \`eslint-disable\` without rule names is forbidden; name the exact non-protected rule being disabled`;
    }
    if (hit.kind === 'protectedConfig') {
      return `${file}:${line}: do not reconfigure \`${hit.rule ?? ''}\` with inline \`eslint\` config; replace native browser dialogs with app UI`;
    }
    return `${file}:${line}: do not silence \`${hit.rule ?? ''}\` with \`eslint-disable\`; replace native browser dialogs with app UI`;
  });
}

export function dialogSourceFiles(root: string): string[] {
  const files: string[] = [];
  const walk = (relative: string): void => {
    const entries = readdirSync(path.join(root, relative), { withFileTypes: true });
    for (const entry of entries.sort((a, b) => (a.name < b.name ? -1 : 1))) {
      const child = `${relative}/${entry.name}`;
      if (DIALOG_GENERATED.some((made) => child === made || `${child}/` === made)) continue;
      if (entry.isSymbolicLink() || lstatSync(path.join(root, child)).isSymbolicLink()) {
        throw new Error(`${child} is a symbolic link`);
      }
      if (entry.isDirectory()) walk(child);
      else if (DIALOG_EXTENSIONS.some((ext) => entry.name.endsWith(ext))) files.push(child);
    }
  };
  walk(DIALOG_CHECKED_DIR);
  return files;
}

export function checkDialogs(root: string): { files: string[]; problems: string[] } {
  const files = dialogSourceFiles(root);
  const problems: string[] = [];
  for (const file of files) {
    const source = readFileSync(path.join(root, file), 'utf8');
    problems.push(...findDialogDisableViolations(file, source));
  }
  return { files, problems };
}

/** Refuses `eslint-disable` and inline `/* eslint ... * /` comments targeting browser-dialog rules. */
const noDialogDisable: Rule.RuleModule = {
  meta: {
    type: 'problem',
    docs: { description: 'forbids silencing browser modal dialog lint rules' },
    schema: [],
    messages: {
      bareDisable:
        'Bare `eslint-disable` without rule names is forbidden; name the exact non-protected rule being disabled.',
      protectedRule:
        'Do not silence `{{rule}}` with `eslint-disable`; replace native browser dialogs with app UI.',
      protectedConfig:
        'Do not reconfigure `{{rule}}` with inline `eslint` config; replace native browser dialogs with app UI.',
    },
  },
  create(context) {
    return {
      Program(node) {
        const text = context.sourceCode.getText();
        for (const hit of scanDialogDirectiveHits(text)) {
          const loc = context.sourceCode.getLocFromIndex(hit.index);
          if (hit.kind === 'bareDisable') {
            context.report({ node, loc, messageId: 'bareDisable' });
          } else if (hit.kind === 'protectedConfig') {
            context.report({
              node,
              loc,
              messageId: 'protectedConfig',
              data: { rule: hit.rule ?? '' },
            });
          } else {
            context.report({
              node,
              loc,
              messageId: 'protectedRule',
              data: { rule: hit.rule ?? '' },
            });
          }
        }
      },
    };
  },
};

export const noBrowserDialogs: Linter.Config = {
  files: ['src/**'],
  plugins: {
    'fvoci-dialogs': {
      rules: {
        'no-dialog-disable': noDialogDisable,
      },
    },
  },
  rules: {
    'fvoci-dialogs/no-dialog-disable': 'error',
    'no-restricted-globals': [
      'error',
      ...BROWSER_DIALOGS.map((name) => ({
        name,
        message: dialogMessage(name),
      })),
    ],
    'no-restricted-properties': [
      'error',
      ...GLOBAL_DIALOG_RECEIVERS.flatMap((object) =>
        BROWSER_DIALOGS.map((property) => ({
          object,
          property,
          message: dialogMessage(`${object}.${property}`),
        })),
      ),
    ],
  },
};
