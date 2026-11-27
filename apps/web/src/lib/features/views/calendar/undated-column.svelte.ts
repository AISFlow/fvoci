/**
 * The cards without a date of a calendar or a Gantt chart, opened from
 * "No date" in the row above it: a column at the right of the view, or a
 * sheet on a compact page.
 *
 * - The column is closed when the page changes between the two, so that a
 *   device turned on its side does not raise a sheet unasked.
 * - Opened, the focus goes to its close button; closed, back to "No date".
 *   Escape in the column closes it. The focus going into the view under it
 *   (Shift+Tab out of it, an arrow key) means the reader has left it, and it
 *   closes so that nothing focused is under it (`left`).
 * - A card opened from it gets the focus put on "No date" first: given a
 *   date, it leaves the list, and that is where the card gives it back then
 *   (CardDialog gives it to the card's bar where there is one). On a sheet,
 *   the sheet closes first.
 */
import { tick } from 'svelte';

export interface UndatedHooks {
  /** The page is compact: the cards are on a sheet. */
  sheet: () => boolean;
  onOpenCard: (cardId: string) => void;
  /** The column is about to open: what else lies over the view gives way. */
  onopen?: () => void;
}

export class UndatedColumn {
  readonly #hooks: UndatedHooks;
  /** The cards without a date are open. */
  open = $derived.by<boolean>(() => this.#hooks.sheet() && false);
  /** "No date" in the row above: where the focus comes back to. */
  button = $state<HTMLElement | null>(null);
  #panel: HTMLElement | undefined;

  constructor(hooks: UndatedHooks) {
    this.#hooks = hooks;
  }

  async toggle(): Promise<void> {
    this.open = !this.open;
    if (!this.open) return;
    this.#hooks.onopen?.();
    await tick();
    this.#panel?.querySelector<HTMLElement>('[data-testid="undated-close"]')?.focus();
  }

  /** Closes the column, the focus back on "No date". */
  readonly close = (): void => {
    this.open = false;
    this.button?.focus();
  };

  /** The focus went into the view under the column. */
  left(): void {
    if (!this.#hooks.sheet()) this.open = false;
  }

  /**
   * The column: kept for the focus, and Escape in it closes it. A listener
   * and not `onkeydown`: the column is a region of the page, not a control,
   * and the key is its way out like its close button. For `{@attach}`.
   */
  readonly attach = (element: HTMLElement): (() => void) => {
    this.#panel = element;
    const keys = (event: KeyboardEvent): void => {
      if (event.key === 'Escape') this.close();
    };
    element.addEventListener('keydown', keys);
    return () => {
      element.removeEventListener('keydown', keys);
      this.#panel = undefined;
    };
  };

  /** A card opened from the list. */
  readonly openCard = async (cardId: string): Promise<void> => {
    if (this.#hooks.sheet()) {
      this.open = false;
      await tick();
    }
    this.button?.focus();
    this.#hooks.onOpenCard(cardId);
  };
}
