/**
 * Search, grouping, the property table and CSV rows. A case that pins a
 * numbered rule names it (Q3, Q4, Q12, Q13 in docs/notes.md).
 */
import { describe, expect, it, vi } from 'vitest';

import { mergeBlocks } from '$lib/logic/block-tree.ts';
import { CSV_BOM, csvCell, csvText, tableRows } from '$lib/logic/csv.ts';
import { calendarDate } from '$lib/logic/dates.ts';
import { diagnostics } from '$lib/logic/diagnostics.ts';
import {
  filterConditionDisplayString,
  filterConditionValidOrDefault,
  type FilterConditionMessages,
} from '$lib/logic/filter-conditions.ts';
import { getVisibleAndHiddenGroups } from '$lib/logic/group.ts';
import {
  PROPERTY_BEHAVIORS,
  propertyBehavior,
  SELECTABLE_PROPERTY_TYPES,
} from '$lib/logic/properties.ts';
import { searchFilterCards } from '$lib/logic/search.ts';
import {
  createBlock,
  createCard,
  setCardProperty,
  type Card,
  type PropertyTemplate,
  type PropertyValue,
} from '$lib/logic/wire.ts';

import { storedDate } from './support/factory.ts';

import { at } from '../support/zones.ts';

function card(id: string, title: string, properties: Record<string, PropertyValue> = {}): Card {
  const created = createCard();
  created.id = id;
  created.title = title;
  for (const [propertyId, value] of Object.entries(properties)) {
    setCardProperty(created, propertyId, value);
  }
  return created;
}

const ids = (cards: readonly Card[]): string => cards.map((item) => item.id).join(' ');

const status: PropertyTemplate = {
  id: 'status',
  name: 'Status',
  type: 'select',
  options: [
    { id: 'todo', label: 'To do', color: 'default' },
    { id: 'doing', label: 'In progress', color: 'default' },
    { id: 'done', label: 'Done', color: 'default' },
  ],
};
const tags: PropertyTemplate = {
  id: 'tags',
  name: 'Tags',
  type: 'multiSelect',
  options: [
    { id: 't1', label: 'Urgent', color: 'default' },
    { id: 't2', label: 'Backend work', color: 'default' },
  ],
};
const notes: PropertyTemplate = { id: 'notes', name: 'Notes', type: 'text', options: [] };
const due: PropertyTemplate = { id: 'due', name: 'Due', type: 'date', options: [] };
const amount: PropertyTemplate = { id: 'amount', name: 'Amount', type: 'number', options: [] };
const owner: PropertyTemplate = { id: 'owner', name: 'Owner', type: 'person', options: [] };
const context = { locale: 'en-US', timeZone: 'Asia/Seoul', now: Date.UTC(2026, 0, 15) };

describe('search', () => {
  const templates = [status, tags, notes, due];
  const cards = [
    card('title', 'Fix the Login page'),
    card('select', 'a', { status: 'doing' }),
    card('text', 'b', { notes: 'ask about LOGIN flow' }),
    card('multi', 'c', { tags: ['t1', 't2'] }),
    card('date', 'd', { due: { from: '2026-01-01' } }),
  ];

  it('returns every card for an empty text, as a new list', () => {
    const result = searchFilterCards(cards, templates, '');
    expect(result).toEqual(cards);
    expect(result).not.toBe(cards);
  });

  it('finds the text in titles and text values, ignoring case', () => {
    expect(ids(searchFilterCards(cards, templates, 'login'))).toBe('title text');
  });

  it('finds a select by a part of the label of its option', () => {
    expect(ids(searchFilterCards(cards, templates, 'progress'))).toBe('select');
  });

  it('does not search date values', () => {
    expect(ids(searchFilterCards(cards, templates, '1767'))).toBe('');
  });

  it('skips a property whose stored value is null', () => {
    const odd = card('odd', 'x');
    setCardProperty(odd, 'notes', null as unknown as string);
    expect(ids(searchFilterCards([odd], templates, 'anything'))).toBe('');
  });

  it('finds a multi-select by a part of a label, like every other kind (Q3)', () => {
    expect(ids(searchFilterCards(cards, templates, 'urgent'))).toBe('multi');
    expect(ids(searchFilterCards(cards, templates, 'BACKEND'))).toBe('multi');
    expect(ids(searchFilterCards(cards, templates, 'end wo'))).toBe('multi');
  });

  it('does not look at case or at the width of characters', () => {
    const wide = [card('wide', 'ＡＢＣ 상사'), card('upper', 'x', { notes: 'DEADLINE' })];
    expect(ids(searchFilterCards(wide, templates, 'abc'))).toBe('wide');
    expect(ids(searchFilterCards(wide, templates, 'dead'))).toBe('upper');
  });
});

