import { inField, isComposing, primaryModifier } from '$lib/keys.ts';

export { inField } from '$lib/keys.ts';

/**
 * The keys for undo and redo: ⌘Z on Apple devices and Ctrl+Z elsewhere (the
 * other modifier is no alias), and with Shift for redo.
 *
 * Inside a field they are left alone: a field has an undo of its own for
 * what was typed, and taking the keys there would undo a change of the board
 * while the reader meant a word.
 *
 * A key held down asks once: each undo is a write, and a held key would ask
 * for many more than the reader can follow.
 */

export type UndoKey = 'undo' | 'redo';

type KeyPress = Pick<
  KeyboardEvent,
  'key' | 'ctrlKey' | 'metaKey' | 'shiftKey' | 'altKey' | 'target' | 'defaultPrevented'
> &
  Partial<Pick<KeyboardEvent, 'repeat' | 'isComposing' | 'keyCode'>>;

/** What a key press asks for, if it is one of the two. */
export function undoKey(event: KeyPress, platform?: string): UndoKey | undefined {
  if (event.defaultPrevented || event.altKey || event.repeat || isComposing(event))
    return undefined;
  if (!primaryModifier(event, platform) || event.key.toLowerCase() !== 'z') return undefined;
  if (inField(event.target)) return undefined;
  return event.shiftKey ? 'redo' : 'undo';
}
