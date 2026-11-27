/**
 * The button on a code block in the editor that names its language and
 * opens a list to choose another from, "No language" first. Plain DOM, styled
 * by code-block.css with the design tokens: this layer sits below the app's
 * popover components and cannot use them.
 *
 * The list has a field that narrows it as the reader types (the combobox
 * pattern: the field keeps the focus, the arrow keys move through the list,
 * Enter takes the language, Escape closes and gives the focus back to the
 * button). A press anywhere else closes it. While an input method composes,
 * its keys are its own.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import { isComposing } from '$lib/keys.ts';

import { CODE_LANGUAGES, codeLanguageOf, type CodeLanguage } from './code-languages.ts';

/** A row of the list: a language, or none. */
type Choice = CodeLanguage | null;

const CHOICES: readonly Choice[] = [null, ...CODE_LANGUAGES];

/** What a block's language is called on its button: its name, or the name it was stored with. */
export function languageName(stored: string | null): string {
  if (stored === null) return m.editor_code_language_none();
  return codeLanguageOf(stored)?.label ?? stored;
}

/** Whether a row is found by what was typed. */
function matches(choice: Choice, query: string): boolean {
  if (query === '') return true;
  if (choice === null) return m.editor_code_language_none().toLowerCase().includes(query);
  return (
    choice.label.toLowerCase().includes(query) ||
    [choice.id, ...choice.aliases].some((name) => name.startsWith(query))
  );
}

export interface LanguagePickerOptions {
  /** Whether the language may be changed now: the editor can be written. */
  editable: () => boolean;
  /** Changes the block's language; `null` for none. */
  choose: (language: string | null) => void;
}

let pickers = 0;

export class LanguagePicker {
  /** The button and, while it is open, the list under it. */
  readonly element: HTMLElement;
  readonly #page: Document;
  readonly #options: LanguagePickerOptions;
  readonly #button: HTMLButtonElement;
  readonly #name: HTMLElement;
  readonly #popup: HTMLElement;
  readonly #field: HTMLInputElement;
  readonly #list: HTMLElement;
  readonly #id = `doc-code-languages-${(pickers += 1)}`;
  #stored: string | null;
  #shown: readonly Choice[] = CHOICES;
  #active = 0;

