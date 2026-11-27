/**
 * The second line of a bar of a week
 * (features/views/calendar/placed.ts): the time and the days, then the
 * words of the properties the view turned on; people and boxes are left
 * to the card's own lines.
 */
import { describe, expect, it } from 'vitest';

import { Placed } from '$lib/features/views/calendar/placed.ts';
import type { CalendarDate } from '$lib/logic/dates.ts';
import type { Regional } from '$lib/logic/regional.ts';
import type { PropertyTemplate } from '$lib/logic/wire.ts';

import { createTestCard } from '../../logic/support/factory.ts';

const due: PropertyTemplate = { id: 'due', name: 'Due', type: 'date', options: [] };
const status: PropertyTemplate = {
  id: 'status',
  name: 'Status',
  type: 'select',
  options: [{ id: 'doing', label: 'Doing', color: 'blue' }],
};
const owner: PropertyTemplate = { id: 'owner', name: 'Owner', type: 'person', options: [] };
const days = ['2026-11-12', '2026-11-13', '2026-11-14'] as CalendarDate[];

describe('the second line of a bar of a week', () => {
  const regional: Regional = { timeZone: 'Asia/Seoul', weekStart: 7, hourCycle: 'h12' };

  it('has the days of a range, then the words of the properties turned on', () => {
    const card = createTestCard();
    card.fields.properties = {
      due: { from: '2026-11-12', to: '2026-11-14' },
      status: 'doing',
      owner: 'user-1',
    };
    const placed = new Placed([card], due, regional, 'en-US', days);
    expect(placed.detail(card, [status, owner])?.replace(/\s/gu, ' ')).toBe(
      'November 12 – 14 · Doing',
    );
  });

  it('has the time of a moment, and nothing more where nothing is turned on', () => {
    const card = createTestCard();
    card.fields.properties = { due: { from: Date.parse('2026-11-12T23:00:00Z') } };
    const placed = new Placed([card], due, regional, 'en-US', days);
    expect(placed.detail(card)).toBe('8:00 AM');
  });

  it('is nothing for a card of one day without a time or words', () => {
    const card = createTestCard();
    card.fields.properties = { due: { from: '2026-11-13' } };
    const placed = new Placed([card], due, regional, 'en-US', days);
    expect(placed.detail(card, [owner])).toBeUndefined();
  });
});
