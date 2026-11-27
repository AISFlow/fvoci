/**
 * The face of a card (components/card-face): what a kanban column and a
 * gallery show of a card.
 */
import { cleanup, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it } from 'vitest';

import CardFace from '$lib/components/card-face/CardFace.svelte';
import { faceProperties, hasFaceValue } from '$lib/components/card-face/face.ts';
import type { Regional } from '$lib/logic/regional.ts';
import { createCard, type Card, type PropertyTemplate } from '$lib/logic/wire.ts';

afterEach(() => {
  cleanup();
});

const SEOUL: Regional = { timeZone: 'Asia/Seoul', weekStart: 7, hourCycle: 'h12' };

const STATUS: PropertyTemplate = {
  id: 'status',
  name: 'Status',
  type: 'select',
  options: [
    { id: 'todo', label: 'To do', color: 'blue' },
    { id: 'done', label: 'Done', color: 'green' },
  ],
};
const HOURS: PropertyTemplate = { id: 'hours', name: 'Hours', type: 'number', options: [] };
const TAGS: PropertyTemplate = { id: 'tags', name: 'Tags', type: 'multiSelect', options: [] };
const DONE: PropertyTemplate = { id: 'done', name: 'Reviewed', type: 'checkbox', options: [] };
const MADE: PropertyTemplate = { id: 'made', name: 'Made', type: 'createdTime', options: [] };
const TEMPLATES = [STATUS, HOURS, TAGS, DONE, MADE];

function cardWith(properties: Card['fields']['properties'], title = 'Write the report'): Card {
  const card = createCard();
  card.id = 'card-1';
  card.title = title;
  card.fields.icon = '📝';
  card.fields.properties = properties;
  card.createdAt = Date.UTC(2026, 2, 5, 0, 0);
  return card;
}

function show(card: Card, shownProperties: string[], more: Record<string, unknown> = {}) {
  return render(CardFace, {
    card,
    templates: TEMPLATES,
    shownProperties,
    locale: 'en-US',
    regional: SEOUL,
    ...more,
  });
}

describe('which properties a card shows', () => {
  it('shows a property that has a value, and not one that is empty', () => {
    const card = cardWith({ status: 'todo', hours: '', tags: [] });
    expect(hasFaceValue(card, STATUS)).toBe(true);
    expect(hasFaceValue(card, HOURS)).toBe(false);
    expect(hasFaceValue(card, TAGS)).toBe(false);
    expect(hasFaceValue(cardWith({ tags: ['a'] }), TAGS)).toBe(true);
  });

  it('always shows a checkbox and what the card itself says', () => {
    const card = cardWith({});
    expect(hasFaceValue(card, DONE)).toBe(true);
    expect(hasFaceValue(card, MADE)).toBe(true);
  });

  it('follows the board’s order of its properties, and skips what is no property of the board', () => {
    const card = cardWith({ status: 'todo', hours: '8' });
    expect(
      faceProperties(card, TEMPLATES, ['hours', 'gone', '__title', 'status']).map(
        (template) => template.id,
      ),
    ).toEqual(['status', 'hours']);
    // Moved on the board, it moves on the face.
    expect(
      faceProperties(card, [HOURS, STATUS, TAGS, DONE, MADE], ['status', 'hours']).map(
        (template) => template.id,
      ),
    ).toEqual(['hours', 'status']);
    expect(faceProperties(card, TEMPLATES, [])).toEqual([]);
  });
});

describe('the face of a card', () => {
  it('shows the icon and the title', () => {
    const { container } = show(cardWith({}), []);
    expect(container.querySelector('[data-slot="card-icon"]')).toHaveTextContent('📝');
    expect(container.querySelector('[data-slot="card-title-text"]')).toHaveTextContent(
      'Write the report',
    );
  });

  it('says "Untitled" for a card without a title', () => {
    show(cardWith({}, ''), []);
    expect(screen.getByText('Untitled')).toBeInTheDocument();
  });

  it('gives the title the id it is told, for the element around it to be named by', () => {
    const { container } = show(cardWith({}), [], { titleId: 'the-title' });
    expect(container.querySelector('#the-title')).toHaveTextContent('Write the report');
  });

  it('shows the properties the view turned on, in the board’s order, each with its name', () => {
    const { container } = show(cardWith({ status: 'done', hours: '8' }), ['hours', 'status']);
    const rows = [...container.querySelectorAll('[data-property]')];
    expect(rows.map((row) => row.getAttribute('data-property'))).toEqual(['status', 'hours']);
    expect(rows[1]).toHaveTextContent('Hours: 8');
    expect(rows[0]).toHaveTextContent('Status: Done');
    expect(rows[0]?.querySelector('[data-slot="option-chip"]')).toHaveAttribute(
      'data-color',
      'green',
    );
  });

  it('shows no line for a property the card has no value for', () => {
    const { container } = show(cardWith({ status: 'todo' }), ['hours', 'status']);
    expect(container.querySelectorAll('[data-property]')).toHaveLength(1);
    expect(container.querySelector('[data-slot="card-properties"]')).not.toHaveTextContent('Hours');
  });

  it('shows properties as they are: nothing on the face can be edited', () => {
    const { container } = show(cardWith({ status: 'todo', hours: '8', done: 'true' }), [
      'status',
      'hours',
      'done',
    ]);
    expect(container.querySelector('input')).toBeNull();
    expect(screen.getByRole('checkbox', { name: 'Reviewed' })).toBeDisabled();
    expect(screen.getByRole('checkbox', { name: 'Reviewed' })).toBeChecked();
  });

  it('says that the card has a body, and how its tasks stand', () => {
    const { container } = show(cardWith({}), [], { hasBody: true, tasksTotal: 4, tasksDone: 1 });
    expect(container.querySelector('[data-slot="card-has-body"]')).toHaveTextContent(
      'This card has a description',
    );
    expect(container.querySelector('[data-slot="card-tasks"]')).toHaveTextContent('1/4');
  });

  it('shows no badges for a card without a body', () => {
    const { container } = show(cardWith({}), []);
    expect(container.querySelector('[data-slot="card-badges"]')).toBeNull();
  });

  it('says how many cards travel with it, when more than one', () => {
    const one = show(cardWith({}), [], { lifted: true });
    expect(one.container.querySelector('[data-slot="card-count"]')).toBeNull();
    cleanup();
    const three = show(cardWith({}), [], { lifted: true, count: 3 });
    expect(three.container.querySelector('[data-slot="card-count"]')).toHaveTextContent('3');
  });

  it('is larger in a gallery', () => {
    const { container } = show(cardWith({}), [], { size: 'large' });
    expect(container.querySelector('[data-slot="card-face"]')).toHaveAttribute(
      'data-size',
      'large',
    );
  });
});
