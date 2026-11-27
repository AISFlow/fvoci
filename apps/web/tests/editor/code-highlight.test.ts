/**
 * The colours of code, without a page: the languages and their names, what
 * the highlighter makes of a text (in this process, as the worker does it),
 * the tokens that come back, and how a text is asked for, remembered, left
 * plain when it is too long or takes too long, and given up.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { answer } from '$lib/editor/code-highlight-worker.ts';
import {
  CODE_HIGHLIGHT_LIMIT,
  HIGHLIGHT_TIME,
  knownTokens,
  tokensFor,
  useHighlighter,
  type Highlighter,
} from '$lib/editor/code-highlighter.ts';
import {
  CODE_GRAMMARS,
  CODE_LANGUAGES,
  codeLanguageOf,
  type CodeLanguage,
} from '$lib/editor/code-languages.ts';
import {
  TOKEN_KINDS,
  isCodeTokens,
  moveTokens,
  tokenClassAt,
  type CodeTokens,
} from '$lib/editor/code-tokens.ts';
import { DIAGRAM_LANGUAGE } from '$lib/editor/schema.ts';

const language = (stored: string): CodeLanguage => {
  const found = codeLanguageOf(stored);
  if (!found) throw new Error(`no language ${stored}`);
  return found;
};

/** The runs of a text as `[kind, text]`, for reading in a test. */
function runs(text: string, tokens: CodeTokens): [string, string][] {
  const found: [string, string][] = [];
  for (let index = 0; index < tokens.length; index += 3) {
    const [from = 0, to = 0, kind = 0] = tokens.slice(index, index + 3);
    found.push([TOKEN_KINDS[kind] ?? '?', text.slice(from, to)]);
  }
  return found;
}

const tokensOf = (grammar: string, text: string): number[] | null =>
  answer({ id: 1, grammar, text }).tokens;

describe('the languages', () => {
  it('has about thirty, each named once, over grammars that are all used', () => {
    expect(CODE_LANGUAGES.length).toBeGreaterThanOrEqual(30);
    const names = CODE_LANGUAGES.flatMap((each) => [each.id, ...each.aliases]);
    expect(new Set(names).size).toBe(names.length);
    expect(new Set(CODE_LANGUAGES.map((each) => each.grammar))).toEqual(new Set(CODE_GRAMMARS));
  });

  it('finds a language by its stored name or any other, in any case', () => {
    for (const each of CODE_LANGUAGES) {
      for (const name of [each.id, ...each.aliases]) {
        expect([name, codeLanguageOf(name)]).toEqual([name, each]);
        expect([name, codeLanguageOf(name.toUpperCase())]).toEqual([name, each]);
      }
    }
    expect(codeLanguageOf('c++')?.id).toBe('cpp');
    expect(codeLanguageOf('C#')?.id).toBe('csharp');
    expect(codeLanguageOf('objective-c')?.id).toBe('objectivec');
    expect(codeLanguageOf('ts')?.id).toBe('typescript');
    expect(codeLanguageOf('html')?.grammar).toBe('xml');
  });

  it('knows no language for an unknown name, no name, or a value that is no name', () => {
    for (const stored of [
      '',
      'klingon',
      'constructor',
      '__proto__',
      'toString',
      null,
      undefined,
      3,
    ])
      expect(codeLanguageOf(stored)).toBeUndefined();
  });

  it('leaves the language of diagrams to the diagrams', () => {
    expect(codeLanguageOf(DIAGRAM_LANGUAGE)).toBeUndefined();
  });
});

