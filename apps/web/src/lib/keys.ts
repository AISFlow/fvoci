/** Reading the keyboard. */

/**
 * Whether a key press belongs to an input method that is composing text
 * (Hangul, kana, pinyin). Such a press must not be taken as a command: the
 * Enter that confirms a syllable is not the Enter that saves a field.
 *
 * Safari reports the confirming Enter with `isComposing` already false, and
 * marks it only with the key code 229, so both are checked.
 */
export function isComposing(
  event: Partial<Pick<KeyboardEvent, 'isComposing' | 'keyCode'>>,
): boolean {
  // eslint-disable-next-line @typescript-eslint/no-deprecated -- the only mark Safari leaves on that key press
  return event.isComposing === true || event.keyCode === 229;
}

/** The platform is read when needed, so rendering without a browser is safe. */
export function isApplePlatform(
  platform = typeof navigator === 'undefined' ? '' : navigator.platform,
): boolean {
  return /Mac|iPhone|iPad|iPod/.test(platform);
}

/** Command on Apple devices, Control elsewhere; the other modifier is never an alias. */
export function primaryModifier(
  event: Pick<KeyboardEvent, 'ctrlKey' | 'metaKey'>,
  platform?: string,
): boolean {
  return isApplePlatform(platform)
    ? event.metaKey && !event.ctrlKey
    : event.ctrlKey && !event.metaKey;
}

/**
 * Whether a key press is the one that keeps a version of a body now: ⌘S on
 * Apple devices and Ctrl+S elsewhere, without Shift or Alt. A layout of
 * other letters than Latin ones (Hangul's gives `ㄴ`, a syllable being
 * composed `Process`) is read by where the key is; a Latin one by its
 * letter, so that the key where S is on QWERTY is no save on another layout.
 */
export function isSaveKey(
  event: Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'metaKey' | 'shiftKey' | 'altKey'>,
  platform?: string,
): boolean {
  if (event.shiftKey || event.altKey || !primaryModifier(event, platform)) return false;
  return /^[a-z]$/i.test(event.key) ? event.key.toLowerCase() === 's' : event.code === 'KeyS';
}

type ShortcutOptions = { shift?: boolean; alt?: boolean };

/** Native symbols for Apple keyboards; familiar key names elsewhere. */
export function shortcutLabel(
  key: string,
  options: ShortcutOptions = {},
  platform?: string,
): string {
  if (isApplePlatform(platform)) {
    return `${options.alt ? '⌥' : ''}${options.shift ? '⇧' : ''}⌘${key}`;
  }
  return `Ctrl+${options.alt ? 'Alt+' : ''}${options.shift ? 'Shift+' : ''}${key}`;
}

/** aria-keyshortcuts uses key names even where the visible hint uses symbols. */
export function shortcutKeys(
  key: string,
  options: ShortcutOptions = {},
  platform?: string,
): string {
  return `${isApplePlatform(platform) ? 'Meta' : 'Control'}+${options.alt ? 'Alt+' : ''}${options.shift ? 'Shift+' : ''}${key}`;
}

/**
 * An editor of a body counts as a field even while it is read-only for now
 * (the connection away): the reader who presses the keys there means its text.
 */
const FIELDS =
  'input, textarea, select, [contenteditable]:not([contenteditable="false"]), [data-body-editor]';

/** Whether an event started in something the reader types into. */
export function inField(target: EventTarget | null): boolean {
  return target instanceof Element && target.closest(FIELDS) !== null;
}
