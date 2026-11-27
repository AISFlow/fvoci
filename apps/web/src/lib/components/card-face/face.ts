/** What the face of a card shows of its properties. */
import type { Card, PropertyTemplate } from '$lib/logic/wire.ts';

/** The kinds whose value comes from the card itself, or that say something even when empty. */
const ALWAYS_SHOWN: readonly string[] = [
  'checkbox',
  'createdBy',
  'createdTime',
  'updatedBy',
  'updatedTime',
];

/** Whether a property has something to show on a card: an empty value takes no line. */
export function hasFaceValue(card: Card, template: PropertyTemplate): boolean {
  // A relation is a row of links to other cards: the card detail and the table show it.
  if (template.type === 'relation') return false;
  if (ALWAYS_SHOWN.includes(template.type)) return true;
  const value = card.fields.properties[template.id];
  if (value === undefined || value === '') return false;
  return !Array.isArray(value) || value.length > 0;
}

/**
 * The properties a card shows on its face: those the view turned on, in the
 * board's order of its properties (which the reader can change), less those
 * the card has no value for. An identifier that is no property of the board
 * (a deleted one, the title) is nothing to show.
 */
export function faceProperties(
  card: Card,
  templates: readonly PropertyTemplate[],
  shownProperties: readonly string[],
): PropertyTemplate[] {
  return templates.filter(
    (template) => shownProperties.includes(template.id) && hasFaceValue(card, template),
  );
}
