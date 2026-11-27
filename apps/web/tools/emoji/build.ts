/**
 * Writes the emoji lists of the editor from Unicode CLDR.
 *
 *   node tools/emoji/build.ts            write src/lib/editor/emoji/<locale>.json
 *   node tools/emoji/build.ts --check    fail when they are not what would be written
 *
 * The source is the annotation data of CLDR (the packages
 * `cldr-annotations-full` and `cldr-annotations-derived-full`, development
 * dependencies, Unicode-3.0 licence): for every character a name and a list
 * of keywords, per language. From that this tool keeps
 *
 * - the characters that are emoji recommended for general interchange
 *   (`\p{RGI_Emoji}` of this Node's Unicode tables), in the order CLDR sorts
 *   emoji (faces first), each in its fully qualified form;
 * - not the variants by skin tone, which would triple the list for a search
 *   by name (the base emoji is listed), and not the building blocks that are
 *   no emoji on their own (the skin tones and hair styles themselves);
 * - per locale the name, and the keywords that say more than the name does.
 *
 * English is the base file: it holds the emoji themselves. Every other
 * locale is a file of names and keywords in the same order, fetched only
 * when the app runs in that language. The output is committed; a test fails
 * when it no longer matches the installed data.
 */
import { readFileSync, writeFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';

import { packageDirectory } from '../licenses/packages.ts';

const WEB_ROOT = path.resolve(import.meta.dirname, '../..');
export const EMOJI_DIRECTORY = 'src/lib/editor/emoji';

/** The base locale and the others, by the name of their CLDR folder. Add a locale here. */
export const EMOJI_LOCALES = ['en', 'ko'] as const;

interface Annotation {
  default?: string[];
  tts?: string[];
}
type Annotations = Record<string, Annotation>;

// Built from strings: these use the `v` flag (properties of strings), which
// the project's TypeScript target does not let a literal carry yet.
const RGI = new RegExp('^\\p{RGI_Emoji}$', 'v');
const SKIN_TONE = /[\u{1F3FB}-\u{1F3FF}]/u;
const HAIR = /^[\u{1F9B0}-\u{1F9B3}]$/u;
/** Shown as a symbol unless the variation selector says "as emoji". */
const TEXT_BY_DEFAULT = new RegExp('^(?!\\p{Emoji_Presentation})\\p{Emoji}$', 'v');
const VARIATION_SELECTOR = '\u{FE0F}';

function read(locale: string): Annotations {
  const plain = JSON.parse(
    readFileSync(
      path.join(
        packageDirectory('cldr-annotations-full'),
        'annotations',
        locale,
        'annotations.json',
      ),
      'utf8',
    ),
  ) as { annotations: { annotations: Annotations } };
  const derived = JSON.parse(
    readFileSync(
      path.join(
        packageDirectory('cldr-annotations-derived-full'),
        'annotationsDerived',
        locale,
        'annotations.json',
      ),
      'utf8',
    ),
  ) as { annotationsDerived: { annotations: Annotations } };
  return { ...plain.annotations.annotations, ...derived.annotationsDerived.annotations };
}

/**
 * The form of a character that is drawn as an emoji. CLDR writes a
 * character that is a symbol by default without the selector that turns it
 * into an emoji (U+2764 for the heart); in text it has to be there.
 */
export function qualified(key: string): string | undefined {
  if (RGI.test(key)) return key;
  const withSelectors = Array.from(key)
    .map((character, index, all) =>
      TEXT_BY_DEFAULT.test(character) && all[index + 1] !== VARIATION_SELECTOR
        ? character + VARIATION_SELECTOR
        : character,
    )
    .join('');
  return RGI.test(withSelectors) ? withSelectors : undefined;
}

const clean = (value: string): string => value.replaceAll(/[\n|]/g, ' ').trim();

export interface EmojiFile {
  /** The CLDR release the list was made from. */
  cldr: string;
  /** Only in the base file: the emoji, separated by a space. */
  emoji?: string;
  /** A name per emoji, separated by a line feed, in the order of `emoji`. */
  names: string;
  /** The keywords per emoji: lines in the same order, keywords separated by `|`. */
  keywords: string;
}

/** The files to write: name of the file, content. */
export function emojiFiles(): Map<string, string> {
  const { version } = JSON.parse(
    readFileSync(path.join(packageDirectory('cldr-annotations-full'), 'package.json'), 'utf8'),
  ) as { version: string };
  const [base, ...others] = EMOJI_LOCALES;
  const english = read(base);

  const collator = new Intl.Collator('en-u-co-emoji');
  const listed = Object.keys(english)
    .filter((key) => !SKIN_TONE.test(key) && !HAIR.test(key))
    .map((key) => ({ key, emoji: qualified(key) }))
    .filter((entry): entry is { key: string; emoji: string } => entry.emoji !== undefined)
    .filter((entry) => (english[entry.key]?.tts?.[0] ?? '') !== '')
    .sort((a, b) => collator.compare(a.emoji, b.emoji) || (a.emoji < b.emoji ? -1 : 1));

  const lines = (annotations: Annotations, lower: boolean) => {
    const names: string[] = [];
    const keywords: string[] = [];
    for (const { key } of listed) {
      const annotation = annotations[key] ?? {};
      const fold = (value: string): string => (lower ? clean(value).toLowerCase() : clean(value));
      const name = fold(annotation.tts?.[0] ?? '');
      names.push(name);
      // A keyword that the name already contains adds nothing to a search.
      const extra = [...new Set((annotation.default ?? []).map(fold))].filter(
        (keyword) => keyword !== '' && !name.includes(keyword),
      );
      keywords.push(extra.join('|'));
    }
    return { names: names.join('\n'), keywords: keywords.join('\n') };
  };

  const files = new Map<string, string>();
  const write = (locale: string, file: EmojiFile): void => {
    files.set(`${EMOJI_DIRECTORY}/${locale}.json`, `${JSON.stringify(file, null, 2)}\n`);
  };
  write(base, {
    cldr: version,
    emoji: listed.map((entry) => entry.emoji).join(' '),
    ...lines(english, true),
  });
  for (const locale of others) write(locale, { cldr: version, ...lines(read(locale), true) });
  return files;
}

if (import.meta.main) {
  const files = emojiFiles();
  if (process.argv[2] === '--check') {
    const stale = [...files].filter(([name, content]) => {
      try {
        return readFileSync(path.join(WEB_ROOT, name), 'utf8') !== content;
      } catch {
        return true;
      }
    });
    if (stale.length > 0) {
      console.error(
        `emoji lists are stale: ${stale.map(([name]) => name).join(', ')}\nrun "node tools/emoji/build.ts"`,
      );
      process.exit(1);
    }
    console.log(`emoji lists: up to date (${files.size} files)`);
  } else {
    mkdirSync(path.join(WEB_ROOT, EMOJI_DIRECTORY), { recursive: true });
    for (const [name, content] of files) {
      writeFileSync(path.join(WEB_ROOT, name), content);
      const file = JSON.parse(content) as EmojiFile;
      console.log(
        `${name}: ${file.names.split('\n').length} emoji, ${Buffer.byteLength(content)} bytes`,
      );
    }
  }
}
