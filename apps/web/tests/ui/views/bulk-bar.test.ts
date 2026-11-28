/** What the bar of a selection offers, worked out without a page (features/views/bulk-bar.ts). */
import { describe, expect, it } from 'vitest';

import { bulkProperties, sharedValue } from '$lib/features/views/bulk-bar.ts';
import {
  createCard,
  type Card,
  type PropertyTemplate,
  type PropertyType,
  type PropertyValue,
} from '$lib/logic/wire.ts';

const template = (id: string, type: PropertyType): PropertyTemplate => ({
  id,
  name: id,
  type,
  options: [],
});

const TEMPLATES = [
  template('stage', 'select'),
  template('owner', 'person'),
  template('reviewers', 'multiPerson'),
  template('due', 'date'),
  template('blocks', 'relation'),
  template('made', 'createdTime'),
  template('note', 'text'),
  template('status', 'select'),
];

const ids = (templates: readonly PropertyTemplate[]): string[] => templates.map((each) => each.id);

describe('the properties of the bar', () => {
  it('give the status to what the view groups by, before the board status', () => {
    const roles = bulkProperties(TEMPLATES, { groupBy: 'stage', status: 'status', due: 'due' });
    expect(roles.status?.id).toBe('stage');
    expect(roles.assignee?.id).toBe('owner');
    expect(roles.due?.id).toBe('due');
    expect(ids(roles.others)).toEqual(['reviewers', 'note', 'status']);
  });

  it('fall back to the board status where the view groups by nothing the bar sets', () => {
    expect(bulkProperties(TEMPLATES, { groupBy: 'made', status: 'status' }).status?.id).toBe(
      'status',
    );
    expect(bulkProperties(TEMPLATES, {}).status).toBeUndefined();
  });

  it('give cards to the next person where the view groups by the first', () => {
    const roles = bulkProperties(TEMPLATES, { groupBy: 'owner' });
    expect(roles.status?.id).toBe('owner');
    expect(roles.assignee?.id).toBe('reviewers');
  });

  it('take a due date only where it is a date', () => {
    expect(bulkProperties(TEMPLATES, { due: 'note' }).due).toBeUndefined();
    expect(bulkProperties(TEMPLATES, { due: null }).due).toBeUndefined();
  });

  it('never set a relation, or what the card itself says', () => {
    const roles = bulkProperties(TEMPLATES, {});
    expect(ids(roles.others)).not.toContain('blocks');
    expect(ids(roles.others)).not.toContain('made');
  });
});

const cardWith = (value: PropertyValue | undefined): Card => {
  const card = createCard();
  return value === undefined
    ? card
    : { ...card, fields: { ...card.fields, properties: { p: value } } };
};

describe('the value the selected cards share', () => {
  it('is their value where they all have it', () => {
    expect(sharedValue([cardWith('a'), cardWith('a')], 'p')).toEqual({ mixed: false, value: 'a' });
  });

  it('is mixed where two differ', () => {
    expect(sharedValue([cardWith('a'), cardWith('b')], 'p')).toEqual({
      mixed: true,
      value: undefined,
    });
    expect(sharedValue([cardWith('a'), cardWith(undefined)], 'p').mixed).toBe(true);
  });

  it('is none where none has one: an empty text or list is no value', () => {
    expect(sharedValue([cardWith(undefined), cardWith(''), cardWith([])], 'p')).toEqual({
      mixed: false,
      value: undefined,
    });
  });

  it('is the same date whichever way its keys were written', () => {
    const one = cardWith({ from: '2026-11-28', to: '2026-11-30' });
    const other = cardWith({ to: '2026-11-30', from: '2026-11-28' });
    expect(sharedValue([one, other], 'p').mixed).toBe(false);
  });
});
