/**
 * The colours a person can give a select option. An option stores the name
 * of its colour (`brown`); the fill it is drawn with is a token of
 * styles/app.css, and its text is always the page's text colour.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { DEFAULT_PROPERTY_COLOR, PROPERTY_COLORS, type PropertyColor } from '$lib/logic/wire.ts';

/** The colour names, in the order they are offered. */
export const OPTION_COLORS = PROPERTY_COLORS;

export type OptionColor = PropertyColor;

/** The colour a new option starts with. */
export const DEFAULT_OPTION_COLOR: OptionColor = DEFAULT_PROPERTY_COLOR;

// Whole class names, so that the style sheet is built with them.
const FILLS: Record<OptionColor, string> = {
  default: 'bg-option-default',
  gray: 'bg-option-gray',
  brown: 'bg-option-brown',
  orange: 'bg-option-orange',
  yellow: 'bg-option-yellow',
  green: 'bg-option-green',
  blue: 'bg-option-blue',
  purple: 'bg-option-purple',
  pink: 'bg-option-pink',
  red: 'bg-option-red',
};

const NAMES: Record<OptionColor, () => string> = {
  default: m.property_color_default,
  gray: m.property_color_gray,
  brown: m.property_color_brown,
  orange: m.property_color_orange,
  yellow: m.property_color_yellow,
  green: m.property_color_green,
  blue: m.property_color_blue,
  purple: m.property_color_purple,
  pink: m.property_color_pink,
  red: m.property_color_red,
};

/** The class that fills a label with a colour. */
export function optionFill(color: OptionColor): string {
  return FILLS[color];
}

/** The name of a colour in the reader's language. */
export function optionColorName(color: OptionColor): string {
  return NAMES[color]();
}
