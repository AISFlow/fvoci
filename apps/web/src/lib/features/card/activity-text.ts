/**
 * The words of a line of a card's activity: what was done, with the values
 * as the board shows them (an option by its label, a person by their name, a
 * date as the reader writes it). A property the board has no more is named
 * as deleted, and its values are shown as they were stored.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { DisplayContext } from '$lib/logic/format.ts';
import { propertyBehavior } from '$lib/logic/properties.ts';
import type { Card, PropertyTemplate, PropertyValue } from '$lib/logic/wire.ts';
import type { CardActivityLine } from '$lib/state/index.ts';

export interface ActivityReading {
  /** The card the lines are of: the kinds that read the card itself read it. */
  card: Card;
  templates: readonly PropertyTemplate[];
  /** With `cardTitle` for the cards a value or a parent names, and `personName` for people. */
  context: DisplayContext;
}

const isEmpty = (value: unknown): boolean =>
  value === null ||
  value === undefined ||
  value === '' ||
  (Array.isArray(value) && value.length === 0);

function valueText(
  value: unknown,
  template: PropertyTemplate | undefined,
  reading: ActivityReading,
): string {
  if (isEmpty(value)) return m.card_activity_empty();
  if (!template) return typeof value === 'string' ? value : JSON.stringify(value);
  if (template.type === 'checkbox') return value === true || value === 'true' ? '✓' : '✗';
  const cell = propertyBehavior(template.type).exportValue(
    value as PropertyValue,
    reading.card,
    template,
    reading.context,
  );
  return String(cell) || m.card_activity_empty();
}

function cardText(cardId: unknown, reading: ActivityReading): string {
  if (typeof cardId !== 'string' || cardId === '') return m.card_activity_top();
  return reading.context.cardTitle?.(cardId) ?? m.card_activity_unseen_card();
}

const titleText = (title: unknown): string =>
  typeof title === 'string' && title !== '' ? title : m['KanbanCard.untitled']();

export function activityText(line: CardActivityLine, reading: ActivityReading): string {
  switch (line.kind) {
    case 'created':
      return m.card_activity_created();
    case 'restored':
      return m.card_activity_restored();
    case 'template':
      return line.after === true ? m.card_activity_template_on() : m.card_activity_template_off();
    case 'title':
      return m.card_activity_title({
        before: titleText(line.before),
        after: titleText(line.after),
      });
    case 'parent':
      return m.card_activity_parent({
        before: cardText(line.before, reading),
        after: cardText(line.after, reading),
      });
    case 'property': {
      const template = reading.templates.find((each) => each.id === line.propertyId);
      return m.card_activity_property({
        name: template?.name ?? m.card_activity_property_gone(),
        before: valueText(line.before, template, reading),
        after: valueText(line.after, template, reading),
      });
    }
  }
}
