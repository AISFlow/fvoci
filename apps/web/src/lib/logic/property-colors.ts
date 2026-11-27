/**
 * The colour names a select or multi-select option can carry on the wire,
 * in the order pickers offer them.
 */
export const PROPERTY_COLORS = [
  'default',
  'gray',
  'brown',
  'orange',
  'yellow',
  'green',
  'blue',
  'purple',
  'pink',
  'red',
] as const;

export type PropertyColor = (typeof PROPERTY_COLORS)[number];

/** The colour a new option starts with. */
export const DEFAULT_PROPERTY_COLOR: PropertyColor = 'default';
