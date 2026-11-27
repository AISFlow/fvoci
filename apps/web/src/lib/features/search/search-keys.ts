/**
 * The keys of the search: ⌘K on Apple's devices, Ctrl+K elsewhere, from
 * anywhere on the page, inside an editor too. The other modifier is no alias
 * (`$lib/keys.ts`): on a Mac, Ctrl+K in a field deletes to the end of the line.
 *
 * A press something under it already took (`defaultPrevented`: an editor
 * whose own ⌘K makes a link of what is chosen) is left to it, and so is one
 * while letters are being put together by an input method (`isComposing`).
 */
import { isComposing, primaryModifier, shortcutKeys, shortcutLabel } from '$lib/keys.ts';

/**
 * Whether a key press opens the search. By the letter the key gives; a key
 * that gives a letter of another script (a Korean layout gives ㅏ) by where
 * it is, the K of a Latin keyboard. A key held down asks again: the one who
 * listens takes the repeats too while the search is open, or the browser's
 * own Ctrl+K gets them.
 */
export function opensSearch(event: KeyboardEvent, platform?: string): boolean {
  if (event.defaultPrevented || isComposing(event)) return false;
  if (!primaryModifier(event, platform) || event.altKey || event.shiftKey) return false;
  const key = event.key.toLowerCase();
  if (key === 'k') return true;
  return !/^[a-z]$/.test(key) && event.code === 'KeyK';
}

/** What a key pressed in the field asks of the list of what was found. */
export type ListMove = 'next' | 'previous' | 'go' | 'goNewTab';

/**
 * What a key in the field of the search does to the list. Nothing while an
 * input method puts letters together: its Enter takes the letters, and its
 * arrows move among the candidates (`isComposing`). The platform's modifier
 * with Enter opens a new tab, as with a click; the other modifier does nothing.
 */
export function listMove(event: KeyboardEvent, platform?: string): ListMove | undefined {
  if (isComposing(event) || event.altKey) return undefined;
  const modified = event.ctrlKey || event.metaKey;
  if (event.key === 'Enter') {
    if (primaryModifier(event, platform)) return 'goNewTab';
    return modified || event.shiftKey ? undefined : 'go';
  }
  if (modified || event.shiftKey) return undefined;
  if (event.key === 'ArrowDown') return 'next';
  if (event.key === 'ArrowUp') return 'previous';
  return undefined;
}

/** The keys that open the search as a device's keyboard names them: ⌘K on Apple's, else Ctrl+K. */
export function searchKeysLabel(platform?: string): string {
  return shortcutLabel('K', {}, platform);
}

/** The keys that open the search, for `aria-keyshortcuts`. */
export function searchKeysNames(platform?: string): string {
  return shortcutKeys('K', {}, platform);
}
