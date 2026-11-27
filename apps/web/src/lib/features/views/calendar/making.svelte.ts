/**
 * Making a card on a day of the calendar: the values the
 * view's filter asks for and the day (`newCardProperties`), then the card
 * opens, as a card made on a board does.
 *
 * - One at a time: a second press while the first card is being made makes
 *   nothing (a double click on a "+" made two cards, each its own undo).
 * - The focus goes to the day before the card opens. A new card asks for its
 *   title, which takes the focus before the dialog notes where it came from;
 *   where the card then has no bar to give it back to (the small grid, a
 *   finger, a filter that hides it), the dialog leaves it nowhere. Once the
 *   card has closed, a focus left on nothing goes to the day.
 *
 * The view header's "New" (and its round button on a folded header) makes
 * a card here too, on the day picked: the calendar names this as its maker
 * to the board page (lib/state/new-card.ts).
 */
import { tick } from 'svelte';

import { newCardProperties } from '$lib/logic/calendar.ts';
import type { CalendarDate } from '$lib/logic/dates.ts';
import type { BoardView, PropertyTemplate } from '$lib/logic/wire.ts';
import { whileHere, type BoardStore } from '$lib/state/index.ts';

interface Context {
  board: BoardStore;
  view: BoardView;
  templates: readonly PropertyTemplate[];
  template: PropertyTemplate | undefined;
}

interface Hooks {
  /** The calendar goes to the day. */
  select: (date: CalendarDate) => void;
  /** Puts the focus on the picked day. */
  focusDay: () => void;
  onOpenCard: (cardId: string) => void;
}

export class CardMaker {
  readonly #context: () => Context;
  readonly #hooks: Hooks;
  #busy = false;
  /** The card made here that is open, or about to be; whether it has been seen open. */
  #made: string | undefined;
  #seen = false;

  /** Made while a component starts: it watches the open card for as long as the component lives. */
  constructor(context: () => Context, hooks: Hooks) {
    this.#context = context;
    this.#hooks = hooks;
    $effect(() => {
      const open = context().board.openCard?.cardId;
      if (this.#made === undefined) return;
      if (open === this.#made) {
        this.#seen = true;
      } else if (this.#seen) {
        this.#made = undefined;
        this.#seen = false;
        void this.#refocus();
      }
    });
  }

  async on(date: CalendarDate): Promise<void> {
    if (this.#busy) return;
    this.#busy = true;
    try {
      const { board, view, templates, template } = this.#context();
      const properties = newCardProperties(view.fields.filter, templates, template, date);
      const card = await whileHere(board, () => board.createCard({ properties }));
      if (!card) return;
      this.#hooks.select(date);
      await tick();
      this.#hooks.focusDay();
      this.#made = card.id;
      this.#seen = false;
      this.#hooks.onOpenCard(card.id);
    } finally {
      this.#busy = false;
    }
  }

  /** Waits for the dialog to be gone; a focus it left on nothing goes to the day. */
  async #refocus(): Promise<void> {
    for (let frame = 0; frame < 60; frame += 1) {
      await new Promise((done) => requestAnimationFrame(done));
      if (document.querySelector('[role="dialog"]')) continue;
      if (document.activeElement === document.body) this.#hooks.focusDay();
      return;
    }
  }
}
