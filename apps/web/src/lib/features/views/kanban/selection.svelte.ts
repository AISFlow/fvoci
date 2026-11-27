/**
 * The cards selected on a board, to be dragged or moved together.
 *
 * With a mouse a card is taken in or out with Ctrl or ⌘ and a range with
 * Shift. A finger has neither: "Select" in the menu of a card turns on a
 * state in which a plain press selects (`selecting`), until the selection is
 * empty again.
 */
import { toggled, withRange } from './kanban.ts';

export class Selection {
  #selected = $state.raw<readonly string[]>([]);
  /** A plain press selects instead of opening. */
  selecting = $state(false);
  /** The card a range is measured from: the one pressed last. */
  #anchor: string | undefined;
  readonly #shown: () => readonly string[];

  /** `shown`: the cards of the board in the order it shows them, column by column. */
  constructor(shown: () => readonly string[]) {
    this.#shown = shown;
  }

  /** The selected cards the board shows. One that left the view (deleted, filtered out) is not counted. */
  readonly ids = $derived.by(() => {
    const shown = this.#shown();
    return this.#selected.filter((id) => shown.includes(id));
  });

  #set(next: readonly string[]): void {
    this.#selected = next;
    if (next.length === 0) this.selecting = false;
  }

  /** Takes a card in, or out when it was in. */
  toggle(cardId: string): void {
    this.#set(toggled(this.ids, cardId));
    this.#anchor = cardId;
  }

  /** Takes in every card from the one pressed last to this one. */
  extendTo(cardId: string): void {
    this.#set(withRange(this.#shown(), this.ids, this.#anchor, cardId));
    this.#anchor = cardId;
  }

  /** "Select" in the menu of a card: from then on a plain press selects. */
  begin(cardId: string): void {
    this.selecting = true;
    this.toggle(cardId);
  }

  clear(): void {
    this.#set([]);
    this.#anchor = undefined;
  }
}
