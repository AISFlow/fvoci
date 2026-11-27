/**
 * The parts of wire.ts that wire.test.ts does not reach: reading stored date
 * values, filling in blocks, identifiers. Like wire.test.ts, this file may
 * look inside the `fields` of a block.
 */
import { describe, expect, it } from 'vitest';

import { calendarDate } from '$lib/logic/dates.ts';

import {
  createAttachmentBlock,
  createBlock,
  createBoard,
  createBoardView,
  createCard,
  createFilterClause,
  createPatchesFromContainers,
  encodeBase32,
  formatDateValue,
  formatFilterDate,
  idKindOfBlockType,
  normalizeBlock,
  parseDateValue,
  parseFilterDate,
  plainPropertyValue,
  type Container,
} from '$lib/logic/wire.ts';

describe('the stored value of a date property', () => {
  const fifth = calendarDate(2026, 3, 5);
  const ninth = calendarDate(2026, 3, 9);
  // 09:30 on 5 March 2026 in UTC.
  const moment = Date.UTC(2026, 2, 5, 9, 30);

  it('reads a calendar day from text and a moment from an integer', () => {
    expect(parseDateValue({ from: '2026-03-05' })).toEqual({ from: { date: fifth } });
    expect(parseDateValue({ from: '2026-03-05', to: '2026-03-09' })).toEqual({
      from: { date: fifth },
      to: { date: ninth },
    });
    expect(parseDateValue({ from: moment })).toEqual({ from: { instant: moment } });
    expect(parseDateValue({ from: moment, to: moment + 3_600_000 })).toEqual({
      from: { instant: moment },
      to: { instant: moment + 3_600_000 },
    });
  });

  it('writes a day as its date and a moment as milliseconds since the epoch in UTC', () => {
    expect(formatDateValue({ from: { date: fifth } })).toEqual({ from: '2026-03-05' });
    expect(formatDateValue({ from: { date: fifth }, to: { date: ninth } })).toEqual({
      from: '2026-03-05',
      to: '2026-03-09',
    });
    expect(formatDateValue({ from: { instant: moment } })).toEqual({ from: 1_772_703_000_000 });
    expect(JSON.stringify(formatDateValue({ from: { instant: moment } }))).toBe(
      '{"from":1772703000000}',
    );
  });

  it('writes neither a time zone nor a flag for the time: the type of each end says it all', () => {
    for (const value of [
      { from: { date: fifth } },
      { from: { instant: moment }, to: { instant: moment + 1 } },
    ]) {
      expect(Object.keys(formatDateValue(value) ?? {}).sort()).toEqual(
        value.to ? ['from', 'to'] : ['from'],
      );
    }
  });

  it('reads back what it wrote', () => {
    for (const value of [
      { from: { date: fifth } },
      { from: { date: fifth }, to: { date: ninth } },
      { from: { instant: moment } },
      { from: { instant: moment }, to: { instant: moment + 86_400_000 } },
      { from: { instant: 0 } },
      { from: { date: calendarDate(1, 1, 1) }, to: { date: calendarDate(9999, 12, 31) } },
    ]) {
      expect(parseDateValue(formatDateValue(value))).toEqual(value);
    }
  });

  it('writes a range whose ends are the same as one end, and turns a backward one around', () => {
    expect(formatDateValue({ from: { date: fifth }, to: { date: fifth } })).toEqual({
      from: '2026-03-05',
    });
    expect(formatDateValue({ from: { instant: moment }, to: { instant: moment } })).toEqual({
      from: moment,
    });
    expect(formatDateValue({ from: { date: ninth }, to: { date: fifth } })).toEqual({
      from: '2026-03-05',
      to: '2026-03-09',
    });
    // A day and a moment cannot be the two ends of one value: the start is kept.
    expect(formatDateValue({ from: { date: fifth }, to: { instant: moment } })).toEqual({
      from: '2026-03-05',
    });
  });

  it('writes nothing for an empty value', () => {
    expect(formatDateValue({})).toBeUndefined();
  });

  it('reads a range whose ends are the same as one end', () => {
    expect(parseDateValue({ from: '2026-03-05', to: '2026-03-05' })).toEqual({
      from: { date: fifth },
    });
  });

  it('reads anything that is not a well-formed date value as an empty value', () => {
    for (const stored of [
      undefined,
      null,
      '',
      'tomorrow',
      '2026-03-05',
      1_772_703_000_000,
      ['2026-03-05'],
      {},
      { to: '2026-03-09' },
      { from: 'abc' },
      { from: '2026-3-5' },
      { from: '2026-02-30' },
      { from: '0000-01-01' },
      { from: '2026-03-05T09:30:00Z' },
      { from: -1 },
      { from: 1.5 },
      { from: Number.NaN },
      { from: Date.UTC(10000, 0, 1) },
      { from: '2026-03-05', to: 1_772_703_000_000 },
      { from: 1_772_703_000_000, to: '2026-03-05' },
      { from: '2026-03-09', to: '2026-03-05' },
      { from: moment, to: moment - 1 },
      { from: '2026-03-05', to: 'later' },
      // The text the value was before it became an object (Q10, Q11): not read any more.
      '{"from":1772712000000}',
      '{"from":1772712000000,"includeTime":true}',
      '1625553000000',
    ]) {
      expect(parseDateValue(stored)).toEqual({});
    }
  });

  it('gives a date value a key to count and compare by, and none to a malformed one', () => {
    expect(plainPropertyValue({ from: '2026-03-05' })).toBe('2026-03-05');
    expect(plainPropertyValue({ from: '2026-03-05', to: '2026-03-09' })).toBe(
      '2026-03-05/2026-03-09',
    );
    expect(plainPropertyValue({ from: moment })).toBe('1772703000000');
    expect(plainPropertyValue({ from: 'soon' })).toBeUndefined();
    expect(plainPropertyValue('text')).toBe('text');
    expect(plainPropertyValue(['a', 'b'])).toEqual(['a', 'b']);
    expect(plainPropertyValue([{ card: 'a', kind: 'fs', lagDays: 0 }])).toBeUndefined();
    expect(plainPropertyValue(undefined)).toBeUndefined();
  });

  it('reads and writes the day of a date filter as the date itself', () => {
    expect(formatFilterDate(fifth)).toBe('2026-03-05');
    expect(parseFilterDate('2026-03-05')).toBe(fifth);
    expect(parseFilterDate('soon')).toBeUndefined();
    expect(parseFilterDate('1772712000000')).toBeUndefined();
    expect(parseFilterDate(undefined)).toBeUndefined();
  });
});

