/**
 * Emoji by name: when a colon starts a name, what a search finds in the real
 * lists (English always, Korean when the app is Korean), and that the lists
 * in the repository are what the tool makes from the installed CLDR data.
 */
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import { findEmojiTrigger } from '$lib/editor/emoji-trigger.ts';
import { loadEmoji, searchEmoji } from '$lib/editor/emoji.ts';

import { EMOJI_LOCALES, emojiFiles, qualified, type EmojiFile } from '../../tools/emoji/build.ts';
import { KATEX_CSS_FILE } from '../../tools/math/katex-css.ts';

const ROOT = path.resolve(import.meta.dirname, '../..');

describe('a colon that starts the name of an emoji', () => {
  it.each<[string, string, number]>([
    [':', '', 0],
    [':rock', 'rock', 0],
    ['좋아요 :rock', 'rock', 4],
    ['a :thumbs_up', 'thumbs_up', 2],
    [':+1', '+1', 0],
    ['(:smile', 'smile', 1],
    ['"quoted":heart', 'heart', 8],
    ['끝.:tada', 'tada', 2],
    // Scripts that put no space between words: the colon may follow a letter.
    ['안녕:', '', 2],
    ['안녕:smile', 'smile', 2],
    ['안녕:웃', '웃', 2],
    ['회의록:로켓', '로켓', 3],
    ['你好:smile', 'smile', 2],
    ['你好:笑', '笑', 2],
    ['こんにちは:smile', 'smile', 5],
    ['カタカナ:ロケット', 'ロケット', 4],
    ['한글 뒤:', '', 4],
    // A node that is not text (a mention, a formula) stands in the line as this character.
    ['￼:smile', 'smile', 1],
    // A letter of another alphabet is no ASCII letter either.
    ['été:smile', 'smile', 3],
    // The character in front may take two code units.
    ['😀:smile', 'smile', 2],
    ['𠀀:smile', 'smile', 2],
  ])('%j offers emoji for %j', (text, query, index) => {
    expect(findEmojiTrigger(text)).toEqual({ index, query });
  });

  it.each([
    ['10:30'],
    ['10:'],
    ['http:'],
    ['https://example.com'],
    ['mailto:someone'],
    ['a:b'],
    ['Z:'],
    ['note:smile'],
    ['x9:smile'],
    // The name has ended: a space, a second colon, or something a name is not typed with.
    [':smile '],
    [':smile and more'],
    [':smile:'],
    ['안녕:smile.'],
    ['안녕: smile'],
    ['no colon at all'],
    [''],
    // Only the last colon counts, and here it follows a letter.
    ['안녕:smile:x'],
  ])('%j offers none', (text) => {
    expect(findEmojiTrigger(text)).toBeUndefined();
  });
});

describe('searching the lists', () => {
  const emojiOf = (matches: { emoji: string }[]): string[] => matches.map((match) => match.emoji);

  it('finds an emoji by its English name, by a word of it and by a keyword', async () => {
    const english = await loadEmoji('en');
    expect(english.languages).toHaveLength(1);
    expect(searchEmoji(english, 'rocket')[0]).toEqual({ emoji: '🚀', name: 'rocket' });
    expect(emojiOf(searchEmoji(english, 'rock'))).toContain('🚀');
    expect(searchEmoji(english, 'red heart')[0]?.emoji).toBe('❤️');
    expect(searchEmoji(english, 'red_heart')[0]?.emoji).toBe('❤️');
    expect(searchEmoji(english, 'thumbs')[0]?.emoji).toBe('👍');
    expect(emojiOf(searchEmoji(english, 'tada'))).toContain('🎉');
    expect(searchEmoji(english, 'ROCKET')[0]?.emoji).toBe('🚀');
    expect(emojiOf(searchEmoji(english, 'south korea'))).toContain('🇰🇷');
  });

  it('puts a name before a keyword and the start of a word before its middle', async () => {
    const english = await loadEmoji('en');
    const found = searchEmoji(english, 'smil', 30);
    const names = found.map((match) => match.name);
    const firstInside = names.findIndex((name) => !/(^| )smil/.test(name));
    const lastAtStart = names.findLastIndex((name) => name.startsWith('smil'));
    expect(lastAtStart).toBeGreaterThanOrEqual(0);
    if (firstInside !== -1) expect(firstInside).toBeGreaterThan(lastAtStart);
  });

  it('gives no more than it is asked for, and nothing for nothing', async () => {
    const english = await loadEmoji('en');
    expect(searchEmoji(english, 'face')).toHaveLength(8);
    expect(searchEmoji(english, 'face', 3)).toHaveLength(3);
    expect(searchEmoji(english, '')).toEqual([]);
    expect(searchEmoji(english, '   ')).toEqual([]);
    expect(searchEmoji(english, 'zzzzqqqq')).toEqual([]);
  });

  it('does not find Korean names while the app is not Korean', async () => {
    for (const locale of ['en', 'ja', 'de', 'pt-BR', 'zh-Hans', '']) {
      const index = await loadEmoji(locale);
      expect([locale, index.languages.length]).toEqual([locale, 1]);
      expect([locale, searchEmoji(index, '로켓')]).toEqual([locale, []]);
      expect([locale, searchEmoji(index, 'rocket')[0]?.emoji]).toEqual([locale, '🚀']);
    }
  });

  it('finds an emoji by its Korean name and keywords, and still by the English ones, in a Korean app', async () => {
    for (const locale of ['ko', 'ko-KR', 'KO']) {
      const korean = await loadEmoji(locale);
      expect(korean.languages).toHaveLength(2);
      // Shown under its Korean name whichever language found it.
      expect(searchEmoji(korean, '로켓')[0]).toEqual({ emoji: '🚀', name: '로켓' });
      expect(searchEmoji(korean, 'rocket')[0]).toEqual({ emoji: '🚀', name: '로켓' });
      expect(emojiOf(searchEmoji(korean, '하트', 40))).toContain('❤️');
      expect(emojiOf(searchEmoji(korean, '웃', 40))).toContain('😀');
      expect(emojiOf(searchEmoji(korean, '대한민국'))).toContain('🇰🇷');
      expect(searchEmoji(korean, 'thumbs')[0]?.emoji).toBe('👍');
    }
  });

  it('fetches a list once', async () => {
    expect(await loadEmoji('ko')).toBe(await loadEmoji('ko-KR'));
    expect(await loadEmoji('en')).toBe(await loadEmoji('fr'));
  });
});

