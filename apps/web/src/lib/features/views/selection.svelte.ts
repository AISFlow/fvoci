/**
 * The cards selected in a view of a board, to be changed, moved or deleted
 * together: the kanban, the table and the gallery have one each, and the bar
 * that acts on it (BulkBar.svelte) is the same for all of them.
 *
 * With a mouse a card is taken in or out with Ctrl or ⌘ and a range with
 * Shift. A finger has neither: "Select" in the menu of a card turns on a
 * state in which a plain press selects (`selecting`), until the selection is
 * empty again. Escape lets it go.
 *
 * No more than `CARD_BATCH_LIMIT` cards are selected: what one request of
 * the board's batch routes takes. A press that would take in more takes in
 * what fits, and `full` says so until the selection changes again.
 */
import { untrack } from 'svelte';
import { on } from 'svelte/events';

import { primaryModifier } from '$lib/keys.ts';
import { CARD_BATCH_LIMIT } from '$lib/state/index.ts';

/** What a press on a card asks for: the card opened, taken into the selection, or the range up to it. */
export type Activation = 'open' | 'toggle' | 'range';

/**
 * What a click or Enter on a card asks for: the platform's Ctrl or ⌘ takes
 * it in or out, Shift takes the range up to it. The other platform's
 * modifier is no command on a card (undefined).
 */
export function activationOf(event: MouseEvent | KeyboardEvent): Activation | undefined {
  if ((event.ctrlKey || event.metaKey) && !primaryModifier(event)) return undefined;
  if (primaryModifier(event)) return 'toggle';
  return event.shiftKey ? 'range' : 'open';
}

/** A selection with one card taken in, or taken out when it was in. */
export function toggled(selected: readonly string[], cardId: string): string[] {
  return selected.includes(cardId) ? selected.filter((id) => id !== cardId) : [...selected, cardId];
}

/**
 * A selection grown by a range: every card from `anchor` to `cardId` in the
 * order the view shows them (a board column by column, top to bottom). Without
 * an anchor in the view the card alone is added.
 */
export function withRange(
  order: readonly string[],
  selected: readonly string[],
  anchor: string | undefined,
  cardId: string,
): string[] {
  const from = anchor === undefined ? -1 : order.indexOf(anchor);
  const to = order.indexOf(cardId);
  if (to === -1) return [...selected];
  const range = from === -1 ? [cardId] : order.slice(Math.min(from, to), Math.max(from, to) + 1);
  return [...selected, ...range.filter((id) => !selected.includes(id))];
}

/**
 * A range grown past the limit, cut to it: what was selected, then the
 * cards of the range nearest `anchor`, whichever way the range went; in the
 * order of `grown`.
 */
function nearAnchor(
  order: readonly string[],
  selected: readonly string[],
  grown: readonly string[],
  anchor: string | undefined,
): string[] {
  const at = anchor === undefined ? -1 : order.indexOf(anchor);
  const away = (id: string): number => Math.abs(order.indexOf(id) - at);
  const added = grown.filter((id) => !selected.includes(id)).sort((a, b) => away(a) - away(b));
  const kept = new Set([...selected, ...added].slice(0, CARD_BATCH_LIMIT));
  return grown.filter((id) => kept.has(id));
}

export class Selection {
  #selected = $state.raw<readonly string[]>([]);
  #selecting = $state(false);
  /** The last press would have taken in more cards than may be selected. */
  full = $state(false);
  /** The card a range is measured from: the one pressed last. */
  #anchor: string | undefined;
  readonly #shown: () => readonly string[];

  /** `shown`: the cards of the view in the order it shows them. */
  constructor(shown: () => readonly string[]) {
    this.#shown = shown;
  }

  /** The selected cards the view shows. One that left the view (deleted, filtered out) is not counted. */
  readonly ids = $derived.by(() => {
    const shown = this.#shown();
    return this.#selected.filter((id) => shown.includes(id));
  });

  /**
   * A plain press selects instead of opening: after "Select", while a card
   * of the selection is in the view. One that left it (filtered out) ends
   * that too.
   */
  get selecting(): boolean {
    return this.#selecting && this.ids.length > 0;
  }

  has(cardId: string): boolean {
    return this.ids.includes(cardId);
  }

  /** `cut`: more were asked for than fit, though `next` may have been cut to fit already. */
  #set(next: readonly string[], cut = next.length > CARD_BATCH_LIMIT): void {
    this.full = cut;
    this.#selected = next.length > CARD_BATCH_LIMIT ? next.slice(0, CARD_BATCH_LIMIT) : next;
    if (next.length === 0) this.#selecting = false;
  }

  /** Takes a card in, or out when it was in. */
  toggle(cardId: string): void {
    this.#set(toggled(this.ids, cardId));
    this.#anchor = cardId;
  }

  /** Takes in every card from the one pressed last to this one. */
  extendTo(cardId: string): void {
    const order = this.#shown();
    const grown = withRange(order, this.ids, this.#anchor, cardId);
    if (grown.length > CARD_BATCH_LIMIT) {
      this.#set(nearAnchor(order, this.ids, grown, this.#anchor), true);
    } else {
      this.#set(grown);
    }
    this.#anchor = cardId;
  }

  /**
   * A press on a card, as `activationOf` reads it. Says whether the
   * selection took it: a plain press opens the card, unless `selecting`.
   */
  press(cardId: string, how: Activation): boolean {
    if (how === 'open' && !this.selecting) return false;
    if (how === 'range') this.extendTo(cardId);
    else this.toggle(cardId);
    return true;
  }

  /** "Select" in the menu of a card: from then on a plain press selects. */
  begin(cardId: string): void {
    this.#selecting = true;
    this.toggle(cardId);
  }

  clear(): void {
    this.#set([]);
    this.#anchor = undefined;
  }

  /**
   * Attaches the selection to the root of its view. Escape there lets it go
   * (`attachEscape`), and a card that leaves the view (filtered out, its
   * column hidden, its group folded, deleted) leaves the selection: it does
   * not come back selected with the card.
   */
  readonly attach = (root: HTMLElement): (() => void) => {
    $effect(() => {
      this.#keep(this.#shown());
    });
    return this.attachEscape(root);
  };

  /** Drops the selected cards that `shown` has not. */
  #keep(shown: readonly string[]): void {
    untrack(() => {
      const kept = this.#selected.filter((id) => shown.includes(id));
      if (kept.length < this.#selected.length) this.#set(kept);
    });
  }

  /**
   * Escape in the view lets the selection go, unless something else (a drag,
   * a menu, the end of editing a cell) took the key. With `on`, which runs
   * after the handlers the view declares (`onkeydown`): those are delegated,
   * and a listener of the element's own would hear the key before them.
   */
  readonly attachEscape = (root: HTMLElement): (() => void) =>
    on(root, 'keydown', (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented || this.ids.length === 0) return;
      this.clear();
    });
}
