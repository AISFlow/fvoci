/**
 * The built-in templates, read from the one place they live: the server's
 * `crates/server/assets/templates/`, which embeds those files in its binary.
 * Each `NN-<key>.<language>.json` is one template in one language; the
 * pictures and documents its bodies show are in `files/`.
 */
import { readdirSync, readFileSync } from 'node:fs';
import path from 'node:path';

import type { DocNode } from '$lib/editor/schema.ts';
import type { SeedTemplate } from '$lib/state/memory/seed.ts';

export const TEMPLATES_FOLDER = path.resolve(
  import.meta.dirname,
  '../../../../crates/server/assets/templates',
);

export interface BuiltinCourseChapter {
  id: string;
  type: 'chapter';
  title: string;
  fields: { order: string };
}

export interface BuiltinCourseUnit {
  id: string;
  type: 'unit';
  parentId: string;
  title: string;
  fields: {
    order: string;
    amount?: number;
    amountUnit?: 'minutes' | 'pages' | 'problems';
    scheduledDate?: string;
  };
  body: DocNode | null;
}

export interface BuiltinCourseSchedule {
  startDate: string;
  targetDate: string;
  studyDays: number;
}

/** A template file: a board or course template in a language, with its files. */
export type BuiltinTemplate = Omit<SeedTemplate, 'files' | 'board' | 'views' | 'cards'> & {
  /** The tag of the language it is written in. */
  language: string;
  board: SeedTemplate['board'] & { kind?: 'board' | 'course' };
  views: SeedTemplate['views'];
  cards: SeedTemplate['cards'];
  schedule?: BuiltinCourseSchedule;
  chapters?: BuiltinCourseChapter[];
  units?: BuiltinCourseUnit[];
  /** The files its bodies show: the name in `files/`, and the name each is shown under. */
  files: { asset: string; name: string }[];
};

/** The template files, in the order of their names (the order the server stores them in). */
export function readTemplates(folder: string = TEMPLATES_FOLDER): BuiltinTemplate[] {
  return readdirSync(folder)
    .filter((name) => /^\d+-.+\.json$/.test(name))
    .sort()
    .map((name) => {
      const parsed = JSON.parse(readFileSync(path.join(folder, name), 'utf8')) as Partial<
        Omit<BuiltinTemplate, 'board'>
      > & {
        board: Omit<BuiltinTemplate['board'], 'cardProperties'> & {
          cardProperties?: BuiltinTemplate['board']['cardProperties'];
        };
      };
      return {
        ...parsed,
        board: {
          ...parsed.board,
          cardProperties: parsed.board.cardProperties ?? [],
        },
        views: parsed.views ?? [],
        cards: parsed.cards ?? [],
        chapters: parsed.chapters ?? [],
        units: parsed.units ?? [],
        files: parsed.files ?? [],
      } as BuiltinTemplate;
    });
}

/** The names of the files of `files/`. */
export function templateAssets(folder: string = TEMPLATES_FOLDER): string[] {
  return readdirSync(path.join(folder, 'files')).sort();
}

/** A card's or unit's body, with the titles of its template and block. */
export interface TemplateDocument {
  template: string;
  card: string;
  doc: DocNode;
}

/** Every body of a card or unit in the built-in templates, in every language. */
export function templateDocuments(): TemplateDocument[] {
  return readTemplates().flatMap((template) => [
    ...template.cards.flatMap((card) =>
      card.body ? [{ template: template.board.title, card: card.title, doc: card.body }] : [],
    ),
    ...(template.units ?? []).flatMap((unit) =>
      unit.body ? [{ template: template.board.title, card: unit.title, doc: unit.body }] : [],
    ),
  ]);
}
