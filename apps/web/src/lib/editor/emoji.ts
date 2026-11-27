/**
 * Finding an emoji by its name.
 *
 * The lists come from Unicode CLDR (tools/emoji/build.ts writes them) and
 * are fetched the first time someone types a colon and a letter: the emoji
 * with their English names always, the names of another language only when
 * the app runs in it. A search looks at both, so that `:rocket` works in a
 * Korean app and `:로켓` does too.
 */

/** One list as it is stored; see tools/emoji/build.ts. */
interface EmojiFile {
  emoji?: string;
  names: string;
  keywords: string;
}

interface Names {
  names: string[];
  /** Per emoji, its keywords. */
  keywords: string[][];
}

export interface EmojiIndex {
  emoji: string[];
  /** English, then the language of the app when there is a list for it. */
  languages: Names[];
}

export interface EmojiMatch {
  emoji: string;
  /** The name in the language of the app, in English when it has no list. */
  name: string;
}

/** The languages other than English that have a list. */
const LISTS: Record<string, () => Promise<{ default: EmojiFile }>> = {
  ko: () => import('./emoji/ko.json'),
};

const namesOf = (file: EmojiFile): Names => ({
  names: file.names.split('\n'),
  keywords: file.keywords.split('\n').map((line) => (line === '' ? [] : line.split('|'))),
});

const loaded = new Map<string, Promise<EmojiIndex>>();

/** The lists for a locale (`ko`, `ko-KR`, `en`, …), fetched once. */
export function loadEmoji(locale: string): Promise<EmojiIndex> {
  const language = locale.toLowerCase().split('-')[0] ?? '';
  const key = Object.hasOwn(LISTS, language) ? language : 'en';
  let index = loaded.get(key);
  if (!index) {
    index = Promise.all([
      import('./emoji/en.json'),
      key === 'en' ? undefined : LISTS[key]?.(),
    ]).then(([english, own]) => {
      const base: EmojiFile = english.default;
      const emoji = (base.emoji ?? '').split(' ');
      const languages = [namesOf(base)];
      // A list that does not line up with the emoji is of no use.
      if (own && namesOf(own.default).names.length === emoji.length)
        languages.push(namesOf(own.default));
      return { emoji, languages };
    });
    index.catch(() => loaded.delete(key));
    loaded.set(key, index);
  }
  return index;
}

/**
 * How well a name or keyword list answers a query; lower is better,
 * undefined is no match. A name counts for more than a keyword, and the
 * start of a word for more than its middle.
 */
function rank(query: string, name: string, keywords: string[]): number | undefined {
  if (name === query) return 0;
  if (name.startsWith(query)) return 1;
  if (name.includes(` ${query}`)) return 2;
  if (keywords.includes(query)) return 3;
  if (keywords.some((keyword) => keyword.startsWith(query))) return 4;
  if (name.includes(query)) return 5;
  if (keywords.some((keyword) => keyword.includes(query))) return 6;
  return undefined;
}

/**
 * The emoji that answer what was typed after the colon, best first. An
 * underscore stands for a space (`:thumbs_up`). Within one rank the order is
 * the usual order of emoji, faces first.
 */
export function searchEmoji(index: EmojiIndex, typed: string, limit = 8): EmojiMatch[] {
  const query = typed.toLowerCase().replaceAll('_', ' ').trim();
  if (query === '') return [];
  const own = index.languages.at(-1);
  const found: { at: number; rank: number }[] = [];
  index.emoji.forEach((_emoji, at) => {
    let best: number | undefined;
    for (const language of index.languages) {
      const ranked = rank(query, language.names[at] ?? '', language.keywords[at] ?? []);
      if (ranked !== undefined && (best === undefined || ranked < best)) best = ranked;
    }
    if (best !== undefined) found.push({ at, rank: best });
  });
  return found
    .sort((a, b) => a.rank - b.rank || a.at - b.at)
    .slice(0, limit)
    .map(({ at }) => ({ emoji: index.emoji[at] ?? '', name: own?.names[at] ?? '' }));
}
