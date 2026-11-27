/**
 * Where the focus is on a Gantt chart, and where it goes: the bars are one
 * stop of the Tab key (the one last focused, the first until then), the
 * arrow keys walk them, a card opened from the chart gives the focus back
 * to what opened it, and the focus is not lost when the rows are laid out
 * again.
 */
import { tick } from 'svelte';

import type { GanttChart } from './chart.svelte.ts';

export interface FocusSource {
  chart: () => GanttChart;
  /** The box that scrolls; undefined while it is not drawn. */
  scroller: () => HTMLElement | undefined;
  /** "No date" in the row above: where the focus goes when no bar is left. */
  undatedButton: () => HTMLElement | null;
  onOpenCard: (cardId: string) => void;
  /** The size of 1rem in pixels. */
  rem: () => number;
}

export class BarFocus {
  readonly #source: FocusSource;
  /** The bar the Tab key stops on: the one last focused, the first until then. */
  #focused = $state<string | undefined>(undefined);
  readonly stop = $derived.by(() => {
    const chart = this.#source.chart();
    const focused = this.#focused;
    return focused !== undefined && chart.bars.has(focused) ? focused : chart.cardLines[0]?.id;
  });

  /**
   * The card whose title opened it. The card gives the focus back to what
   * opened it, and CardDialog would hand it on to the card's bar (`data-card`)
   * and so scroll the days to wherever that bar is: for this card the bar
   * does not say it is the card's, and the focus stays on the title.
   */
  viaTitle = $state<string | undefined>(undefined);

  /**
   * The card opened last from the chart. Should it lose its bar while it is
   * open (its date cleared in the card), "No date" says it is the card's
   * (`data-card`), and the card gives the focus back to it when it closes:
   * that is where the card is listed now.
   */
  #opened = $state<string | undefined>(undefined);
  readonly openedUndated = $derived.by(() => {
    const opened = this.#opened;
    return opened !== undefined && !this.#source.chart().bars.has(opened) ? opened : undefined;
  });

  /**
   * The bar the reader was sent to from a count above the chart. It shows
   * its ring also where the focus came by a press of a pointer, which draws
   * none by itself: the reader has to see which bar is meant.
   */
  pointed = $state<string | undefined>(undefined);

  /** The cards of the rows as they were drawn last, from the top. Not reactive. */
  #shown: string[] = [];

  /** To be made while a component is: it keeps the focus from then on. */
  constructor(source: FocusSource) {
    this.#source = source;
    /*
     * The focus is not lost when the rows are laid out again. The rows stand
     * in blocks, and a card added or taken away above moves rows from one
     * block to the next: such a row is made again, and the focus that was on
     * its bar or its title would fall to the page. So would the focus on the
     * bar of a card that leaves the chart (deleted, or its date cleared, by
     * someone else). Before the rows are drawn what has the focus is noted;
     * afterwards, if the page has it, it goes back to the same bar or title,
     * else to the bar that now stands in that place, else to "No date".
     */
    $effect.pre(() => {
      // Read, so that this runs again whenever the rows are laid out anew.
      const lines = source.chart().cardLines;
      const before = this.#shown;
      this.#shown = lines.map((line) => line.id);
      const scroller = source.scroller();
      const active = document.activeElement;
      const row = active instanceof HTMLElement ? active.closest<HTMLElement>('[data-row]') : null;
      const cardId = row?.dataset.row;
      if (!row || cardId === undefined || !scroller?.contains(row)) return;
      const part = active?.hasAttribute('data-row-title')
        ? '[data-row-title]'
        : active?.hasAttribute('data-hierarchy-toggle')
          ? '[data-hierarchy-toggle]'
          : '[data-bar]';
      const at = Math.max(0, before.indexOf(cardId));
      void tick().then(() => {
        if (document.activeElement !== document.body) return;
        const same = scroller.querySelector<HTMLElement>(
          `[data-row="${CSS.escape(cardId)}"] ${part}`,
        );
        if (same) same.focus({ preventScroll: true });
        else if (lines.length > 0) this.focusBar(lines[Math.min(at, lines.length - 1)]?.id);
        else source.undatedButton()?.focus();
      });
    });
  }

  /** The title of a row was pressed: the Tab key goes on from this row, and its card opens. */
  readonly openFromTitle = (cardId: string): void => {
    this.#focused = cardId;
    this.viaTitle = cardId;
    this.openCard(cardId);
  };

  readonly openCard = (cardId: string): void => {
    this.#opened = cardId;
    this.#source.onOpenCard(cardId);
  };

  /**
   * A bar took the focus: the stop of the Tab key is on it from now on. The
   * page brings a focused bar into the box by itself, but knows nothing of
   * the column of titles that lies over the box's left: a bar that begins
   * under it is scrolled out from under it. (Room kept at the left by the
   * style sheet, `scroll-padding`, would do it too, and would scroll the
   * days away whenever a title took the focus: the titles lie in that room.)
   */
  readonly barFocused = (cardId: string, bar: HTMLElement): void => {
    this.#focused = cardId;
    this.viaTitle = undefined;
    if (this.pointed !== cardId) this.pointed = undefined;
    const scroller = this.#source.scroller();
    if (!scroller) return;
    const titles = scroller.querySelector('[data-gantt-corner]')?.getBoundingClientRect().right;
    const hidden = (titles ?? 0) - bar.getBoundingClientRect().left;
    if (hidden > 0) scroller.scrollLeft -= hidden + this.#source.rem() / 2;
  };

  focusBar(cardId: string | undefined): void {
    if (cardId === undefined) return;
    this.#focused = cardId;
    this.#source
      .scroller()
      ?.querySelector<HTMLElement>(`[data-bar="${CSS.escape(cardId)}"]`)
      ?.focus();
  }

  /**
   * Goes to the next card with a mark below the bar last focused, whether
   * that bar has the mark or not, and from the last on to the first.
   */
  focusNext(mark: 'late' | 'loop'): void {
    const lines = this.#source.chart().cardLines;
    const from = lines.findIndex((line) => line.id === this.#focused);
    const next =
      lines.slice(from + 1).find((line) => line[mark]) ?? lines.find((line) => line[mark]);
    this.pointed = next?.id;
    this.focusBar(next?.id);
  }

  /** The arrow keys walk the bars: up and down a row, Home and End to the first and last. */
  readonly barKeys = (event: KeyboardEvent, cardId: string): void => {
    if (event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
    const lines = this.#source.chart().cardLines;
    const at = lines.findIndex((line) => line.id === cardId);
    const to = { ArrowUp: at - 1, ArrowDown: at + 1, Home: 0, End: lines.length - 1 }[event.key];
    if (to === undefined) return;
    event.preventDefault();
    this.focusBar(lines[Math.min(lines.length - 1, Math.max(0, to))]?.id);
  };
}