describe('groups of a board view', () => {
  const noValue = (name: string): string => `No ${name}`;
  const cards = [
    card('a', 'a', { status: 'todo' }),
    card('b', 'b', { status: 'done' }),
    card('c', 'c'),
    card('d', 'd', { status: 'deleted-option' }),
  ];

  it('puts the cards without a value first, then the options in the order of the view', () => {
    const groups = getVisibleAndHiddenGroups(cards, ['done', 'todo'], [], status, noValue);
    expect(groups.visible.map((group) => [group.option.label, ids(group.cards)])).toEqual([
      ['No Status', 'c d'],
      ['Done', 'b'],
      ['To do', 'a'],
      // An option the view does not mention is shown after the others.
      ['In progress', ''],
    ]);
    expect(groups.hidden).toEqual([]);
  });

  it('keeps hidden options apart and honours a placed empty group', () => {
    const groups = getVisibleAndHiddenGroups(cards, ['todo', ''], ['done'], status, noValue);
    expect(groups.visible.map((group) => group.option.id)).toEqual(['todo', '', 'doing']);
    expect(groups.hidden.map((group) => [group.option.id, ids(group.cards)])).toEqual([
      ['done', 'b'],
    ]);
  });

  it('ignores the identifier of an option that was deleted', () => {
    const groups = getVisibleAndHiddenGroups(cards, ['gone', 'todo'], [], status, noValue);
    expect(groups.visible.map((group) => group.option.id)).toEqual(['', 'todo', 'doing', 'done']);
  });

  it('groups by person in the order people first occur', () => {
    const people = [
      card('1', '1', { owner: 'ann' }),
      card('2', '2', { owner: 'bob' }),
      card('3', '3', { owner: 'ann' }),
    ];
    const groups = getVisibleAndHiddenGroups(people, [], ['bob'], owner, noValue);
    expect(groups.visible.map((group) => [group.option.id, ids(group.cards)])).toEqual([
      ['ann', '1 3'],
    ]);
    expect(groups.hidden.map((group) => [group.option.id, ids(group.cards)])).toEqual([
      ['bob', '2'],
    ]);
  });

  it('puts cards without a person in a "no value" group that comes first (Q4)', () => {
    const people = [card('1', '1', { owner: 'ann' }), card('2', '2'), card('3', '3')];
    const groups = getVisibleAndHiddenGroups(people, [], [], owner, noValue);
    expect(groups.visible.map((group) => [group.option, ids(group.cards)])).toEqual([
      [{ id: '', label: 'No Owner', color: 'default' }, '2 3'],
      [{ id: 'ann', label: 'ann', color: 'default' }, '1'],
    ]);
  });

  it('has no "no value" group when every card has a person, and hides it like any other', () => {
    const everyone = [card('1', '1', { owner: 'ann' })];
    expect(
      getVisibleAndHiddenGroups(everyone, [], [], owner, noValue).visible.map(
        (group) => group.option.id,
      ),
    ).toEqual(['ann']);

    const groups = getVisibleAndHiddenGroups(
      [card('1', '1'), ...everyone],
      [],
      [''],
      owner,
      noValue,
    );
    expect(groups.visible.map((group) => group.option.id)).toEqual(['ann']);
    expect(groups.hidden.map((group) => group.option.id)).toEqual(['']);
  });

  it('groups by who created a card', () => {
    const by: PropertyTemplate = { id: 'by', name: 'Created by', type: 'createdBy', options: [] };
    const first = card('1', '1');
    first.createdBy = 'ann';
    const groups = getVisibleAndHiddenGroups([first, card('2', '2')], [], [], by, noValue);
    expect(groups.visible.map((group) => [group.option.label, ids(group.cards)])).toEqual([
      ['No Created by', '2'],
      ['ann', '1'],
    ]);
  });
});