describe('what the highlighter makes of a text', () => {
  it('colours the words of TypeScript by their kinds', () => {
    const text = 'const answer: number = 42; // the answer\nfunction ask() { return "why"; }';
    const tokens = tokensOf('typescript', text) ?? [];
    expect(isCodeTokens(tokens, text.length)).toBe(true);
    const found = runs(text, tokens);
    expect(found).toContainEqual(['keyword', 'const']);
    expect(found).toContainEqual(['number', '42']);
    expect(found).toContainEqual(['comment', '// the answer']);
    expect(found).toContainEqual(['title', 'ask']);
    expect(found).toContainEqual(['string', '"why"']);
    // What is not coloured has no run: the space, the name of a constant.
    expect(found.map(([, part]) => part)).not.toContain(' ');
  });

  it('answers every grammar it has, for an empty text and for text that is no code at all', () => {
    const odd = '\u0000\u{1F600}<script>"\'`${}/*\\\n\r\n\t'.repeat(20);
    for (const grammar of CODE_GRAMMARS) {
      for (const text of ['', odd]) {
        const tokens = tokensOf(grammar, text);
        expect([grammar, isCodeTokens(tokens, text.length)]).toEqual([grammar, true]);
      }
    }
  });

  it('answers nothing for a grammar it does not have', () => {
    for (const grammar of ['klingon', 'constructor', '__proto__', 'mermaid'])
      expect(tokensOf(grammar, 'a = 1')).toBeNull();
  });
});

describe('the tokens', () => {
  it('are read again before they are drawn', () => {
    expect(isCodeTokens([0, 2, 0, 2, 4, 1], 4)).toBe(true);
    expect(isCodeTokens([], 0)).toBe(true);
    for (const wrong of [
      null,
      'tokens',
      [0, 2],
      [0, 5, 0],
      [2, 2, 0],
      [0, 2, 0, 1, 3, 0],
      [0, 2, TOKEN_KINDS.length],
      [0, 2, -1],
      [0.5, 2, 0],
      [0, 2, '0'],
    ])
      expect([wrong, isCodeTokens(wrong, 4)]).toEqual([wrong, false]);
  });

  it('name a class only for a kind there is', () => {
    expect(tokenClassAt(0)).toBe('doc-code-comment');
    expect(tokenClassAt(TOKEN_KINDS.length)).toBeUndefined();
    expect(tokenClassAt(-1)).toBeUndefined();
  });

  it('move with the text that changed and leave what changed plain', () => {
    // "let x = 1;" → "let xy = 1;": the keyword stays, the number moves by one.
    const tokens = [0, 3, 1, 8, 9, 3];
    expect(moveTokens(tokens, 'let x = 1;', 'let xy = 1;')).toEqual([0, 3, 1, 9, 10, 3]);
    // Typing inside the keyword cuts it at the change.
    expect(moveTokens(tokens, 'let x = 1;', 'leet x = 1;')).toEqual([0, 2, 1, 3, 4, 1, 9, 10, 3]);
    // Everything gone leaves nothing.
    expect(moveTokens(tokens, 'let x = 1;', '')).toEqual([]);
    // Whatever the change, what comes out are tokens of the new text.
    for (const [before, after] of [
      ['abc', 'abcabc'],
      ['aaaa', 'aa'],
      ['ab', 'xab'],
      ['ab', ''],
      ['a\nb', 'a\n\nb'],
    ] as const) {
      const moved = moveTokens([0, before.length, 0], before, after);
      expect([before, after, isCodeTokens(moved, after.length)]).toEqual([before, after, true]);
    }
  });
});