describe('stored data with gaps', () => {
  it('fills a filter clause that has no values', () => {
    expect(createFilterClause({ propertyId: 'x', condition: 'isEmpty' })).toEqual({
      propertyId: 'x',
      condition: 'isEmpty',
      values: [],
    });
    const view = createBoardView({
      ...createBlock(),
      fields: { filter: { operation: 'or', filters: [{ propertyId: 'x', condition: 'isSet' }] } },
    });
    expect(view.fields.filter.filters).toEqual([
      { propertyId: 'x', condition: 'isSet', values: [] },
    ]);
  });

  it('makes board patches when a board has no properties object', () => {
    const board = createBoard();
    const bare = { ...board, properties: undefined } as unknown as Container;
    const [patch, undo] = createPatchesFromContainers({ ...board, properties: { a: '1' } }, bare);
    expect(patch.updatedProperties).toEqual({ a: '1' });
    expect(undo.deletedProperties).toEqual(['a']);
    expect(createPatchesFromContainers(bare, bare)[0].updatedProperties).toEqual({});
  });
});

describe('blocks', () => {
  it('fills the gaps of a block and copies its fields', () => {
    const source = { ...createBlock(), id: 'x', title: 't', fields: { a: 1 } };
    const copy = createBlock(source);
    expect(copy).toEqual({ ...source, limited: false });
    expect(copy.fields).not.toBe(source.fields);
    expect(createBlock().id).toMatch(/^7/);
  });

  it('drops empty entries of a content order and copies its rows', () => {
    const row = ['b', 'c'];
    const card = createCard({ ...createBlock(), fields: { contentOrder: ['a', null, row] } });
    expect(card.type).toBe('card');
    expect(card.fields.contentOrder).toEqual(['a', ['b', 'c']]);
    expect(card.fields.contentOrder[1]).not.toBe(row);
    expect(card.fields.properties).toEqual({});
  });

  it('reads the file of an attachment under its older name too', () => {
    const old = createAttachmentBlock({ ...createBlock(), fields: { attachmentId: 'file-1' } });
    expect(old.fields).toEqual({ fileId: 'file-1' });
  });

  it('gives a block of a known type the members of that type', () => {
    const view = normalizeBlock({ ...createBlock(), type: 'view' });
    expect(view.fields.viewType).toBe('board');
    expect(view.fields.filter).toEqual({ operation: 'and', filters: [] });
  });

  it('names the kind of identifier for a block type', () => {
    expect(['card', 'view', 'board', 'text', undefined].map(idKindOfBlockType)).toEqual([
      'card',
      'view',
      'board',
      'none',
      'none',
    ]);
  });

  it('encodes bytes in z-base-32', () => {
    expect(encodeBase32(new Uint8Array([0, 0, 0, 0, 0]))).toBe('yyyyyyyy');
    expect(encodeBase32(new Uint8Array([255]))).toBe('9h');
    expect(encodeBase32(new Uint8Array([255]), true)).toBe('9h======');
  });
});