describe('property behaviour', () => {
  it('knows every kind a person can choose, and the one that is not offered', () => {
    expect(Object.keys(PROPERTY_BEHAVIORS).sort()).toEqual(
      [...SELECTABLE_PROPERTY_TYPES, 'file'].sort(),
    );
    expect(propertyBehavior('something-new')).toBe(PROPERTY_BEHAVIORS.file);
  });

  it('says which kinds can group, filter and be edited', () => {
    const where = (test: (name: string) => boolean): string[] =>
      Object.keys(PROPERTY_BEHAVIORS).filter(test).sort();
    expect(where((name) => propertyBehavior(name).canGroup)).toEqual([
      'createdBy',
      'person',
      'select',
      'updatedBy',
    ]);
    expect(where((name) => !propertyBehavior(name).canFilter)).toEqual(['file', 'number']);
    expect(where((name) => propertyBehavior(name).isReadOnly)).toEqual([
      'createdBy',
      'createdTime',
      'updatedBy',
      'updatedTime',
    ]);
    expect(PROPERTY_BEHAVIORS.checkbox.filterValueType).toBe('boolean');
    expect(PROPERTY_BEHAVIORS.multiPerson.filterValueType).toBe('person');
    expect(PROPERTY_BEHAVIORS.createdTime.filterValueType).toBe('date');
  });

  it('offers number calculations for numbers and time calculations for the times of a card', () => {
    expect(PROPERTY_BEHAVIORS.number.calculationOptions).toContain('median');
    expect(PROPERTY_BEHAVIORS.text.calculationOptions).not.toContain('sum');
    expect(PROPERTY_BEHAVIORS.createdTime.calculationOptions).toContain('dateRange');
    // The date property itself offers no earliest and latest: the table adds them.
    expect(PROPERTY_BEHAVIORS.date.calculationOptions).not.toContain('earliest');
  });

  it('shows a select by the label of its option', () => {
    const report = vi.spyOn(diagnostics, 'report').mockImplementation(() => undefined);
    const { displayValue } = PROPERTY_BEHAVIORS.select;
    expect(displayValue('done', card('x', 'x'), status, context)).toBe('Done');
    expect(displayValue(undefined, card('x', 'x'), status, context)).toBe('');
    expect(displayValue('gone', card('x', 'x'), status, context)).toBe('(Unknown)');
    expect(
      displayValue('gone', card('x', 'x'), status, { ...context, unknownOption: '(없음)' }),
    ).toBe('(없음)');
    expect(report).toHaveBeenCalledTimes(2);
    report.mockRestore();
  });

  it('shows a multi-select as the labels of its options, in the order of the board', () => {
    const { displayValue } = PROPERTY_BEHAVIORS.multiSelect;
    expect(displayValue(['t2', 't1'], card('x', 'x'), tags, context)).toEqual([
      'Urgent',
      'Backend work',
    ]);
    expect(displayValue([], card('x', 'x'), tags, context)).toBe('');
  });

  it('shows a date and a range of dates the same in every time zone', () => {
    const { displayValue } = PROPERTY_BEHAVIORS.date;
    const day = (date: number) => ({ date: calendarDate(2026, 3, date) });
    const single = storedDate({ from: day(5) });
    const range = storedDate({ from: day(5), to: day(9) });
    for (const timeZone of ['UTC', 'Asia/Seoul', 'Pacific/Kiritimati', 'Pacific/Pago_Pago']) {
      const zoned = { ...context, timeZone };
      expect(displayValue(single, card('x', 'x'), due, zoned)).toBe('March 05');
      expect(displayValue(range, card('x', 'x'), due, zoned)).toBe('March 05 -> March 09');
    }
    const lastYear = storedDate({ from: { date: calendarDate(2025, 12, 31) } });
    expect(displayValue(lastYear, card('x', 'x'), due, context)).toBe('December 31, 2025');
    expect(displayValue('not a date', card('x', 'x'), due, context)).toBe('');
    expect(displayValue(undefined, card('x', 'x'), due, context)).toBe('');
  });

  it('shows a date with a time as the moment it is', () => {
    const { displayValue } = PROPERTY_BEHAVIORS.date;
    const timed = storedDate({ from: { instant: at('Asia/Seoul', 2026, 3, 5, 15, 20) } });
    expect(displayValue(timed, card('x', 'x'), due, context)).toBe('March 05 at 3:20 PM');
  });

  it('shows the times of the card itself', () => {
    const created = card('x', 'x');
    created.createdAt = at('Asia/Seoul', 2026, 3, 5, 15, 20);
    expect(PROPERTY_BEHAVIORS.createdTime.displayValue(undefined, created, due, context)).toBe(
      'March 05 at 3:20 PM',
    );
  });

  it('says which days a card occupies in a calendar', () => {
    const day = (date: number) => ({ date: calendarDate(2026, 3, date) });
    const days = PROPERTY_BEHAVIORS.date.days;
    expect(days?.(storedDate({ from: day(5), to: day(9) }), card('x', 'x'), 'UTC')).toEqual({
      first: '2026-03-05',
      last: '2026-03-09',
    });
    expect(days?.(undefined, card('x', 'x'), 'UTC')).toBeUndefined();

    // A card made at 18:00 UTC on the 6th is on the 7th for a reader in Seoul.
    const created = card('x', 'x');
    created.createdAt = Date.UTC(2023, 11, 6, 18, 0);
    expect(PROPERTY_BEHAVIORS.createdTime.days?.(undefined, created, 'Asia/Seoul')?.first).toBe(
      '2023-12-07',
    );
    expect(PROPERTY_BEHAVIORS.createdTime.days?.(undefined, created, 'UTC')?.first).toBe(
      '2023-12-06',
    );
    expect(PROPERTY_BEHAVIORS.text.days).toBeUndefined();
  });
});

