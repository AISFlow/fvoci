/**
 * The keys of the notifications list while it has the focus, as a mail
 * inbox's: J or ↓ the next, K or ↑ the one before, Enter opens it, E puts it
 * away (or back, in the archive), U marks it read or unread again, and ⇧U
 * marks all as read. A letter is read by where its key is on a layout of
 * other letters than Latin ones (Hangul's gives `ㅓ` for J), by the letter on
 * a Latin one; a press that an input method is composing with, or one with
 * ⌘, Ctrl or Alt, is none of them.
 */
import type { Attachment } from 'svelte/attachments';

import { isComposing } from '$lib/keys.ts';
import type { Notification } from '$lib/state/notification-types.ts';

export type InboxKey = 'next' | 'previous' | 'open' | 'archive' | 'toggleRead' | 'readAll';

type Press = Pick<
  KeyboardEvent,
  'key' | 'code' | 'shiftKey' | 'ctrlKey' | 'metaKey' | 'altKey' | 'isComposing' | 'keyCode'
>;

/** The letter of the key, lower case: by the letter on a Latin layout, by the key's place else. */
function letterOf(press: Press): string | undefined {
  if (/^[a-z]$/i.test(press.key)) return press.key.toLowerCase();
  const place = /^Key([A-Z])$/.exec(press.code);
  return place?.[1]?.toLowerCase();
}

/** What a press does in the list, if anything. */
export function inboxKey(press: Press): InboxKey | undefined {
  if (isComposing(press) || press.ctrlKey || press.metaKey || press.altKey) return undefined;
  if (press.key === 'ArrowDown' && !press.shiftKey) return 'next';
  if (press.key === 'ArrowUp' && !press.shiftKey) return 'previous';
  if (press.key === 'Enter' && !press.shiftKey) return 'open';
  const letter = letterOf(press);
  if (press.shiftKey) return letter === 'u' ? 'readAll' : undefined;
  switch (letter) {
    case 'j':
      return 'next';
    case 'k':
      return 'previous';
    case 'e':
      return 'archive';
    case 'u':
      return 'toggleRead';
    default:
      return undefined;
  }
}

/** What the list's keys do to the notification of a line, and to all. */
export interface ListKeyActions {
  /** The notification of a line, by its identifier. */
  readonly of: (notificationId: string) => Notification | undefined;
  readonly toggleRead: (notification: Notification) => void;
  readonly toggleArchive: (notification: Notification) => void;
  readonly readAll: () => void;
}

/**
 * The keys of the list it is attached to, from a line (`[data-row-focus]`):
 * the next and the one before take the focus; a line put away hands it to
 * the next, or to the one before where it was the last. Enter is the link's
 * own; a key pressed on a line's menu, or anywhere else, is theirs.
 */
export function listKeys(actions: ListKeyActions): Attachment<HTMLElement> {
  return (list) => {
    const onKeyDown = (event: KeyboardEvent): void => {
      const key = inboxKey(event);
      const from = event.target;
      if (!key || key === 'open' || !(from instanceof HTMLElement)) return;
      if (!from.matches('[data-row-focus]')) return;
      const rows = [...list.querySelectorAll<HTMLElement>('[data-notification]')];
      const at = rows.findIndex((row) => row.contains(from));
      const focus = (index: number): void => {
        rows[index]?.querySelector<HTMLElement>('[data-row-focus]')?.focus();
      };
      event.preventDefault();
      const notification = actions.of(rows[at]?.dataset.notification ?? '');
      switch (key) {
        case 'next':
          focus(Math.min(rows.length - 1, at + 1));
          break;
        case 'previous':
          focus(Math.max(0, at - 1));
          break;
        case 'readAll':
          actions.readAll();
          break;
        case 'toggleRead':
          if (notification) actions.toggleRead(notification);
          break;
        case 'archive':
          if (!notification) break;
          focus(at + 1 < rows.length ? at + 1 : at - 1);
          actions.toggleArchive(notification);
          break;
      }
    };
    list.addEventListener('keydown', onKeyDown);
    return () => {
      list.removeEventListener('keydown', onKeyDown);
    };
  };
}