describe('asking for the tokens of a text', () => {
  /** A highlighter that answers as the worker does, and counts what it is asked. */
  function inProcess(): Highlighter & { asked: string[] } {
    const asked: string[] = [];
    return {
      asked,
      ready: () => Promise.resolve(true),
      highlight: (grammar, text) => {
        asked.push(text);
        return Promise.resolve(tokensOf(grammar, text));
      },
      stop: () => undefined,
    };
  }

  afterEach(() => {
    vi.useRealTimers();
    useHighlighter(undefined);
  });

  it('colours a text once and then knows it', async () => {
    const highlighter = inProcess();
    useHighlighter(highlighter);
    const ts = language('ts');
    expect(knownTokens(ts, 'let a = 1;')).toBeUndefined();
    const [first, second] = await Promise.all([
      tokensFor(ts, 'let a = 1;'),
      tokensFor(ts, 'let a = 1;'),
    ]);
    expect(first).not.toBeNull();
    expect(second).toBe(first);
    expect(knownTokens(ts, 'let a = 1;')).toBe(first);
    expect(highlighter.asked).toEqual(['let a = 1;']);
  });

  it('leaves an empty text and a text over the limit plain without asking', async () => {
    // One word as long as this takes TypeScript's grammar about a minute: it is never tried.
    const word = 'a'.repeat(100_000);
    expect(knownTokens(language('ts'), word)).toBeNull();
    const highlighter = inProcess();
    useHighlighter(highlighter);
    const long = 'x'.repeat(CODE_HIGHLIGHT_LIMIT + 1);
    expect(knownTokens(language('ts'), '')).toBeNull();
    expect(knownTokens(language('ts'), long)).toBeNull();
    expect(await tokensFor(language('ts'), long)).toBeNull();
    expect(highlighter.asked).toEqual([]);
  });

  it('gives up a text that takes too long, and leaves texts as long in that grammar plain', async () => {
    vi.useFakeTimers();
    const stop = vi.fn();
    const asked: string[] = [];
    useHighlighter({
      ready: () => Promise.resolve(true),
      highlight: (_grammar, text) => {
        asked.push(text);
        return new Promise(() => undefined);
      },
      stop,
    });
    const slow = 'x'.repeat(1000);
    const coloured = tokensFor(language('ts'), slow);
    await vi.advanceTimersByTimeAsync(HIGHLIGHT_TIME);
    expect(await coloured).toBeNull();
    expect(stop).toHaveBeenCalled();
    // As long or longer: not tried again, not even once. Shorter, or in another grammar: tried.
    expect(knownTokens(language('ts'), `${slow}y`)).toBeNull();
    expect(knownTokens(language('ts'), slow.slice(1))).toBeUndefined();
    expect(knownTokens(language('python'), slow)).toBeUndefined();
    expect(asked).toEqual([slow]);
  });

  it('drops a text nobody waits for any more', async () => {
    const highlighter = inProcess();
    useHighlighter(highlighter);
    const ts = language('ts');
    const stop = new AbortController();
    const first = tokensFor(ts, 'let first = 1;');
    const dropped = tokensFor(ts, 'let dropped = 2;', stop.signal);
    stop.abort();
    expect(await first).not.toBeNull();
    expect(await dropped).toBeNull();
    expect(highlighter.asked).toEqual(['let first = 1;']);
    // Nor remembered as plain: asked again, it is coloured.
    expect(knownTokens(ts, 'let dropped = 2;')).toBeUndefined();
  });

  it('drops a text nobody waits for at once, while the one before waits for the highlighter', async () => {
    let readyNow: ((ready: boolean) => void) | undefined;
    const highlight = vi.fn(() => Promise.resolve(null));
    useHighlighter({
      ready: () =>
        new Promise((resolve) => {
          readyNow = resolve;
        }),
      highlight,
      stop: () => undefined,
    });
    const ts = language('ts');
    const first = tokensFor(ts, 'let first = 1;');
    // Typed on meanwhile: each text is given up for the next.
    const typed = [1, 2, 3].map((at) => {
      const stop = new AbortController();
      const tokens = tokensFor(ts, `let typed = ${String(at)};`, stop.signal);
      stop.abort();
      return tokens;
    });
    const settled = await Promise.race([
      Promise.all(typed),
      new Promise((resolve) => setTimeout(resolve, 0, 'still waiting')),
    ]);
    expect(settled).toEqual([null, null, null]);
    readyNow?.(false);
    expect(await first).toBeNull();
    expect(highlight).not.toHaveBeenCalled();
  });

  it('leaves everything plain when the highlighter cannot start, or answers nonsense', async () => {
    useHighlighter({
      ready: () => Promise.resolve(false),
      highlight: () => Promise.reject(new Error('never asked')),
      stop: () => undefined,
    });
    expect(await tokensFor(language('ts'), 'let a = 1;')).toBeNull();
    useHighlighter({
      ready: () => Promise.resolve(true),
      highlight: () => Promise.resolve([0, 999, 0]),
      stop: () => undefined,
    });
    expect(await tokensFor(language('ts'), 'let a = 1;')).toBeNull();
  });
});
