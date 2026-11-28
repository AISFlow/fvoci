/**
 * The built-in templates (`crates/server/assets/templates/`): what has to
 * hold of the files, which are written by hand. Each is written in every
 * language and whole in it, their bodies pass the editor's validator, they
 * name no address outside the app (none in a body, only example addresses as
 * the value of a link or mail property), dates have the shape of the wire,
 * every reference inside a template leads somewhere, and between them they
 * show every kind of view, property and node there is.
 */
import { readdirSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { describe, expect, it } from 'vitest';

import { MARK_SPECS, NODE_SPECS, validateDoc, type DocNode } from '$lib/editor/schema.ts';
import { ADDABLE_VIEW_TYPES } from '$lib/features/view-header/view-fields.ts';
import { CALCULATION_IDS } from '$lib/logic/calculations.ts';
import { PROPERTY_BEHAVIORS } from '$lib/logic/properties.ts';
import { parseDateValue, parseRelationValue, type BoardView } from '$lib/logic/wire.ts';
import { orderViews } from '$lib/state/order.ts';

import { readTemplates, templateAssets, type BuiltinTemplate } from '../support/templates.ts';

const LANGUAGES = ['en', 'ko'];
const HANGUL = /[가-힣]/;

/** The values of the properties of the given kinds of a template's cards. */
function valuesOf(template: BuiltinTemplate, kinds: readonly string[]): string[] {
  const ids = template.board.cardProperties
    .filter((property) => kinds.includes(property.type))
    .map((property) => property.id);
  return template.cards.flatMap((card) =>
    ids.flatMap((id) => {
      const value = card.fields.properties[id];
      return typeof value === 'string' ? [value] : [];
    }),
  );
}

/**
 * Whether the value of a link property leads outside the app. Such a value
 * is opened as an address whatever it looks like, so everything but a path
 * inside the app does.
 */
function valueLeadsOutside(value: string): boolean {
  const address = value.trim();
  return address !== '' && !/^\/(?!\/)/.test(address);
}

/**
 * Whether a value is an address under the domain reserved for examples
 * (RFC 2606) and names no other site: the only kind of address that leads
 * outside and may stay in a template.
 */
function isExampleAddress(value: string): boolean {
  const host = /^https:\/\/(?:[a-z0-9-]+\.)*example\.com(?![^/?#])/i.exec(value)?.[0];
  return host !== undefined && !/:\/\/|\bwww\./i.test(value.slice(host.length));
}

/** One line per address that leads outside and may not; none when there is none. */
function addressProblems(templates: readonly BuiltinTemplate[]): string[] {
  const problems: string[] = [];
  // The values of link properties are judged one by one, then left out of the
  // search through everything else.
  const rest = templates.map((template) => {
    const linkIds = new Set(
      template.board.cardProperties.filter(({ type }) => type === 'url').map(({ id }) => id),
    );
    const cards = template.cards.map((card) => {
      const properties = Object.fromEntries(
        Object.entries(card.fields.properties).filter(([id, value]) => {
          if (!linkIds.has(id) || typeof value !== 'string') return true;
          if (valueLeadsOutside(value) && !isExampleAddress(value)) {
            problems.push(`${template.key} / ${card.title}: a link value leads outside`);
          }
          return false;
        }),
      );
      return { ...card, fields: { ...card.fields, properties } };
    });
    return { ...template, cards };
  });
  if (/https?:\/\/|\bwww\./i.test(JSON.stringify(rest))) {
    problems.push('an address that leads outside is left outside a link value');
  }
  return problems;
}

/** Every node of a body, the body itself first. */
function nodesOf(node: DocNode): DocNode[] {
  return [node, ...(node.content ?? []).flatMap(nodesOf)];
}

/** Every node of every body of the templates. */
function allNodes(templates: readonly BuiltinTemplate[]): DocNode[] {
  return templates.flatMap((template) => [
    ...template.cards.flatMap((card) => (card.body ? nodesOf(card.body) : [])),
    ...(template.units ?? []).flatMap((unit) => (unit.body ? nodesOf(unit.body) : [])),
  ]);
}

/** Every text a person reads in a template, each with where it is. */
function texts(template: BuiltinTemplate): [where: string, text: string][] {
  return [
    ['the title', template.board.title],
    ['the description', template.board.description],
    ...template.board.cardProperties.flatMap((property): [string, string][] => [
      [`the property ${property.id}`, property.name],
      ...property.options.map((option): [string, string] => [
        `the option ${option.id}`,
        option.label,
      ]),
    ]),
    ...template.views.map((view): [string, string] => [`the view ${view.id}`, view.title]),
    ...template.cards.map((card): [string, string] => [`the card ${card.id}`, card.title]),
    ...(template.chapters ?? []).map((chapter): [string, string] => [
      `the chapter ${chapter.id}`,
      chapter.title,
    ]),
    ...(template.units ?? []).map((unit): [string, string] => [`the unit ${unit.id}`, unit.title]),
    ...template.files.map((file): [string, string] => [`the file ${file.asset}`, file.name]),
  ];
}

const templates = readTemplates();
const byLanguage = (language: string): BuiltinTemplate[] =>
  templates.filter((template) => template.language === language);

describe('the built-in templates', () => {
  const everything = JSON.stringify(templates);

  it('are nine in each language, with personal and shared welcome boards and the course template', () => {
    for (const language of LANGUAGES) {
      const written = byLanguage(language);
      expect(written.map((template) => template.key)).toEqual([
        'welcome-personal',
        'welcome',
        'project-schedule',
        'issue-tracker',
        'meeting-notes',
        'content-calendar',
        'customer-contacts',
        'study-notes',
        'course-study-plan',
      ]);
      expect(written.filter((template) => template.onboarding).map((t) => t.key)).toEqual([
        'welcome',
      ]);
    }
    expect(templates).toHaveLength(9 * LANGUAGES.length);
  });

  it('hold 50 cards, 34 views, 2 chapters, and 5 units in each language, each body accepted by the validator', () => {
    for (const language of LANGUAGES) {
      const cards = byLanguage(language).flatMap((template) => template.cards);
      expect(cards).toHaveLength(50);
      for (const card of cards) expect(validateDoc(card.body), card.title).toEqual([]);
      expect(byLanguage(language).flatMap((template) => template.views)).toHaveLength(34);
      const chapters = byLanguage(language).flatMap((template) => template.chapters ?? []);
      const units = byLanguage(language).flatMap((template) => template.units ?? []);
      expect(chapters).toHaveLength(2);
      expect(units).toHaveLength(5);
      for (const unit of units) expect(validateDoc(unit.body), unit.title).toEqual([]);
    }
  });

  it('are the same template in each language: the same properties, views, cards, chapters, and units', () => {
    const shape = (template: BuiltinTemplate): unknown => ({
      board: [template.board.id, template.board.kind ?? 'board'],
      properties: template.board.cardProperties.map((property) => [
        property.id,
        property.type,
        property.options.map((option) => [option.id, option.color]),
      ]),
      views: template.views.map((view) => [view.id, view.fields]),
      cards: template.cards.map((card) => [
        card.id,
        card.fields.icon,
        // The words are cut into texts where a language marks them.
        card.body
          ? nodesOf(card.body)
              .map((node) => node.type)
              .filter((type) => type !== 'text')
          : null,
      ]),
      schedule: template.schedule ?? null,
      chapters: (template.chapters ?? []).map((chapter) => [chapter.id, chapter.fields]),
      units: (template.units ?? []).map((unit) => [
        unit.id,
        unit.parentId,
        unit.fields,
        unit.body
          ? nodesOf(unit.body)
              .map((node) => node.type)
              .filter((type) => type !== 'text')
          : null,
      ]),
      files: template.files.length,
    });
    for (const template of byLanguage('en')) {
      const korean = byLanguage('ko').find((other) => other.key === template.key);
      if (!korean) throw new Error(`${template.key} is not written in Korean`);
      expect(shape(korean), template.key).toEqual(shape(template));
    }
  });

  it('are mirrored in the web memory seed directory', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const serverDir = resolve(here, '../../../../crates/server/assets/templates');
    const webSeedDir = resolve(here, '../../src/lib/state/memory/seed');
    const jsonFiles = (dir: string): string[] =>
      readdirSync(dir)
        .filter((name) => /^[0-9].*\.json$/.test(name))
        .sort();
    const expected = jsonFiles(serverDir);
    expect(jsonFiles(webSeedDir)).toEqual(expected);
    for (const name of expected) {
      expect(JSON.parse(readFileSync(resolve(webSeedDir, name), 'utf8')), name).toEqual(
        JSON.parse(readFileSync(resolve(serverDir, name), 'utf8')),
      );
    }
  });

  it('are whole in their language: Korean in every text of one, none in the other', () => {
    for (const template of byLanguage('ko')) {
      for (const [where, text] of texts(template)) {
        expect(HANGUL.test(text), `${template.key}, ${where}: ${text}`).toBe(true);
      }
      for (const card of template.cards) {
        const body = allNodes([{ ...template, cards: [card], units: [] }])
          .map((node) => node.text ?? '')
          .join('');
        expect(HANGUL.test(body), `${template.key}, the body of ${card.title}`).toBe(true);
      }
      for (const unit of template.units ?? []) {
        const body = allNodes([{ ...template, cards: [], units: [unit] }])
          .map((node) => node.text ?? '')
          .join('');
        expect(HANGUL.test(body), `${template.key}, the body of ${unit.title}`).toBe(true);
      }
    }
    expect(HANGUL.test(JSON.stringify(byLanguage('en')))).toBe(false);
  });

  it('open on the view meant to be seen first: the tabs are in the order of their titles', () => {
    // A board opens on its first tab, and the tabs are sorted by title in the
    // reader's language: the titles of a template's views decide.
    const first: Record<string, string> = {
      'welcome-personal': 'board',
      welcome: 'board',
      'project-schedule': 'gantt',
      'issue-tracker': 'board',
      'meeting-notes': 'table',
      'content-calendar': 'calendar',
      'customer-contacts': 'table',
      'study-notes': 'board',
    };
    for (const template of templates) {
      if (template.board.kind === 'course') continue;
      const views = template.views.map(
        (view) => ({ ...view, createdAt: 0 }) as unknown as BoardView,
      );
      const [opened] = orderViews(views, template.language);
      expect(opened?.fields.viewType, `${template.key} (${template.language})`).toBe(
        first[template.key],
      );
    }
  });

  it('name no address outside, but for example addresses in link and mail values', () => {
    expect(addressProblems(templates)).toEqual([]);
    const links = templates.flatMap((template) => valuesOf(template, ['url']));
    expect(links.length).toBeGreaterThan(0);
    for (const value of links) expect(isExampleAddress(value), value).toBe(true);
    const mail = templates.flatMap((template) => valuesOf(template, ['email']));
    expect(mail.length).toBeGreaterThan(0);
    for (const value of mail) expect(value).toMatch(/^[a-z0-9.]+@example\.com$/);
    const bodies = JSON.stringify(templates.flatMap((t) => t.cards.map((card) => card.body)));
    expect(bodies).not.toMatch(/https?:\/\/|\bwww\.|@[a-z0-9-]+\.[a-z]/i);
  });

  it('would be found out with an address that is not an example, or one outside a link value', () => {
    const copy = (): BuiltinTemplate[] => JSON.parse(everything) as BuiltinTemplate[];

    const real = copy();
    const template = real.find((one) => valuesOf(one, ['url']).length > 0);
    const linkId = template?.board.cardProperties.find((property) => property.type === 'url')?.id;
    const card = template?.cards.find(
      (one) => typeof one.fields.properties[linkId ?? ''] === 'string',
    );
    if (!card || linkId === undefined) throw new Error('no card with a link value');
    card.fields.properties[linkId] = 'example.org';
    expect(addressProblems(real)).toHaveLength(1);
    card.fields.properties[linkId] = 'https://www.example.com.evil.test/';
    expect(addressProblems(real)).toHaveLength(1);

    const elsewhere = copy();
    const first = elsewhere[0];
    if (!first) throw new Error('no template');
    first.board.description = 'See https://example.com for more.';
    expect(addressProblems(elsewhere)).toEqual([
      'an address that leads outside is left outside a link value',
    ]);
  });

  it('write date values the way the wire has them', () => {
    let dates = 0;
    for (const template of templates) {
      const dateIds = template.board.cardProperties
        .filter((property) => property.type === 'date')
        .map((property) => property.id);
      for (const card of template.cards) {
        for (const id of dateIds) {
          const value = card.fields.properties[id];
          if (value === undefined) continue;
          dates += 1;
          const { from, to } = parseDateValue(value);
          expect(from?.date, card.title).toMatch(/^\d{4}-\d{2}-\d{2}$/);
          if (to) expect((to.date ?? '') >= (from?.date ?? ''), card.title).toBe(true);
        }
      }
    }
    expect(dates).toBeGreaterThan(0);
  });

  it('keep every reference inside a template whole', () => {
    for (const template of templates) {
      const where = `${template.key} (${template.language})`;
      const cardIds = new Set(template.cards.map((card) => card.id));
      const cardTemplates = new Set(
        template.cards.filter((card) => card.fields.isTemplate).map((card) => card.id),
      );
      const properties = new Map(
        template.board.cardProperties.map((property) => [property.id, property]),
      );
      const optionsOf = (id: unknown): string[] =>
        (properties.get(String(id))?.options ?? []).map((option) => option.id);
      for (const view of template.views) {
        const fields = view.fields as Record<string, unknown> & {
          cardOrder: string[];
          shownProperties: string[];
          hiddenGroups: string[];
          visibleGroups: string[];
          collapsedGroups: string[];
          sort: { propertyId: string }[];
          filter: { filters: { propertyId: string }[] };
          columnCalculations: Record<string, string>;
          columnWidths: Record<string, number>;
        };
        // The order names every card that is no card template, once.
        expect([...fields.cardOrder].sort(), `${where}, ${view.title}`).toEqual(
          [...cardIds].filter((id) => !cardTemplates.has(id)).sort(),
        );
        const named = [
          ...fields.shownProperties,
          ...fields.sort.map((sort) => sort.propertyId),
          ...fields.filter.filters.map((filter) => filter.propertyId),
          ...Object.keys(fields.columnCalculations),
          ...Object.keys(fields.columnWidths),
          ...['groupBy', 'dateProperty', 'dependencyPropertyId'].flatMap((member) =>
            member in fields ? [String(fields[member])] : [],
          ),
        ].filter((id) => id !== '__title');
        for (const id of named) expect(properties.has(id), `${where}, ${view.title}`).toBe(true);
        const options = optionsOf(fields.groupBy);
        for (const id of [
          ...fields.visibleGroups,
          ...fields.hiddenGroups,
          ...fields.collapsedGroups,
        ]) {
          expect(id === '' || options.includes(id), `${where}, ${view.title}`).toBe(true);
        }
        const starting = String(fields.defaultTemplateId);
        expect(starting === '' || cardTemplates.has(starting), `${where}, ${view.title}`).toBe(
          true,
        );
      }
      for (const card of template.cards) {
        for (const [id, value] of Object.entries(card.fields.properties)) {
          const property = properties.get(id);
          expect(property, `${where} / ${card.title}: a value of no property`).toBeDefined();
          const chosen =
            property?.type === 'select' || property?.type === 'multiSelect'
              ? [value].flat().filter((option) => typeof option === 'string')
              : [];
          for (const option of chosen) {
            expect(optionsOf(id).includes(option), `${where} / ${card.title}`).toBe(true);
          }
          if (property?.type === 'relation') {
            expect(Array.isArray(value), `${where} / ${card.title}`).toBe(true);
            const links = parseRelationValue(value);
            expect(value, `${where} / ${card.title}`).toEqual(links);
            for (const link of links) {
              expect(cardIds.has(link.card), `${where} / ${card.title}`).toBe(true);
            }
          }
        }
        for (const node of card.body ? nodesOf(card.body) : []) {
          if (node.type === 'reference') {
            expect(cardIds.has(String(node.attrs?.id)), `${where} / ${card.title}`).toBe(true);
          }
        }
      }
      const chapterIds = new Set((template.chapters ?? []).map((chapter) => chapter.id));
      const unitIds = new Set((template.units ?? []).map((unit) => unit.id));
      for (const unit of template.units ?? []) {
        if (unit.parentId !== '') {
          expect(chapterIds.has(unit.parentId), `${where} / ${unit.title}`).toBe(true);
        }
        for (const node of unit.body ? nodesOf(unit.body) : []) {
          if (node.type === 'reference') {
            expect(unitIds.has(String(node.attrs?.id)), `${where} / ${unit.title}`).toBe(true);
          }
        }
      }
    }
  });

  it('show the files they name, each a small file of `files/`, and name the files they show', () => {
    const assets = templateAssets();
    const used = new Set<string>();
    for (const template of templates) {
      const named = template.files.map((file) => file.asset).sort();
      const shown = allNodes([template])
        .filter((node) => node.type === 'image' || node.type === 'file')
        .map((node) => String(node.attrs?.fileId));
      expect([...new Set(shown)].sort(), `${template.key} (${template.language})`).toEqual(named);
      for (const asset of named) {
        expect(assets, asset).toContain(asset);
        used.add(asset);
      }
    }
    expect([...used].sort()).toEqual(assets);
  });
});

/**
 * What no template shows, each with why. Every other kind the code knows has
 * to be in a template of each language: a kind that is added to the code
 * fails these tests until a template shows it or it is listed here.
 */
const NOT_SHOWN: Record<'views' | 'properties' | 'nodes' | 'marks', Record<string, string>> = {
  views: {},
  properties: {
    file: 'has no editor and no value of its own yet: it is shown as an unknown kind',
    // A person property is there, without a value: see the test of the people.
  },
  nodes: {
    mention: 'names a user, and a template is copied to people it knows nothing of',
    boardView: 'shows a view by its identifier, and a template is written before any view is made',
  },
  marks: {
    link: 'a link leads to an address, and the templates name none',
  },
};

/** Fails unless each of `kinds` is either among `shown` or listed in `left`, and not both. */
function expectCovered(
  kinds: readonly string[],
  shown: ReadonlySet<string>,
  left: Record<string, string>,
  language: string,
): void {
  for (const kind of kinds) {
    expect(
      shown.has(kind) !== kind in left,
      `${kind} (${language}) is shown by a template, or listed as not shown, and not both`,
    ).toBe(true);
  }
  for (const kind of Object.keys(left)) {
    expect(kinds, `${kind} is listed as not shown and is no kind`).toContain(kind);
  }
}

describe.each(LANGUAGES)('what the built-in templates show between them, in %s', (language) => {
  const written = byLanguage(language);
  const views = written.flatMap((template) => template.views.map((view) => view.fields));
  const nodes = allNodes(written);

  it('is every kind of view', () => {
    const shown = new Set(views.map((fields) => String(fields.viewType)));
    expectCovered(ADDABLE_VIEW_TYPES, shown, NOT_SHOWN.views, language);
  });

  it('is every kind of property', () => {
    const shown = new Set(
      written.flatMap((template) => template.board.cardProperties.map((property) => property.type)),
    );
    expectCovered(Object.keys(PROPERTY_BEHAVIORS), shown, NOT_SHOWN.properties, language);
  });

  it('is every node and mark of a body', () => {
    expectCovered(
      Object.keys(NODE_SPECS),
      new Set(nodes.map((node) => node.type)),
      NOT_SHOWN.nodes,
      language,
    );
    expectCovered(
      Object.keys(MARK_SPECS),
      new Set(nodes.flatMap((node) => (node.marks ?? []).map((mark) => mark.type))),
      NOT_SHOWN.marks,
      language,
    );
    const code = nodes.filter((node) => node.type === 'codeBlock');
    expect(code.some((node) => node.attrs?.language === 'mermaid')).toBe(true);
    expect(code.some((node) => node.attrs?.language !== 'mermaid')).toBe(true);
    const levels = nodes.filter((n) => n.type === 'heading').map((n) => Number(n.attrs?.level));
    expect([...new Set(levels)].sort()).toEqual([1, 2, 3]);
  });

  it('is what a view does with its cards: groups, filters, sorts, calculates, hides', () => {
    const some = (holds: (fields: Record<string, unknown>) => boolean): boolean =>
      views.some(holds);
    const list = (value: unknown): unknown[] => (Array.isArray(value) ? value : []);
    expect(some((fields) => typeof fields.groupBy === 'string')).toBe(true);
    expect(some((f) => list((f.filter as { filters: unknown[] }).filters).length > 0)).toBe(true);
    expect(some((fields) => list(fields.sort).length > 0)).toBe(true);
    expect(some((fields) => list(fields.hiddenGroups).some((id) => id !== ''))).toBe(true);
    expect(some((fields) => list(fields.collapsedGroups).length > 0)).toBe(true);
    expect(some((fields) => Object.keys(fields.columnWidths as object).length > 0)).toBe(true);
    expect(some((fields) => typeof fields.dateProperty === 'string')).toBe(true);
    // A gantt view draws what a card comes after from a card relation.
    expect(some((f) => f.viewType === 'gantt' && typeof f.dependencyPropertyId === 'string')).toBe(
      true,
    );
    // A view starts a new card from a card template of its board.
    expect(some((fields) => fields.defaultTemplateId !== '')).toBe(true);
    const calculations = views.flatMap((fields) =>
      Object.values(fields.columnCalculations as Record<string, string>),
    );
    // A column is calculated in the ways the code knows. Which of them the templates
    // show is theirs to choose: a foot for each of the twenty would be there for the
    // test, not for a reader.
    expect(calculations.length).toBeGreaterThan(0);
    for (const calculation of calculations) expect(CALCULATION_IDS).toContain(calculation);
  });

  it('is nobody: the properties for people are there, and theirs to fill', () => {
    for (const template of written) {
      const people = template.board.cardProperties
        .filter((property) => ['person', 'multiPerson'].includes(property.type))
        .map((property) => property.id);
      for (const card of template.cards) {
        for (const id of people) expect(card.fields.properties[id], card.title).toBeUndefined();
      }
    }
  });
});
