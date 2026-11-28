/**
 * The read-only view of a body must stay light and must never build markup
 * from text. Both are properties of its source, so the source is read here:
 * everything the view imports, followed to the end, has to be free of the
 * editor and of the Markdown reader, and no file on the way may use an API
 * that turns a string into elements.
 *
 * Two things are fetched on demand and are followed like any other import: the
 * formula library and the diagram library, each behind a module of its own.
 * The diagram library hands back its picture as a string, so one file has to
 * read a string into elements. That file is named here as the exception, and
 * what it may use for it is named too.
 */
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import { CODE_GRAMMARS } from '../../src/lib/editor/code-languages.ts';

const ROOT = path.resolve(import.meta.dirname, '../..');
const LIB = path.join(ROOT, 'src/lib');

const ENTRIES = ['src/lib/editor/render.ts', 'src/lib/editor/BodyView.svelte'];

/** Packages the view must not pull in, however indirectly. */
const HEAVY = [/^@tiptap\//, /^prosemirror-/, /^marked$/, /^marked\//];

/** Ways of making elements out of a string. The view has none of them, with the one exception below. */
const MARKUP_FROM_TEXT = [
  'innerHTML',
  'outerHTML',
  'insertAdjacentHTML',
  '{@html',
  'document.write',
  'DOMParser',
  'parseFromString',
  'createContextualFragment',
  'setHTMLUnsafe',
  'setHTML(',
  'srcdoc',
  'new Function',
  'eval(',
  // The formula library can also give markup as a string; only its DOM path is used.
  'renderToString',
];

/**
 * THE EXCEPTION. The picture of a diagram arrives as a string. This one
 * file reads it, into a document of its own where nothing runs or loads, and
 * builds the picture anew from lists of what is allowed. It may name the
 * parser and nothing else from the list above.
 */
const READS_MARKUP = 'src/lib/editor/safe-svg.ts';
const ITS_MEANS = ['DOMParser', 'parseFromString'];

/**
 * The generated message catalogue: named as something the view imports, not
 * followed into (it is written by a tool at build time and holds no drawing).
 */
const MESSAGES = '$lib/i18n/paraglide/';

/** Every module specifier a source file imports from, statically or not. */
function specifiersOf(source: string): string[] {
  const found = new Set<string>();
  const patterns = [
    /\bimport\s+(?:type\s+)?[^'";]*?\sfrom\s*['"]([^'"]+)['"]/g,
    /\bexport\s+(?:type\s+)?[^'";]*?\sfrom\s*['"]([^'"]+)['"]/g,
    /\bimport\s*['"]([^'"]+)['"]/g,
    /\bimport\s*\(\s*['"]([^'"]+)['"]\s*\)/g,
  ];
  for (const pattern of patterns) {
    for (const match of source.matchAll(pattern)) if (match[1]) found.add(match[1]);
  }
  return [...found];
}

function resolve(from: string, given: string): string | undefined {
  // What a module is fetched as (`?worker`) is no part of its file's name.
  const specifier = given.replace(/\?[a-z&]+$/, '');
  if (specifier.startsWith(MESSAGES)) return undefined;
  if (specifier.startsWith('$lib/')) return path.join(LIB, specifier.slice('$lib/'.length));
  if (specifier.startsWith('.')) return path.resolve(path.dirname(from), specifier);
  return undefined;
}

/** The files reachable from an entry, and the packages they import. */
function graphOf(entry: string): { files: string[]; packages: string[] } {
  const files = new Set<string>();
  const packages = new Set<string>();
  const queue = [path.join(ROOT, entry)];
  for (let file = queue.pop(); file !== undefined; file = queue.pop()) {
    if (files.has(file)) continue;
    expect([file, existsSync(file)]).toEqual([file, true]);
    files.add(file);
    if (!/\.(ts|js|svelte)$/.test(file)) continue;
    for (const specifier of specifiersOf(readFileSync(file, 'utf8'))) {
      const target = resolve(file, specifier);
      if (target) queue.push(target);
      else packages.add(specifier);
    }
  }
  return {
    files: [...files].map((file) => path.relative(ROOT, file)).sort(),
    packages: [...packages].sort(),
  };
}

describe('what the read-only view is made of', () => {
  it('finds imports however they are written', () => {
    expect(
      specifiersOf(
        [
          "import a from 'one';",
          "import { b, type C } from '$lib/two.ts';",
          "import type { D } from './three.ts';",
          "import './four.css';",
          "export { e } from '../five.ts';",
          "export * from 'six';",
          "const f = await import('seven');",
          'import {\n  g,\n  h,\n} from "eight";',
        ].join('\n'),
      ).sort(),
    ).toEqual(
      [
        '$lib/two.ts',
        '../five.ts',
        './four.css',
        './three.ts',
        'eight',
        'one',
        'seven',
        'six',
      ].sort(),
    );
  });

  it('draws with nothing but the list, the validator, the address of files, the width of the numbers of a code block, the names of the kinds of callout and the place of a board view', () => {
    expect(graphOf('src/lib/editor/render.ts')).toEqual({
      files: [
        // A callout's icon, and the name of its kind that the icon is read as.
        'src/lib/editor/block-icons.ts',
        // The place of a board view, drawn into by the app.
        'src/lib/editor/board-view-read.ts',
        'src/lib/editor/callout-tones.ts',
        'src/lib/editor/code-lines.ts',
        'src/lib/editor/drawing.ts',
        'src/lib/editor/files.ts',
        // What draws a toggle and a callout.
        'src/lib/editor/render-blocks.ts',
        'src/lib/editor/render-table.ts',
        'src/lib/editor/render.ts',
        'src/lib/editor/schema-rules.ts',
        'src/lib/editor/schema.ts',
        'src/lib/editor/table-grid.ts',
      ],
      packages: ['$lib/i18n/paraglide/messages.js'],
    });
  });

  it('adds, for the component, the framework, the style sheets, what draws a reference, a file block and a code block, and the libraries that draw formulas, diagrams and colours of code', () => {
    expect(graphOf('src/lib/editor/BodyView.svelte')).toEqual({
      files: [
        'src/lib/editor/BodyView.svelte',
        // An image opened larger (image-zoom.ts finds what is pressed), and its close button's icon.
        'src/lib/editor/ImageLightbox.svelte',
        'src/lib/editor/block-icons.ts',
        'src/lib/editor/board-view-read.ts',
        'src/lib/editor/board-view.css',
        'src/lib/editor/callout-tones.ts',
        // What draws a code block's bar, colours and line numbers, fetched when there is one.
        'src/lib/editor/code-block.css',
        'src/lib/editor/code-copy-texts.ts',
        'src/lib/editor/code-copy.ts',
        'src/lib/editor/code-highlight-worker.ts',
        'src/lib/editor/code-highlighter.ts',
        'src/lib/editor/code-languages.ts',
        'src/lib/editor/code-later.ts',
        'src/lib/editor/code-lines.ts',
        'src/lib/editor/code-paint.ts',
        'src/lib/editor/code-tokens.ts',
        'src/lib/editor/code-view.ts',
        'src/lib/editor/diagram-kind.ts',
        'src/lib/editor/diagram.ts',
        'src/lib/editor/drawing.ts',
        'src/lib/editor/editor.css',
        'src/lib/editor/embeds.ts',
        'src/lib/editor/file-view.ts',
        'src/lib/editor/files.ts',
        'src/lib/editor/image-zoom.ts',
        'src/lib/editor/katex.css',
        'src/lib/editor/math.ts',
        'src/lib/editor/reference-chip.ts',
        'src/lib/editor/render-blocks.ts',
        'src/lib/editor/render-table.ts',
        'src/lib/editor/render.ts',
        'src/lib/editor/safe-svg.ts',
        'src/lib/editor/schema-rules.ts',
        'src/lib/editor/schema.ts',
        'src/lib/editor/table-grid.ts',
        // What a finger's press on a file's name does in a body being written; nothing it imports.
        'src/lib/editor/touch-press.ts',
        'src/lib/keys.ts',
        'src/lib/logic/file-size.ts',
        'src/lib/save.ts',
      ],
      packages: [
        '$lib/i18n/paraglide/messages.js',
        '@lucide/svelte/icons/x',
        ...CODE_GRAMMARS.map((grammar) => `highlight.js/lib/languages/${grammar}`),
        'katex',
        'lowlight',
        'mermaid',
        'svelte',
        'svelte/elements',
      ].sort(),
    });
  });

  it('fetches what draws code blocks with no messages and nothing of the framework, so that the first screen keeps its files', () => {
    // Fetched on its own, a module that reached either would split what the
    // first screen loads of them into more files, one of them too small to
    // be compressed (e2e/shell.spec.ts).
    const { files, packages } = graphOf('src/lib/editor/code-view.ts');
    expect(files.filter((file) => /\.svelte(\.ts)?$/.test(file))).toEqual([]);
    expect(
      packages.filter((name) => name.startsWith(MESSAGES) || /^svelte(\/|$)/.test(name)),
    ).toEqual([]);
  });

  it('fetches the two libraries only on demand, each from one module', () => {
    const importers = (name: string): [string, boolean][] =>
      graphOf('src/lib/editor/BodyView.svelte')
        .files.filter((file) => /\.(ts|svelte)$/.test(file))
        .map((file): [string, string] => [file, readFileSync(path.join(ROOT, file), 'utf8')])
        .filter(([, source]) => specifiersOf(source).includes(name))
        .map(([file, source]) => [
          file,
          // A type may be imported the ordinary way; the code itself only through `import()`.
          !new RegExp(`^import (?!type )[^;]*['"]${name}['"]`, 'm').test(source) &&
            new RegExp(`import\\(['"]${name}['"]\\)`).test(source),
        ]);
    expect(importers('katex')).toEqual([['src/lib/editor/math.ts', true]]);
    expect(importers('mermaid')).toEqual([['src/lib/editor/diagram.ts', true]]);
    // And those two modules are themselves fetched on demand by the one that finds the places.
    const embeds = readFileSync(path.join(LIB, 'editor/embeds.ts'), 'utf8');
    for (const module of ['./math.ts', './diagram.ts']) {
      expect([module, embeds.includes(`await import('${module}')`)]).toEqual([module, true]);
      expect([
        module,
        new RegExp(`^import (?!type )[^;]*'${module.replace('.', '\\.')}'`, 'm').test(embeds),
      ]).toEqual([module, false]);
    }
  });

  it('fetches what draws code only for a body with code, and colours it in a worker of its own', () => {
    const files = graphOf('src/lib/editor/BodyView.svelte').files.filter((file) =>
      /\.(ts|svelte)$/.test(file),
    );
    const sources = files.map((file): [string, string] => [
      file,
      readFileSync(path.join(ROOT, file), 'utf8'),
    ]);
    /** The files that import a module, and whether they do it only through `import()`. */
    const importers = (pattern: RegExp): [string, boolean][] =>
      sources
        .filter(([, source]) => specifiersOf(source).some((name) => pattern.test(name)))
        .map(([file, source]) => [
          file,
          !new RegExp(`^import (?!type )[^;]*['"]${pattern.source}`, 'm').test(source) &&
            new RegExp(`import\\(['"]${pattern.source}`).test(source),
        ]);
    expect(importers(/\.\/code-view\.ts/)).toEqual([['src/lib/editor/code-later.ts', true]]);
    expect(importers(/\.\/code-highlight-worker\.ts/)).toEqual([
      ['src/lib/editor/code-highlighter.ts', true],
    ]);
    // The highlighter and its grammars are in the worker and nowhere else.
    const library = /^(lowlight|highlight\.js)/;
    const users = sources.filter(([, source]) =>
      specifiersOf(source).some((name) => library.test(name)),
    );
    expect(users.map(([file]) => file)).toEqual(['src/lib/editor/code-highlight-worker.ts']);
    // And the worker is started from a file of the app's own, not from a `blob:`.
    const highlighter = readFileSync(path.join(LIB, 'editor/code-highlighter.ts'), 'utf8');
    expect(highlighter).toContain("import('./code-highlight-worker.ts?worker')");
    expect(highlighter).not.toContain('&inline');
  });

  it.each(ENTRIES)('%s reaches neither the editor nor the Markdown reader', (entry) => {
    const { packages } = graphOf(entry);
    for (const name of packages) {
      expect([name, HEAVY.some((pattern) => pattern.test(name))]).toEqual([name, false]);
    }
  });

  it('would notice the editor if it were imported', () => {
    // The same walk from the editor's own module does find what is forbidden.
    const { packages } = graphOf('src/lib/editor/extensions.ts');
    expect(packages.some((name) => HEAVY.some((pattern) => pattern.test(name)))).toBe(true);
    const markdown = graphOf('src/lib/editor/markdown.ts');
    expect(markdown.packages).toContain('marked');
  });

  it.each(ENTRIES)('%s and what it imports never make elements out of a string', (entry) => {
    for (const file of graphOf(entry).files) {
      if (file === READS_MARKUP) continue;
      const source = readFileSync(path.join(ROOT, file), 'utf8');
      for (const api of MARKUP_FROM_TEXT) {
        expect([file, api, source.includes(api)]).toEqual([file, api, false]);
      }
    }
  });

  it('has one file that reads markup, with the parser and nothing else', () => {
    expect(graphOf('src/lib/editor/BodyView.svelte').files).toContain(READS_MARKUP);
    const source = readFileSync(path.join(ROOT, READS_MARKUP), 'utf8');
    const used = MARKUP_FROM_TEXT.filter((api) => source.includes(api));
    expect(used.sort()).toEqual([...ITS_MEANS].sort());
    // One call, into a document of its own.
    expect(source.match(/parseFromString\(/g)).toHaveLength(1);
    expect(source).toContain("new DOMParser().parseFromString(markup, 'text/html')");
    // It imports nothing: what it builds comes from its own lists.
    expect(specifiersOf(source)).toEqual([]);
  });

  it('keeps the editor, too, from turning the libraries’ strings into elements', () => {
    // The editor draws formulas and diagrams through the same modules as the view.
    for (const file of [
      'src/lib/editor/math-nodes.ts',
      'src/lib/editor/math-editing.ts',
      'src/lib/editor/diagram-block.ts',
      'src/lib/editor/editing-place.ts',
      'src/lib/editor/place-state.ts',
      'src/lib/editor/math-block.ts',
      'src/lib/editor/suggestion-list.ts',
      'src/lib/editor/uploads.ts',
      'src/lib/editor/BodyEditor.svelte',
    ]) {
      const source = readFileSync(path.join(ROOT, file), 'utf8');
      for (const api of MARKUP_FROM_TEXT) {
        expect([file, api, source.includes(api)]).toEqual([file, api, false]);
      }
    }
  });
});
