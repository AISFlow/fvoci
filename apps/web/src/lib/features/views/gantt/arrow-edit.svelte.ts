/**
 * State and focus coordination for hovering, focusing, and editing Gantt
 * dependency arrows. Reconciles open editors if the arrow disappears or the
 * view becomes read-only, and prevents async races between close and re-select.
 */
import { tick } from 'svelte';

import type { ArrowLine } from './chart.svelte.ts';

export interface ArrowEditSource {
  editable: () => boolean;
  arrows: () => readonly ArrowLine[];
  hasBar: (cardId: string) => boolean;
  fallbackBar: () => string | undefined;
  scroller: () => HTMLElement | undefined;
  rem: () => number;
  closeUndated: () => void;
  focusBar: (cardId: string | undefined) => void;
}

export class ArrowEdit {
  readonly #source: ArrowEditSource;
  hovered = $state<string | undefined>(undefined);
  focused = $state<string | undefined>(undefined);
  editing = $state.raw<ArrowLine | undefined>(undefined);
  open = $state(false);
  anchor = $state.raw<HTMLElement | null>(null);

  constructor(source: ArrowEditSource) {
    this.#source = source;
    $effect(() => {
      const current = this.editing;
      if (current === undefined) return;
      const editable = source.editable();
      const stillDrawn = source.arrows().some((arrow) => arrow.key === current.key);
      if (editable && stillDrawn) return;
      this.close(current.to, current.from);
    });
  }

  readonly activeKey = $derived(this.editing?.key ?? this.focused ?? this.hovered);

  readonly setHovered = (key: string | undefined): void => {
    this.hovered = key;
  };

  readonly setFocused = (key: string | undefined): void => {
    this.focused = key;
  };

  /** Closes any open or pending editor and restores focus to a visible bar when needed. */
  readonly close = (preferredBar?: string, alternateBar?: string): void => {
    const wasActive = this.open || this.editing !== undefined;
    const to = preferredBar ?? this.editing?.to;
    const from = alternateBar ?? this.editing?.from;
    this.open = false;
    this.editing = undefined;
    this.anchor = null;
    if (!wasActive) return;
    void tick().then(() => {
      if (this.editing !== undefined || this.open) return;
      const active = document.activeElement;
      if (active === null || active === document.body) {
        this.#focusVisibleBar(to, from);
      }
    });
  };

  readonly select = (arrow: ArrowLine, anchor: HTMLElement): void => {
    if (!this.#source.editable()) return;
    this.#source.closeUndated();
    this.anchor = anchor;
    this.editing = arrow;
    void tick().then(() => {
      if (this.editing?.key === arrow.key) this.open = true;
    });
  };

  /** Brings a keyboard-focused arrow button out from under the sticky title column. */
  readonly focusedButton = (button: HTMLElement): void => {
    const scroller = this.#source.scroller();
    if (!scroller) return;
    const titlesRight = scroller
      .querySelector('[data-gantt-corner]')
      ?.getBoundingClientRect().right;
    const hidden = (titlesRight ?? 0) - button.getBoundingClientRect().left;
    if (hidden > 0) scroller.scrollLeft -= hidden + this.#source.rem() / 2;
  };

  /** Returns focus to the arrow button after save/cancel, or to a visible bar after removal. */
  readonly closed = (removed: boolean, toCardId: string | undefined, fromCardId?: string): void => {
    const back = this.anchor;
    this.anchor = null;
    void tick().then(() => {
      if (this.editing !== undefined || this.open) return;
      if (!removed && back?.isConnected) back.focus({ preventScroll: true });
      else this.#focusVisibleBar(toCardId, fromCardId);
    });
  };

  #focusVisibleBar(preferred?: string, alternate?: string): void {
    const target =
      (preferred !== undefined && this.#source.hasBar(preferred) ? preferred : undefined) ??
      (alternate !== undefined && this.#source.hasBar(alternate) ? alternate : undefined) ??
      this.#source.fallbackBar();
    this.#source.focusBar(target);
  }
}
