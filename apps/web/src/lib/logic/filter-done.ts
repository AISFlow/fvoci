/**
 * Whether a card is done by its board's rule (`Container.doneWhen`), for the
 * filters: the pseudo property `done` a view's filter can hold a condition
 * on, and the quick filters that leave out done cards.
 */
import { cardProperty, type Card, type DoneRule, type PropertyTemplate } from './wire.ts';

/**
 * The pseudo property of a condition on whether a card is done: `isSet` is
 * done, `isNotSet` is not. Like the title's (`title`), it is no property of
 * the board, and the server lets a condition on it through unchecked.
 */
export const DONE_FILTER_ID = 'done';

/**
 * The rule of done of a board, when it is one a card can meet: on a property
 * the board still has. A rule on a property deleted since tells nothing.
 */
export function doneRuleOf(
  rule: DoneRule | null | undefined,
  templates: readonly PropertyTemplate[],
): DoneRule | undefined {
  return rule && templates.some((template) => template.id === rule.propertyId) ? rule : undefined;
}

/**
 * Whether `card` is done by `rule`: its box ticked, or one of the rule's
 * options chosen. A rule read without its kind is a rule of options, as the
 * server's is when it names some.
 */
export function isCardDone(rule: DoneRule, card: Card): boolean {
  const value = cardProperty(card, rule.propertyId);
  if (rule.kind === 'checkbox') return value === 'true';
  const chosen = Array.isArray(value) ? value : typeof value === 'string' ? [value] : [];
  return chosen.some((id) => typeof id === 'string' && rule.optionIds.includes(id));
}
