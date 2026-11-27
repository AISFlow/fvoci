/**
 * An emoji at random, for an icon given at once ("Add icon" of a card or a
 * page): the list is short on purpose until a picker over the full emoji list
 * is there.
 */

/**
 * Things, plants, food and weather: nothing that reads as a judgement on the
 * card. Every one is drawn in colour without a variation selector.
 */
// prettier-ignore
export const CARD_EMOJI: readonly string[] = [
  '🌱', '🌿', '🍀', '🌵', '🌲', '🌳', '🌴', '🌻', '🌼', '🌷',
  '🍁', '🍂', '🍄', '🌊', '🌙', '⭐', '🌈', '🔥', '⚡', '⛄',
  '🍎', '🍊', '🍋', '🍉', '🍇', '🍓', '🍒', '🍑', '🥝', '🥑',
  '🥕', '🌽', '🍞', '🧀', '🍪', '🍰', '🍵', '☕', '🍙', '🍜',
  '🎈', '🎁', '🎨', '🎬', '🎧', '🎹', '🎸', '🎲', '🎯', '🏆',
  '⚽', '🏀', '🎾', '🏓', '🚲', '🚗', '🚀', '⛵', '🏠', '⛺',
  '📌', '📎', '📐', '📚', '📖', '📝', '📦', '📫', '📷', '💡',
  '🔑', '🔔', '🔍', '🔧', '🧭', '🧩', '🧪', '🧵', '🧺', '🪴',
];

/** One emoji of the list. `random` is a number from 0 up to, not including, 1. */
export function randomEmoji(random: number = Math.random()): string {
  const at = Math.min(CARD_EMOJI.length - 1, Math.max(0, Math.floor(random * CARD_EMOJI.length)));
  return CARD_EMOJI[at] ?? '📌';
}
