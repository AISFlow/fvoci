/**
 * Zero-exception file and function length gate for `src`.
 *
 * Every measured file under `src` must be at most `MAX_FILE_LINES` (500) lines,
 * and every function, snippet, or class static block at most `MAX_FUNCTION_LINES`
 * (100) lines. No exception list exists: code that would exceed a limit is split
 * along a clean responsibility boundary first.
 */
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

import { ESLint, type Linter, type Rule } from 'eslint';
import svelte from 'eslint-plugin-svelte';
import ts from 'typescript-eslint';

import svelteConfig from '../../svelte.config.js';

/** The most lines a file of `src` may have. */
export const MAX_FILE_LINES = 500;
/** The most lines a function may have. */
export const MAX_FUNCTION_LINES = 100;

/** The folder that is measured, from the web app's root. */
export const MEASURED = 'src';
const EXTENSIONS = ['.ts', '.svelte', '.js'];
const UNMEASURED = ['.mts', '.cts', '.mjs', '.cjs', '.tsx', '.jsx'];
export const GENERATED = ['src/lib/i18n/paraglide/', 'src/lib/api/contract.ts'];

export interface Subject {
  kind: 'file' | 'function';
  file: string;
  name?: string;
}

export interface Length extends Subject {
  lines: number;
  at: number;
}

function limitOf(subject: Subject): number {
  return subject.kind === 'file' ? MAX_FILE_LINES : MAX_FUNCTION_LINES;
}

function shown(subject: Subject): string {
  return subject.kind === 'file' ? subject.file : `${subject.file}: function ${subject.name ?? ''}`;
}

/**
 * Checks every measured file and function directly against `MAX_FILE_LINES` and
 * `MAX_FUNCTION_LINES` with zero exceptions.
 */
export function judge(measured: readonly Length[]): string[] {
  return measured
    .filter((length) => length.lines > limitOf(length))
    .sort((a, b) => (a.file < b.file ? -1 : a.file > b.file ? 1 : a.at - b.at))
    .map(
      (length) =>
        `${length.file}:${length.at}: ${shown(length)} has ${length.lines} lines, the limit is ${limitOf(length)}`,
    );
}

export function countLines(text: string): number {
  if (text === '') return 0;
  const lines = text.split(/\r\n|\r|\n/);
  return lines.length - (lines.at(-1) === '' ? 1 : 0);
}

export function measuredFiles(root: string): string[] {
  const files: string[] = [];
  const walk = (relative: string): void => {
    const entries = readdirSync(path.join(root, relative), { withFileTypes: true });
    for (const entry of entries.sort((a, b) => (a.name < b.name ? -1 : 1))) {
      const child = `${relative}/${entry.name}`;
      if (GENERATED.some((made) => child === made || `${child}/` === made)) continue;
      if (entry.isSymbolicLink()) throw new Error(`${child} is a symbolic link`);
      if (entry.isDirectory()) walk(child);
      else if (EXTENSIONS.some((extension) => entry.name.endsWith(extension))) files.push(child);
      else if (UNMEASURED.some((extension) => entry.name.endsWith(extension))) {
        throw new Error(
          `${child} is code in a form that is not measured: write it as .ts, .svelte or .js`,
        );
      }
    }
  };
  walk(MEASURED);
  return files;
}

interface Named {
  type: string;
  id?: { type: string; name?: string } | null;
  key?: { type: string; name?: unknown };
  callee?: Named;
  property?: { type: string; name?: string };
  name?: unknown;
  parent?: Named | null;
}