describe('CSV rows', () => {
  const templates = [status, tags, notes, amount, due];
  const view = {
    viewType: 'table' as const,
    shownProperties: ['notes', 'status', 'tags', 'amount'],
    dateProperty: undefined,
  };
  const first = card('1', 'Say "hi" #1', {
    status: 'done',
    tags: ['t1', 't2'],
    notes: 'a, b',
    amount: '012.50',
  });
  /** The lines of a file, its byte order mark taken off. */
  const lines = (text: string): string[] => text.slice(CSV_BOM.length).split('\r\n');

  it('writes a header and a row per card, properties in the order of the board', () => {
    const rows = tableRows([first, card('2', 'Empty')], templates, view, 'Name', context);
    expect(rows).toEqual([
      ['Name', 'Status', 'Tags', 'Notes', 'Amount'],
      ['Say "hi" #1', 'Done', 'Urgent, Backend work', 'a, b', 12.5],
      ['Empty', '', '', '', ''],
    ]);
    expect(lines(csvText(rows))).toEqual([
      '"Name","Status","Tags","Notes","Amount"',
      // A `#` stays what it is; a comma or a quote stays in its cell.
      '"Say ""hi"" #1","Done","Urgent, Backend work","a, b",12.5',
      '"Empty",,,,',
      '',
    ]);
  });

  it('starts the file with the mark that tells a spreadsheet it is UTF-8', () => {
    const text = csvText([['이름'], ['한글 제목']]);
    expect(text.startsWith('\uFEFF')).toBe(true);
    expect(text.indexOf(CSV_BOM, 1)).toBe(-1);
    expect(lines(text)).toEqual(['"이름"', '"한글 제목"', '']);
  });

  it('quotes a text with a comma, a quote or a line break as RFC 4180 has it', () => {
    expect(csvCell('a, b')).toBe('"a, b"');
    expect(csvCell('say "hi"')).toBe('"say ""hi"""');
    expect(csvCell('one\r\ntwo\nthree')).toBe('"one\r\ntwo\nthree"');
    expect(csvCell('')).toBe('');
    expect(csvCell(-3.5)).toBe('-3.5');
    expect(csvCell(Number.NaN)).toBe('');
  });

  it('writes a text a spreadsheet would run as a formula after an apostrophe', () => {
    for (const formula of ['=HYPERLINK("x")', '+1+2', '-2+3', '@SUM(A1)', '\tTab', '\rReturn']) {
      expect(csvCell(formula)).toBe(`"'${formula.replaceAll('"', '""')}"`);
    }
    // A plain number is no formula, signed or not, and a sign inside a text is nothing.
    expect(csvCell('-42')).toBe('"-42"');
    expect(csvCell('+1.5e3')).toBe('"+1.5e3"');
    expect(csvCell('a=b')).toBe('"a=b"');
    // After what a spreadsheet passes by, and in the full width of East Asian text.
    const hidden = [' =1+1', '\n=1+1', '\u00a0=1+1', '\u3000=1+1', '\u200b=1+1', '\ufeff=1+1'];
    for (const formula of [...hidden, '\u202e=1', '＝1+1', '＋1+1', '－1+1', '＠1']) {
      expect(csvCell(formula), JSON.stringify(formula)).toBe(`"'${formula}"`);
    }
    // A number so written is still a number.
    expect(csvCell(' -5')).toBe('" -5"');
    expect(csvCell('－5')).toBe('"－5"');
    const titled = tableRows([card('1', '=1+2')], [], view, 'Name', context);
    expect(lines(csvText(titled))[1]).toBe(`"'=1+2"`);
  });

  it('names people by the names the reader knows them by, never by their identifiers', () => {
    const helpers: PropertyTemplate = {
      id: 'helpers',
      name: 'Helpers',
      type: 'multiPerson',
      options: [],
    };
    const by: PropertyTemplate = { id: 'by', name: 'By', type: 'createdBy', options: [] };
    const changed: PropertyTemplate = { id: 'last', name: 'Last', type: 'updatedBy', options: [] };
    const made = card('1', 't', { owner: 'user-1', helpers: ['user-2', 'user-3'] });
    made.createdBy = 'user-1';
    made.modifiedBy = 'user-3';
    const names: Record<string, string> = { 'user-1': 'Ann (@ann)', 'user-2': 'bo' };
    const people = { ...context, personName: (id: string) => names[id] ?? 'Unknown user' };
    const shown = { ...view, shownProperties: ['owner', 'helpers', 'by', 'last'] };
    const rows = tableRows([made], [owner, helpers, by, changed], shown, 'Name', people);
    expect(rows[1]).toEqual(['t', 'Ann (@ann)', 'bo, Unknown user', 'Ann (@ann)', 'Unknown user']);
  });

  it('writes a box as a truth value, ticked or not', () => {
    const done: PropertyTemplate = { id: 'ok', name: 'OK', type: 'checkbox', options: [] };
    const shown = { ...view, shownProperties: ['ok'] };
    const rows = tableRows(
      [card('1', 'a', { ok: 'true' }), card('2', 'b')],
      [done],
      shown,
      'N',
      context,
    );
    expect(rows.slice(1)).toEqual([
      ['a', 'TRUE'],
      ['b', 'FALSE'],
    ]);
  });

  it('writes a number as a number, and what is no number as its text', () => {
    const shown = { ...view, shownProperties: ['amount'] };
    const rows = tableRows(
      [card('1', 'a', { amount: '-7' }), card('2', 'b', { amount: 'about 3' })],
      templates,
      shown,
      'Name',
      context,
    );
    expect(lines(csvText(rows)).slice(1, 3)).toEqual(['"a",-7', '"b","about 3"']);
  });

  it('adds the date a calendar view places its cards by', () => {
    const calendar = { ...view, viewType: 'calendar' as const, dateProperty: 'due' };
    expect(tableRows([], templates, calendar, 'Name', context)[0]).toEqual([
      'Name',
      'Status',
      'Tags',
      'Notes',
      'Amount',
      'Due',
    ]);
    expect(
      tableRows([], templates, { ...view, dateProperty: 'due' }, 'Name', context)[0],
    ).toHaveLength(5);
  });

  it('adds the date a calendar falls back to where it names none', () => {
    const calendar = { ...view, viewType: 'calendar' as const };
    expect(tableRows([], templates, calendar, 'Name', context)[0]?.at(-1)).toBe('Due');
  });

  it('adds the date a Gantt view places its cards by, the one it falls back to too', () => {
    const gantt = { ...view, viewType: 'gantt' as const };
    expect(tableRows([], templates, gantt, 'Name', context)[0]?.at(-1)).toBe('Due');
    expect(
      tableRows([], templates, { ...gantt, dateProperty: 'due' }, 'Name', context)[0],
    ).toHaveLength(6);
    // Shown already: not twice.
    const shown = { ...gantt, shownProperties: ['due'] };
    expect(tableRows([], templates, shown, 'Name', context)[0]).toEqual(['Name', 'Due']);
  });

  it('quotes the headings too, so a comma in a property name stays in its cell (Q12)', () => {
    const awkward: PropertyTemplate = {
      id: 'c',
      name: 'Cost, "total" #1',
      type: 'text',
      options: [],
    };
    const rows = tableRows([], [awkward], { ...view, shownProperties: ['c'] }, 'Name', context);
    expect(csvText(rows)).toBe(`${CSV_BOM}"Name","Cost, ""total"" #1"\r\n`);
  });
});

