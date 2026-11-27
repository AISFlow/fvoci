/**
 * The message files (`messages/<locale>.json`) and the comparison of their
 * key sets (tools/i18n/keys.ts). The message files are the source: they are
 * edited by hand, and paraglide compiles them.
 */
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import { COMPLETE_LOCALES, compareKeys, keyErrors, messageKeys } from '../../tools/i18n/keys.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');

/** The texts of a message: the message itself, or every text among its variants. */
function textsOf(message: unknown): string[] {
  if (typeof message === 'string') return [message];
  if (typeof message === 'object' && message !== null) {
    return Object.values(message).flatMap(textsOf);
  }
  return [];
}

interface Placeholders {
  /** The names of the values put in at run time: `{name}`, `{count: number}`. */
  values: Set<string>;
  /** The names of the tags: `{#b}…{/b}`, `{#br/}`. */
  tags: Set<string>;
  /** What paraglide would take as text or leave open instead of refusing. */
  problems: string[];
}

/** The values and tags of a text in the inlang message format, and what is not closed. */
function placeholdersOf(text: string): Placeholders {
  const values = new Set<string>();
  const tags = new Set<string>();
  const problems: string[] = [];
  const open: string[] = [];
  for (let index = 0; index < text.length; index += 1) {
    const char = text[index];
    if (char === '\\') {
      index += 1;
    } else if (char === '}') {
      problems.push(`a "}" that closes nothing at ${index}`);
    } else if (char === '{') {
      const end = text.indexOf('}', index);
      const next = text.indexOf('{', index + 1);
      if (end === -1 || (next !== -1 && next < end)) {
        problems.push(`a "{" that is not closed at ${index}`);
        continue;
      }
      const inside = text.slice(index + 1, end).trim();
      index = end;
      if (inside.startsWith('#')) {
        const standalone = inside.endsWith('/');
        const name = inside.slice(1, standalone ? -1 : undefined).trim();
        tags.add(name);
        if (!standalone) open.push(name);
      } else if (inside.startsWith('/')) {
        const name = inside.slice(1).trim();
        if (open.pop() !== name) problems.push(`{/${name}} closes no {#${name}}`);
      } else {
        values.add(inside.split(/[\s:]/)[0] ?? inside);
      }
    }
  }
  for (const name of open) problems.push(`{#${name}} is not closed`);
  return { values, tags, problems };
}

/** The placeholders of every text of a message, together. */
function placeholdersOfMessage(message: unknown): Placeholders {
  const all: Placeholders = { values: new Set(), tags: new Set(), problems: [] };
  for (const one of textsOf(message).map(placeholdersOf)) {
    for (const value of one.values) all.values.add(value);
    for (const tag of one.tags) all.tags.add(tag);
    all.problems.push(...one.problems);
  }
  return all;
}

describe('the values and tags of a message', () => {
  it('are read from the text, with escaped braces left out', () => {
    const found = placeholdersOf(
      'Move {#b}{name}{/b} and {count: number} cards \\{not this\\}{#br/}',
    );
    expect([...found.values]).toEqual(['name', 'count']);
    expect([...found.tags]).toEqual(['b', 'br']);
    expect(found.problems).toEqual([]);
  });

  it('are refused when a brace or a tag is not closed, or closes nothing', () => {
    expect(placeholdersOf('Hi {name').problems).toHaveLength(1);
    expect(placeholdersOf('Hi name}').problems).toHaveLength(1);
    expect(placeholdersOf('{#b}bold').problems).toEqual(['{#b} is not closed']);
    expect(placeholdersOf('bold{/b}').problems).toEqual(['{/b} closes no {#b}']);
  });

  it('are read from every variant of a message with variants', () => {
    const message = [
      {
        declarations: ['input count', 'local countPlural = count: plural'],
        selectors: ['countPlural'],
        match: { 'countPlural=one': '{count} card', 'countPlural=*': '{#b}{count}{/b} cards' },
      },
    ];
    const found = placeholdersOfMessage(message);
    expect([...found.values]).toEqual(['count']);
    expect([...found.tags]).toEqual(['b']);
  });
});

describe('key sets', () => {
  it('names what is missing and what is extra, in the order of the files', () => {
    expect(compareKeys(['a', 'b', 'c', 'd'], ['d', 'x', 'b', 'y'])).toEqual({
      missing: ['a', 'c'],
      extra: ['x', 'y'],
    });
  });

  it('finds no difference between equal sets in a different order', () => {
    expect(compareKeys(['a', 'b'], ['b', 'a'])).toEqual({ missing: [], extra: [] });
  });

  it('reads ids with dots as they are, and a complex message as one key', () => {
    const file = {
      $schema: 'https://inlang.com/schema/inlang-message-format',
      ui_close: 'Close',
      'Sidebar.delete-board': 'Delete board',
      'Sidebar.delete-board.confirm': 'Sure?',
      count: [{ match: { 'countPlural=one': 'one', 'countPlural=*': 'many' } }],
      nested: { title: 'Title', deeper: { text: 'Text' } },
    };
    expect(messageKeys(file)).toEqual([
      'ui_close',
      'Sidebar.delete-board',
      'Sidebar.delete-board.confirm',
      'count',
      'nested.title',
      'nested.deeper.text',
    ]);
  });

  it('accepts a locale that lacks keys, unless it has to be complete', () => {
    const differences = new Map([
      ['ko', { missing: [], extra: [] }],
      ['de', { missing: ['a', 'b'], extra: [] }],
    ]);
    expect(keyErrors(differences, ['ko'])).toEqual([]);
    expect(keyErrors(differences, ['ko', 'de'])).toEqual([
      'de: 2 key(s) missing; this locale has to be complete',
    ]);
  });

  it('refuses a key the base locale does not have, in any locale', () => {
    const differences = new Map([
      ['ko', { missing: [], extra: [] }],
      ['de', { missing: [], extra: ['stale'] }],
    ]);
    expect(keyErrors(differences, ['ko'])).toEqual(['de: 1 key(s) the base locale does not have']);
  });

  it('refuses a complete locale that is not there at all', () => {
    expect(keyErrors(new Map(), ['ko'])).toEqual(['ko: has to be complete but is not a locale']);
  });
});