  constructor(page: Document, stored: string | null, options: LanguagePickerOptions) {
    this.#page = page;
    this.#options = options;
    this.#stored = stored;
    this.element = page.createElement('span');
    this.element.setAttribute('class', 'doc-code-picker');

    this.#button = page.createElement('button');
    this.#button.setAttribute('type', 'button');
    this.#button.setAttribute('class', 'doc-code-language');
    this.#button.setAttribute('aria-haspopup', 'listbox');
    this.#button.setAttribute('aria-expanded', 'false');
    this.#name = page.createElement('span');
    const chevron = page.createElement('span');
    chevron.setAttribute('class', 'doc-code-chevron');
    chevron.setAttribute('aria-hidden', 'true');
    this.#button.append(this.#name, chevron);

    this.#popup = page.createElement('div');
    this.#popup.setAttribute('class', 'doc-code-popup');
    this.#popup.hidden = true;
    this.#field = page.createElement('input');
    for (const [name, value] of Object.entries({
      type: 'text',
      class: 'doc-code-search',
      role: 'combobox',
      'aria-label': m.editor_code_language_search(),
      placeholder: m.editor_code_language_search(),
      'aria-expanded': 'true',
      'aria-controls': `${this.#id}-list`,
      'aria-autocomplete': 'list',
      autocomplete: 'off',
      autocapitalize: 'off',
      spellcheck: 'false',
      enterkeyhint: 'done',
    }))
      this.#field.setAttribute(name, value);
    this.#list = page.createElement('ul');
    this.#list.setAttribute('class', 'doc-code-options');
    this.#list.setAttribute('id', `${this.#id}-list`);
    this.#list.setAttribute('role', 'listbox');
    this.#list.setAttribute('aria-label', m.editor_code_language());
    this.#popup.append(this.#field, this.#list);
    this.element.append(this.#button, this.#popup);

    this.#button.addEventListener('click', () => {
      if (this.#popup.hidden) this.open();
      else this.close(true);
    });
    this.#button.addEventListener('keydown', (event) => {
      if (event.key !== 'ArrowDown' || !this.#popup.hidden) return;
      event.preventDefault();
      this.open();
    });
    this.#field.addEventListener('input', () => {
      this.#filter();
    });
    this.#field.addEventListener('keydown', (event) => {
      this.#key(event);
    });
    // Leaving the button and the list, by the keys or by a press elsewhere, closes it.
    this.element.addEventListener('focusout', (event) => {
      const next = event.relatedTarget;
      if (!(next instanceof Node) || !this.element.contains(next)) this.close(false);
    });
    this.#list.addEventListener('mousedown', (event) => {
      // The field keeps the focus while a row is pressed.
      event.preventDefault();
    });
    this.#list.addEventListener('click', (event) => {
      const row = event.target instanceof Element ? event.target.closest('[role="option"]') : null;
      const index = row ? Number(row.getAttribute('data-index')) : Number.NaN;
      if (Number.isInteger(index)) this.#pick(index);
    });
    this.set(stored);
  }

  /** The block's language changed (here, or by someone else): the button says the new one. */
  set(stored: string | null): void {
    this.#stored = stored;
    const name = languageName(stored);
    this.#name.textContent = name;
    this.#button.setAttribute('aria-label', m.editor_code_language_named({ language: name }));
  }

  /** Whether the language can be changed here: in an editor that only reads, the button only names it. */
  enable(editable: boolean): void {
    this.#button.disabled = !editable;
    if (editable) this.#button.setAttribute('aria-haspopup', 'listbox');
    else {
      this.#button.removeAttribute('aria-haspopup');
      this.close(false);
    }
  }

  open(): void {
    if (!this.#options.editable()) return;
    this.#field.value = '';
    this.#popup.hidden = false;
    this.#button.setAttribute('aria-expanded', 'true');
    this.#filter();
    const current = this.#stored === null ? null : (codeLanguageOf(this.#stored) ?? undefined);
    const at = current === undefined ? 0 : this.#shown.indexOf(current);
    this.#highlight(Math.max(at, 0));
    this.#field.focus();
  }

  /** Closes the list; `focusButton` gives the focus back to the button. */
  close(focusButton: boolean): void {
    if (this.#popup.hidden) return;
    this.#popup.hidden = true;
    this.#button.setAttribute('aria-expanded', 'false');
    this.#field.removeAttribute('aria-activedescendant');
    if (focusButton) this.#button.focus();
  }

  destroy(): void {
    this.#popup.hidden = true;
  }

  #rowId(index: number): string {
    return `${this.#id}-${index}`;
  }

  #filter(): void {
    const query = this.#field.value.trim().toLowerCase();
    this.#shown = CHOICES.filter((choice) => matches(choice, query));
    const page = this.#page;
    const selected = this.#stored === null ? null : codeLanguageOf(this.#stored);
    const rows = this.#shown.map((choice, index) => {
      const row = page.createElement('li');
      row.setAttribute('id', this.#rowId(index));
      row.setAttribute('role', 'option');
      row.setAttribute('data-index', String(index));
      row.setAttribute('data-language', choice?.id ?? '');
      row.setAttribute('aria-selected', String(choice === selected));
      row.append(page.createTextNode(choice?.label ?? m.editor_code_language_none()));
      return row;
    });
    if (rows.length === 0) {
      const empty = page.createElement('li');
      empty.setAttribute('class', 'doc-code-nothing');
      empty.setAttribute('role', 'presentation');
      empty.append(page.createTextNode(m.editor_code_language_nothing()));
      rows.push(empty);
    }
    this.#list.replaceChildren(...rows);
    this.#highlight(0);
  }

  #highlight(index: number): void {
    const rows = this.#list.querySelectorAll('[role="option"]');
    this.#active = rows.length === 0 ? -1 : Math.min(Math.max(index, 0), rows.length - 1);
    rows.forEach((row, at) => {
      row.toggleAttribute('data-active', at === this.#active);
    });
    const active = rows[this.#active];
    if (!active) {
      this.#field.removeAttribute('aria-activedescendant');
      return;
    }
    this.#field.setAttribute('aria-activedescendant', active.id);
    // Not every environment lays out (and so scrolls) elements.
    if (typeof active.scrollIntoView === 'function') active.scrollIntoView({ block: 'nearest' });
  }

  #key(event: KeyboardEvent): void {
    if (isComposing(event)) return;
    const count = this.#shown.length;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (count > 0)
        this.#highlight((this.#active + (event.key === 'ArrowDown' ? 1 : -1) + count) % count);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      if (this.#active >= 0) this.#pick(this.#active);
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      this.close(true);
    }
  }

  #pick(index: number): void {
    const choice = this.#shown[index];
    if (choice === undefined) return;
    this.close(true);
    if (!this.#options.editable()) return;
    this.#options.choose(choice?.id ?? null);
  }
}
