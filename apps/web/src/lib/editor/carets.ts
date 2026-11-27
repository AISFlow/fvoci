/**
 * The carets of the others who edit a body together: each a line where
 * they are, their selection tinted, and their name for a few seconds after
 * they moved.
 *
 * The carets are y-protocols' awareness of the shared document (the room's
 * client holds it, `state/doc-carets.ts`); the plugin that writes the
 * reader's selection into it and draws the others' is the binding's own
 * (`yCursorPlugin`). Whose a caret is comes with it as a user identifier
 * the room wrote: the name and the colour are looked up here, never read
 * from the caret.
 *
 * A caret is no part of the document and is not said to a screen reader
 * (who else is there is said elsewhere); the name is drawn from an
 * attribute, so that the text of the body is the body's alone. It does not
 * blink. The name sits above the line where there is room, below it on the
 * first line, and moves left from its caret at the right edge.
 */
import { Extension } from '@tiptap/core';
import { Plugin, PluginKey } from '@tiptap/pm/state';
import type { EditorView } from '@tiptap/pm/view';
import { yCursorPlugin } from '@tiptap/y-tiptap';
import type { Awareness } from 'y-protocols/awareness';

export interface CaretsOptions {
  awareness: Awareness;
  /** The name shown for a user. */
  nameOf: (userId: string) => string;
  /** The colour of a user, as CSS: the one their picture has. */
  colorOf: (userId: string) => string;
}

/** How long a caret's name shows after it moved, in milliseconds. */
export const NAME_SHOWN = 3000;

interface Changes {
  added: number[];
  updated: number[];
  removed: number[];
}

/** The user identifier the room wrote into a caret's state. */
const userOf = (user: unknown): string => {
  const id = (user as { id?: unknown } | null | undefined)?.id;
  return typeof id === 'string' ? id : '';
};

/**
 * Whether a name `height` high has room above its caret at `at`: it would be
 * within the body, and the body itself is what is seen there (not cut off by a
 * box it scrolls in or the window, nor under a bar that stays over it). A name
 * lets the pointer through, so one drawn there already is not what is seen.
 */
function roomAbove(view: EditorView, body: DOMRect, at: DOMRect, height: number): boolean {
  const top = at.top - height;
  if (top < body.top) return false;
  const root = view.root;
  if (typeof root.elementFromPoint !== 'function') return true;
  const seen = root.elementFromPoint(at.left + 1, top + 1);
  return seen !== null && view.dom.contains(seen);
}

/**
 * Puts a name where it is not cut off: under its line where there is no room above, and moved left
 * from its caret as far as keeps it within the right edge (the body's, or
 * the window's where the body runs past it), never past the left one. The
 * name is looked up again: it may have become known since it was drawn.
 */
function place(view: EditorView, caret: HTMLElement, nameOf: (userId: string) => string): void {
  const name = caret.querySelector<HTMLElement>('.doc-caret-name');
  if (!name) return;
  const shown = nameOf(caret.dataset.user ?? '');
  if (name.dataset.name !== shown) name.dataset.name = shown;
  const box = view.dom.getBoundingClientRect();
  const at = caret.getBoundingClientRect();
  const size = name.getBoundingClientRect();
  const right = Math.min(box.right, view.dom.ownerDocument.documentElement.clientWidth);
  const past = Math.max(0, at.left + size.width - right);
  const moved = Math.min(past, Math.max(0, at.left - box.left));
  caret.toggleAttribute('data-below', !roomAbove(view, box, at, size.height));
  name.style.setProperty('--moved', `${String(-moved)}px`);
}

/**
 * Calls `again` whenever where a name fits may be elsewhere: the body is
 * another width (the window, the card's frame), or the body's box or the
 * window scrolled, so that what is seen above a name is something else.
 * Returns what stops it.
 */
function watch(body: HTMLElement, again: () => void): () => void {
  const resized = typeof ResizeObserver === 'undefined' ? undefined : new ResizeObserver(again);
  resized?.observe(body);
  const page = body.ownerDocument;
  page.addEventListener('scroll', again, { capture: true, passive: true });
  return () => {
    resized?.disconnect();
    page.removeEventListener('scroll', again, { capture: true });
  };
}

/** The line of a caret of `userId`, with their name to be shown beside it. */
function draw(options: CaretsOptions, userId: string): HTMLElement {
  const caret = document.createElement('span');
  caret.className = 'doc-caret';
  caret.setAttribute('aria-hidden', 'true');
  caret.dataset.user = userId;
  caret.style.setProperty('--caret', options.colorOf(userId));
  const name = document.createElement('span');
  name.className = 'doc-caret-name';
  name.dataset.name = options.nameOf(userId);
  caret.append(name);
  return caret;
}

/** The extension that shows the others' carets and tells the reader's own. */
export function caretsOf(options: CaretsOptions): Extension {
  const { awareness } = options;
  /** The line drawn for each caret now, by the number its state has. */
  const drawn = new Map<number, HTMLElement>();
  /** The carets whose name shows, each with what hides it again. */
  const named = new Map<number, ReturnType<typeof setTimeout>>();
  let view: EditorView | undefined;
  let frame: number | undefined;

  const placeSoon = (): void => {
    if (frame !== undefined || !view) return;
    frame = requestAnimationFrame(() => {
      frame = undefined;
      for (const [client, caret] of drawn) {
        if (!caret.isConnected) drawn.delete(client);
        // A name that does not show is placed as it shows again (`moved`).
        else if (view && !view.isDestroyed && caret.hasAttribute('data-named'))
          place(view, caret, options.nameOf);
      }
    });
  };

  const hide = (client: number): void => {
    clearTimeout(named.get(client));
    named.delete(client);
    drawn.get(client)?.removeAttribute('data-named');
  };

  const moved = (client: number): void => {
    clearTimeout(named.get(client));
    named.set(
      client,
      setTimeout(() => {
        hide(client);
      }, NAME_SHOWN),
    );
    drawn.get(client)?.setAttribute('data-named', '');
  };

  const changed = ({ added, updated, removed }: Changes): void => {
    for (const client of [...added, ...updated]) {
      if (client !== awareness.clientID) moved(client);
    }
    for (const client of removed) {
      hide(client);
      drawn.delete(client);
    }
    placeSoon();
  };

  return Extension.create({
    name: 'carets',

    addProseMirrorPlugins() {
      return [
        yCursorPlugin(awareness, {
          cursorBuilder: (user, client) => {
            const caret = draw(options, userOf(user));
            if (named.has(client)) caret.setAttribute('data-named', '');
            drawn.set(client, caret);
            placeSoon();
            return caret;
          },
          selectionBuilder: (user) => ({
            class: 'doc-caret-selection',
            style: `--caret: ${options.colorOf(userOf(user))}`,
          }),
        }),
        new Plugin({
          key: new PluginKey('caretNames'),
          view: (editorView) => {
            view = editorView;
            awareness.on('change', changed);
            const unwatch = watch(editorView.dom, () => {
              if (named.size > 0) placeSoon();
            });
            return {
              update: placeSoon,
              destroy: () => {
                unwatch();
                awareness.off('change', changed);
                for (const timer of named.values()) clearTimeout(timer);
                named.clear();
                drawn.clear();
                if (frame !== undefined) cancelAnimationFrame(frame);
                frame = undefined;
                view = undefined;
              },
            };
          },
        }),
      ];
    },
  });
}