describe('the message files', () => {
  const settings = JSON.parse(
    readFileSync(path.join(WEB_ROOT, 'project.inlang/settings.json'), 'utf8'),
  ) as { baseLocale: string; locales: string[] };
  const messages = (locale: string): Record<string, unknown> =>
    JSON.parse(readFileSync(path.join(WEB_ROOT, `messages/${locale}.json`), 'utf8')) as Record<
      string,
      unknown
    >;

  it('are those of the two complete locales, English and Korean', () => {
    expect(settings.baseLocale).toBe('en');
    expect(settings.locales).toEqual(['en', 'ko']);
    for (const locale of COMPLETE_LOCALES) expect(settings.locales).toContain(locale);
    for (const locale of settings.locales)
      expect(messageKeys(messages(locale)).length).toBeGreaterThan(0);
  });

  it('give Korean every key of English and no locale a key English lacks', () => {
    const english = messageKeys(messages('en'));
    expect(compareKeys(english, messageKeys(messages('ko')))).toEqual({ missing: [], extra: [] });
    for (const locale of settings.locales) {
      expect(compareKeys(english, messageKeys(messages(locale))).extra, locale).toEqual([]);
    }
  });

  it('name the product where the English text named it "Boards", and only there', () => {
    for (const id of [
      'BoardPage.newVersion',
      'error.invalid-read-only-board',
      'error.not-logged-in',
    ]) {
      expect(messages('en')[id]).toContain('fvoci');
      expect(messages('en')[id]).not.toContain('Boards');
      expect(messages('ko')[id]).toContain('fvoci');
    }
    // The default category of the sidebar holds boards and wikis: it is shown as "General".
    expect(messages('en')['SidebarCategories.CategoryMenu.DeleteModal.Body']).toContain(
      'the General category',
    );
    expect(messages('en').shell_default_category).toBe('General');
  });

  it('use only the values and tags of the English message, each closed', () => {
    const english = messages('en');
    const problems: string[] = [];
    for (const locale of settings.locales) {
      for (const [id, message] of Object.entries(messages(locale))) {
        const own = placeholdersOfMessage(message);
        const base = placeholdersOfMessage(english[id]);
        for (const problem of own.problems) problems.push(`${locale} ${id}: ${problem}`);
        for (const value of own.values) {
          if (!base.values.has(value))
            problems.push(`${locale} ${id}: {${value}} is not in English`);
        }
        for (const tag of own.tags) {
          if (!base.tags.has(tag)) problems.push(`${locale} ${id}: {#${tag}} is not in English`);
        }
      }
    }
    expect(problems).toEqual([]);
  });

  const OUTSIDE_ADDRESS = /https?:\/\/|\bwww\.|\b[a-z0-9-]+\.(?:com|org|net|io|html)\b/i;
  const forbidden = (text: string): string | undefined =>
    OUTSIDE_ADDRESS.test(text) ? 'the address of an outside site' : undefined;

  it('find an outside address in a text', () => {
    expect(forbidden('See https://example.org')).toBe('the address of an outside site');
    expect(forbidden('Visit docs.example.com')).toBe('the address of an outside site');
    expect(forbidden('보드를 추가합니다')).toBeUndefined();
  });

  it('name no outside site, in any locale', () => {
    const found = settings.locales.flatMap((locale) =>
      Object.entries(messages(locale)).flatMap(([id, message]) =>
        textsOf(message).flatMap((text) => {
          const reason = forbidden(text);
          return reason === undefined ? [] : [`${locale} ${id}: ${reason}`];
        }),
      ),
    );
    expect(found).toEqual([]);
  });

  it('keep the rewritten English message translated in Korean', () => {
    const id = 'ShareBoard.searchPlaceholder';
    expect(messages('en')[id]).toBeDefined();
    expect(messages('ko')[id]).toBeDefined();
  });

  // A Korean particle depends on how the word before it ends (을/를, 이/가, …),
  // so it cannot follow a value that is put in at run time: `{name}` or the end
  // of a tag, `{/b}`.
  const particleAfterValue = /\}["”]?(?:은|는|이|가|을|를|로|으로|와|과)(?:\s|$)/;

  it('find a Korean particle right after a value', () => {
    expect(particleAfterValue.test('{name}을 지웁니다')).toBe(true);
    expect(particleAfterValue.test('{#b}{name}{/b}를 옮깁니다')).toBe(true);
    expect(particleAfterValue.test('"{name}" 보드')).toBe(false);
  });

  it('have no Korean particle right after a value', () => {
    const risky = Object.entries(messages('ko')).filter(([, message]) =>
      textsOf(message).some((text) => particleAfterValue.test(text)),
    );
    expect(risky.map(([id]) => id)).toEqual([]);
  });
});