const FUNCTIONS = ['FunctionDeclaration', 'FunctionExpression', 'ArrowFunctionExpression'];
const BLOCKS = ['SvelteSnippetBlock', 'StaticBlock'];
const MEMBERS = ['Property', 'MethodDefinition', 'PropertyDefinition'];

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function ownName(node: Named): string {
  const { id, parent } = node;
  if (node.type === 'StaticBlock') return 'static{}';
  if (id?.name !== undefined && node.type !== 'ArrowFunctionExpression') return id.name;
  if (parent === undefined || parent === null) return '<anonymous>';
  if (parent.type === 'VariableDeclarator' && parent.id?.type === 'Identifier') {
    return parent.id.name ?? '<anonymous>';
  }
  if (MEMBERS.includes(parent.type) && parent.key !== undefined) {
    const name = text(parent.key.name);
    if (parent.key.type === 'Identifier' && name !== undefined) return name;
    if (parent.key.type === 'PrivateIdentifier' && name !== undefined) return `#${name}`;
  }
  if (
    (parent.type === 'CallExpression' || parent.type === 'NewExpression') &&
    parent.callee !== node
  ) {
    const callee = parent.callee;
    const called = callee?.type === 'MemberExpression' ? callee.property?.name : text(callee?.name);
    if (called !== undefined) return `${called}()`;
  }
  if (parent.type === 'SvelteMustacheTag' && parent.parent?.key !== undefined) {
    const attribute = text(parent.parent.key.name);
    if (attribute !== undefined) return attribute;
  }
  return '<anonymous>';
}

function nameOf(node: Rule.Node): string {
  const self: Named = node;
  const names = [ownName(self)];
  for (let outer = self.parent; outer; outer = outer.parent) {
    if (FUNCTIONS.includes(outer.type) || BLOCKS.includes(outer.type)) {
      names.unshift(ownName(outer));
    } else if (outer.type === 'ClassDeclaration' || outer.type === 'ClassExpression') {
      names.unshift(outer.id?.name ?? ownName(outer));
    }
  }
  return names.join('/');
}

const everyFunction: Rule.RuleModule = {
  meta: {
    type: 'suggestion',
    docs: { description: 'reports the length of every function' },
    schema: [],
    messages: { length: '{{lines}} {{name}}' },
  },
  create(context) {
    const report = (node: Rule.Node): void => {
      if (node.loc === null || node.loc === undefined) return;
      const lines = node.loc.end.line - node.loc.start.line + 1;
      context.report({
        node,
        messageId: 'length',
        data: { lines: String(lines), name: nameOf(node) },
      });
    };
    return Object.fromEntries([...FUNCTIONS, ...BLOCKS].map((type) => [type, report]));
  },
};

const RULE = 'fvoci-lengths/every-function';

export const measuringConfig: Linter.Config[] = [
  ...svelte.configs.base,
  { files: ['**/*.ts'], ignores: ['**/*.svelte.ts'], languageOptions: { parser: ts.parser } },
  {
    files: ['**/*.svelte', '**/*.svelte.ts', '**/*.svelte.js'],
    languageOptions: { parserOptions: { parser: ts.parser, svelteConfig } },
  },
  {
    linterOptions: { noInlineConfig: true, reportUnusedDisableDirectives: 'off' },
    plugins: { 'fvoci-lengths': { rules: { 'every-function': everyFunction } } },
    rules: { [RULE]: 'error', 'svelte/comment-directive': 'off' },
  },
];

const NOT_OBEYED = "has no effect because you have 'noInlineConfig'";

export function functionsOf(file: string, messages: readonly Linter.LintMessage[]): Length[] {
  const functions: Length[] = [];
  for (const message of messages) {
    if (message.ruleId === RULE) {
      const [lines, name = ''] = message.message.split(' ');
      functions.push({ kind: 'function', file, name, lines: Number(lines), at: message.line });
    } else if (!message.message.includes(NOT_OBEYED)) {
      throw new Error(`${file}:${message.line}: not measured: ${message.message}`);
    }
  }
  return functions;
}

export async function measure(root: string): Promise<Length[]> {
  const files = measuredFiles(root);
  const measured: Length[] = files.map((file) => ({
    kind: 'file',
    file,
    lines: countLines(readFileSync(path.join(root, file), 'utf8')),
    at: 1,
  }));
  const eslint = new ESLint({
    cwd: root,
    overrideConfigFile: true,
    overrideConfig: measuringConfig,
    ignore: false,
  });
  const results = await eslint.lintFiles(files);
  for (const result of results) {
    const file = path.relative(root, result.filePath).split(path.sep).join('/');
    measured.push(...functionsOf(file, result.messages));
  }
  return measured;
}

export async function check(root: string): Promise<{ measured: Length[]; problems: string[] }> {
  const measured = await measure(root);
  return { measured, problems: judge(measured) };
}
