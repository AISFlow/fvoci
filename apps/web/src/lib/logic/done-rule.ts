/**
 * What makes a card of a board done (`Container.doneWhen`): the rules the
 * board's properties offer (a checkbox ticked, or an option chosen), and the
 * value a card is given to be done by one.
 */
import type { PropertyValue } from './wire-blocks.ts';
import type { DoneRule, PropertyTemplate } from './wire.ts';

export interface DoneChoice {
  /** Tells the choice apart among the others (`checkbox:<property>`, `option:<property>:<option>`). */
  value: string;
  /** The property, and the option where there is one. */
  label: string;
  /** The option alone where there is one, else the property. */
  short: string;
  rule: DoneRule;
}

/** The rules `properties` offer, in their order: a checkbox's one, a select's one per option. */
export function doneChoices(properties: readonly PropertyTemplate[]): DoneChoice[] {
  return properties.flatMap((prop): DoneChoice[] => {
    if (prop.type === 'checkbox') {
      const rule: DoneRule = { kind: 'checkbox', propertyId: prop.id };
      return [{ value: `checkbox:${prop.id}`, label: prop.name, short: prop.name, rule }];
    }
    if (prop.type !== 'select' && prop.type !== 'multiSelect') return [];
    return prop.options.map((opt) => ({
      value: `option:${prop.id}:${opt.id}`,
      label: `${prop.name}: ${opt.label}`,
      short: opt.label,
      rule: { kind: 'option', propertyId: prop.id, optionIds: [opt.id] },
    }));
  });
}

/** The choice `rule` is among the choices, by its value; empty for none. */
export function doneChoiceValue(rule: DoneRule | null | undefined): string {
  if (!rule) return '';
  if (rule.kind === 'checkbox') return `checkbox:${rule.propertyId}`;
  const first = rule.optionIds[0];
  return first ? `option:${rule.propertyId}:${first}` : '';
}

/**
 * The value of the rule's property that makes a card done, from the one it
 * had: a ticked checkbox, the rule's option, or the options of a multi-select
 * with the rule's added.
 */
export function doneValue(
  rule: DoneRule,
  property: PropertyTemplate | undefined,
  had: PropertyValue | undefined,
): PropertyValue | undefined {
  if (rule.kind === 'checkbox') return 'true';
  const option = rule.optionIds[0];
  if (option === undefined) return undefined;
  if (property?.type !== 'multiSelect') return option;
  const chosen = Array.isArray(had) ? had.filter((one) => typeof one === 'string') : [];
  return chosen.includes(option) ? chosen : [...chosen, option];
}