describe('merging updated blocks', () => {
  it('replaces updated blocks, appends new ones and drops deleted ones', () => {
    const block = (id: string, title: string, deletedAt?: number) => ({
      ...createBlock(),
      id,
      title,
      ...(deletedAt === undefined ? {} : { deletedAt }),
    });
    const merged = mergeBlocks(
      [block('a', 'old a'), block('b', 'old b'), block('c', 'old c')],
      [block('b', 'new b'), block('c', 'gone', 5), block('d', 'new d')],
    );
    expect(merged.map((item) => `${item.id}:${item.title}`)).toEqual([
      'a:old a',
      'b:new b',
      'd:new d',
    ]);
  });
});

describe('the condition of a filter after its property changed', () => {
  it('keeps a condition the new kind of value offers', () => {
    expect(filterConditionValidOrDefault('options', 'isEmpty')).toBe('isEmpty');
    expect(filterConditionValidOrDefault('date', 'isAfter')).toBe('isAfter');
  });

  it('treats people like options (Q13)', () => {
    for (const condition of ['includes', 'notIncludes', 'isEmpty', 'isNotEmpty'] as const) {
      expect(filterConditionValidOrDefault('person', condition)).toBe(condition);
    }
    expect(filterConditionValidOrDefault('person', 'contains')).toBe('includes');
  });

  it('falls back to the default of the kind, and to "includes" where nothing can be filtered', () => {
    expect(filterConditionValidOrDefault('text', 'includes')).toBe('is');
    expect(filterConditionValidOrDefault('boolean', 'is')).toBe('isSet');
    expect(filterConditionValidOrDefault('none', 'is')).toBe('includes');
  });
});

describe('names of filter conditions', () => {
  const messages = new Proxy({} as FilterConditionMessages, {
    get: (_target, name) => () => `<${String(name)}>`,
  });

  it('names a condition the kind of value offers', () => {
    expect(filterConditionDisplayString('notIncludes', messages, 'options')).toBe('<notIncludes>');
    expect(filterConditionDisplayString('isEmpty', messages, 'person')).toBe('<isEmpty>');
    expect(filterConditionDisplayString('isAfter', messages, 'date')).toBe('<isAfter>');
    expect(filterConditionDisplayString('endsWith', messages, 'text')).toBe('<endsWith>');
  });

  it('shows the default of the kind for a condition it does not offer', () => {
    expect(filterConditionDisplayString('is', messages, 'options')).toBe('<includes>');
    expect(filterConditionDisplayString('includes', messages, 'boolean')).toBe('<isSet>');
    expect(filterConditionDisplayString('includes', messages, 'date')).toBe('<is>');
    expect(filterConditionDisplayString('includes', messages, 'none')).toBe('(unknown)');
  });
});
