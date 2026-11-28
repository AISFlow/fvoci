/**
 * The list an autocomplete shows under the cursor: people after `@`, emoji
 * after `:`; and the same list opened by hand (`choiceList`), as for the two
 * ways a pasted address of a view goes in (board-view-paste.ts). Plain DOM, styled by editor.css with the design tokens. This
 * layer sits below the app's popover components and cannot use them.
 *
 * The cursor stays in the editor; the list is driven from there. The arrow
 * keys move through it, Enter or Tab takes the entry, Escape closes it. A
 * key that an input method is composing with (the Enter that ends a Korean
 * syllable) is the input method's, never the list's; an entry pressed while a
 * syllable is being composed (`/젬`) is taken once it is written, or the
 * syllable would be taken away half made. It is placed again whenever the
 * editor reports the cursor's rectangle anew, which it does on every change
 * and when the window is resized.
 */
import type { EditorView } from '@tiptap/pm/view';
import type { SuggestionKeyDownProps, SuggestionProps } from '@tiptap/suggestion';

import { isComposing } from '$lib/keys.ts';

import type { SuggestionEntry, SuggestionRenderer } from './extensions.ts';

export interface SuggestionListOptions {
  /** Told apart in the page by `data-suggestions`. */
  kind: string;
  /** The accessible name of the list. */
  label: () => string;
}

let lists = 0;

/**
 * How long after a composition ends what it wrote is the document's: ProseMirror takes it in
 * within 20 ms (prosemirror-view `scheduleComposeEnd`).
 */
const COMPOSITION_WRITTEN = 30;

/** One opening of the list, with its DOM, selection and owner kept together. */
class SuggestionList {
  #list: HTMLDivElement | undefined;
  #entries: SuggestionEntry[] = [];
  #active = 0;
  #choose: (entry: SuggestionEntry) => void = () => undefined;
  #owner: HTMLElement | undefined;
  #view: EditorView | undefined;
  readonly #id = `doc-suggestions-${(lists += 1)}`;
  readonly #options: SuggestionListOptions;

  constructor(options: SuggestionListOptions) {
    this.#options = options;
  }

  #place(rect: DOMRect | null | undefined): void {
    if (!this.#list || !rect) return;
    const view = this.#list.ownerDocument.defaultView;
    const width = view?.innerWidth ?? 0;
    const height = view?.innerHeight ?? 0;
    const box = this.#list.getBoundingClientRect();
    // Under the cursor's line; above it when there is no room below. Never past an edge.
    const below = rect.bottom + 4;
    const top = below + box.height > height && rect.top - 4 - box.height >= 0;
    this.#list.style.left = `${Math.max(4, Math.min(rect.left, width - box.width - 4))}px`;
    this.#list.style.top = `${top ? rect.top - 4 - box.height : below}px`;
  }

  #draw(rect: DOMRect | null | undefined): void {
    if (!this.#list) return;
    const page = this.#list.ownerDocument;
    this.#list.replaceChildren(
      ...this.#entries.map((entry, index) => {
        const option = page.createElement('div');
        option.setAttribute('role', 'option');
        option.setAttribute('id', `${this.#id}-${index}`);
        option.setAttribute('aria-selected', String(index === this.#active));
        option.dataset.value = entry.value;
        option.textContent = entry.label;
        // `mousedown`, and prevented: a click must not take the focus out of the editor.
        option.addEventListener('mousedown', (event) => {
          event.preventDefault();
          this.#pressed(entry);
        });
        return option;
      }),
    );
    this.#list.hidden = this.#entries.length === 0;
    if (this.#entries.length > 0)
      this.#owner?.setAttribute('aria-activedescendant', `${this.#id}-${this.#active}`);
    else this.#owner?.removeAttribute('aria-activedescendant');
    this.#place(rect);
  }

  /** Takes an entry pressed; while a syllable is composed, once it is written. */
  #pressed(entry: SuggestionEntry): void {
    const view = this.#view;
    if (!view?.composing) {
      const choose = this.#choose;
      choose(entry);
      return;
    }
    const written = (): void => {
      setTimeout(() => {
        // The list may have closed meanwhile; the entry is taken with the range as it is now.
        if (!this.#list || view.composing) return;
        const choose = this.#choose;
        choose(entry);
      }, COMPOSITION_WRITTEN);
    };
    view.dom.addEventListener('compositionend', written, { once: true });
  }

  #close(): void {
    this.#list?.remove();
    this.#list = undefined;
    this.#owner?.removeAttribute('aria-activedescendant');
    this.#owner?.removeAttribute('aria-controls');
  }

  onStart = (props: SuggestionProps<SuggestionEntry, SuggestionEntry>): void => {
    this.open(props.editor.view, props.items, props.command, props.clientRect?.());
  };

  /** Opens the list of `entries` at `rect`, under the editor of `view`; `choose` takes one. */
  open(
    view: EditorView,
    entries: SuggestionEntry[],
    choose: (entry: SuggestionEntry) => void,
    rect: DOMRect | null | undefined,
  ): void {
    this.#view = view;
    this.#owner = view.dom;
    const page = this.#owner.ownerDocument;
    this.#list = page.createElement('div');
    this.#list.setAttribute('role', 'listbox');
    this.#list.setAttribute('id', this.#id);
    this.#list.setAttribute('aria-label', this.#options.label());
    this.#list.setAttribute('class', 'doc-suggestions');
    this.#list.dataset.suggestions = this.#options.kind;
    page.body.append(this.#list);
    this.#owner.setAttribute('aria-controls', this.#id);
    this.#entries = entries;
    this.#active = 0;
    this.#choose = choose;
    this.#draw(rect);
  }

  onUpdate = (props: SuggestionProps<SuggestionEntry, SuggestionEntry>): void => {
    this.#entries = props.items;
    this.#active = Math.min(this.#active, Math.max(0, this.#entries.length - 1));
    this.#choose = props.command;
    this.#draw(props.clientRect?.());
  };

  onKeyDown = ({ event }: SuggestionKeyDownProps): boolean => this.keyDown(event);

  /** Moves through the list, takes an entry, or closes it; whether the key was the list's. */
  keyDown(event: KeyboardEvent): boolean {
    if (!this.#list || this.#entries.length === 0 || isComposing(event)) return false;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      const step = event.key === 'ArrowDown' ? 1 : -1;
      this.#active = (this.#active + step + this.#entries.length) % this.#entries.length;
      this.#draw(undefined);
      // Not every page can scroll an element into view (a simulated one cannot).
      const chosen: Partial<Element> | null = this.#list.querySelector('[aria-selected="true"]');
      chosen?.scrollIntoView?.({ block: 'nearest' });
      return true;
    }
    if (event.key === 'Enter' || event.key === 'Tab') {
      const entry = this.#entries[this.#active];
      const choose = this.#choose;
      if (entry) choose(entry);
      return true;
    }
    if (event.key === 'Escape') {
      this.#close();
      return true;
    }
    return false;
  }

  onExit = (): void => {
    this.#close();
  };

  close(): void {
    this.#close();
  }
}

/** A list of choices opened by hand, not by what is typed (board-view-paste.ts). */
export interface ChoiceList {
  open: SuggestionList['open'];
  keyDown: SuggestionList['keyDown'];
  close: () => void;
}

export function choiceList(options: SuggestionListOptions): () => ChoiceList {
  return () => new SuggestionList(options);
}

export function suggestionList(options: SuggestionListOptions): NonNullable<SuggestionRenderer> {
  return () => new SuggestionList(options);
}