describe('the lists in the repository', () => {
  const files = emojiFiles();
  const read = (locale: string): EmojiFile =>
    JSON.parse(
      readFileSync(path.join(ROOT, `src/lib/editor/emoji/${locale}.json`), 'utf8'),
    ) as EmojiFile;

  it('are what the tool writes from the installed CLDR data', () => {
    expect([...files.keys()].sort()).toEqual(
      EMOJI_LOCALES.map((locale) => `src/lib/editor/emoji/${locale}.json`).sort(),
    );
    for (const [name, content] of files) {
      // On a failure: run `node tools/emoji/build.ts`.
      expect([name, readFileSync(path.join(ROOT, name), 'utf8') === content]).toEqual([name, true]);
    }
  });

  it('line up: one name and one line of keywords per emoji, in every locale', () => {
    const emoji = (read('en').emoji ?? '').split(' ');
    expect(emoji.length).toBeGreaterThan(1800);
    expect(new Set(emoji).size).toBe(emoji.length);
    for (const locale of EMOJI_LOCALES) {
      const file = read(locale);
      expect([locale, file.names.split('\n').length]).toEqual([locale, emoji.length]);
      expect([locale, file.keywords.split('\n').length]).toEqual([locale, emoji.length]);
      expect([locale, file.names.split('\n').filter((name) => name === '')]).toEqual([locale, []]);
      expect([locale, 'emoji' in file]).toEqual([locale, locale === 'en']);
    }
  });

  it('hold emoji that are drawn as emoji, without the variants by skin tone', () => {
    const emoji = (read('en').emoji ?? '').split(' ');
    const rgi = new RegExp('^\\p{RGI_Emoji}$', 'v');
    expect(emoji.filter((entry) => !rgi.test(entry))).toEqual([]);
    expect(emoji.filter((entry) => /[\u{1F3FB}-\u{1F3FF}]/u.test(entry))).toEqual([]);
    // In the usual order, faces first.
    expect(emoji[0]).toBe('😀');
    for (const expected of ['🚀', '❤️', '👍', '🇰🇷', '1️⃣', '👨‍👩‍👧', '🏴󠁧󠁢󠁷󠁬󠁳󠁿', '©️']) {
      expect([expected, emoji.includes(expected)]).toEqual([expected, true]);
    }
  });

  it('gives a symbol the selector that makes it an emoji', () => {
    expect(qualified('❤')).toBe('❤️');
    expect(qualified('😀')).toBe('😀');
    expect(qualified('1⃣')).toBe('1️⃣');
    expect(qualified('{')).toBeUndefined();
    expect(qualified('→')).toBeUndefined();
  });
});

describe('the style sheet of the formula library in the repository', () => {
  it('is what the tool writes from the installed package, and names WOFF2 files alone', () => {
    // The tool formats what it writes, which it can only do when run on its
    // own. On a failure: run `node tools/math/katex-css.ts`.
    const check = spawnSync(process.execPath, ['tools/math/katex-css.ts', '--check'], {
      cwd: ROOT,
      encoding: 'utf8',
    });
    expect([check.status, check.stderr]).toEqual([0, '']);
    const css = readFileSync(path.join(ROOT, KATEX_CSS_FILE), 'utf8');
    const addresses = [...css.matchAll(/url\(([^)]+)\)/g)].map((match) => match[1] ?? '');
    expect(addresses).toHaveLength(20);
    for (const address of addresses) {
      expect(address).toMatch(/^'katex\/dist\/fonts\/KaTeX_[\w-]+\.woff2'$/);
    }
    expect(css).not.toMatch(/\.woff\b|\.ttf\b|truetype/);
    // No colour of its own: a formula takes the colour of the text around it.
    expect(css).not.toMatch(/#[0-9a-f]{3,8}\b|rgb\(|hsl\(/i);
  });
});
