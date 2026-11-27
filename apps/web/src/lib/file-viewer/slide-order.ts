/**
 * A deck's slides as the viewer turns through them, its hidden slides
 * (PowerPoint's "Hide Slide", `p:sld@show="0"`) left out or shown in place.
 *
 * A slide keeps its number, its place in the file, as PowerPoint numbers
 * it: with the hidden slides left out, the numbers jump where they were.
 * The viewer opens on the slides a show would give; when every slide is
 * hidden there would be none, so they are all shown. Both the app (what is
 * counted and turned to) and the frame (what is laid out) go by this.
 * A module without imports: the frame's chunk takes it as it is.
 */
export class SlideOrder {
  /** How many slides the deck has, hidden or not. */
  readonly pages: number;
  /** The hidden slides, by number (from 1). */
  readonly hidden: ReadonlySet<number>;

  /** `hidden` are numbers from 1 to `pages`; others are left out. */
  constructor(pages: number, hidden: Iterable<number>) {
    this.pages = Math.max(0, Math.floor(pages));
    this.hidden = new Set(
      [...hidden].filter((page) => Number.isInteger(page) && page >= 1 && page <= this.pages),
    );
  }

  /** Every slide is hidden: there is nothing to leave them out for. */
  get allHidden(): boolean {
    return this.pages > 0 && this.hidden.size === this.pages;
  }

  /** Whether slide `page` is laid out, with the hidden slides shown (`expanded`) or not. */
  shows(page: number, expanded: boolean): boolean {
    if (!Number.isInteger(page) || page < 1 || page > this.pages) return false;
    return expanded || this.allHidden || !this.hidden.has(page);
  }

  /** How many slides are laid out. */
  count(expanded: boolean): number {
    return expanded || this.allHidden ? this.pages : this.pages - this.hidden.size;
  }

  /** The first slide laid out; 1 for a deck of none. */
  first(expanded: boolean): number {
    return this.next(0, 1, expanded) ?? 1;
  }

  /** The slide laid out after (`step` 1) or before (-1) `page`; undefined past the end. */
  next(page: number, step: 1 | -1, expanded: boolean): number | undefined {
    for (let at = page + step; at >= 1 && at <= this.pages; at += step) {
      if (this.shows(at, expanded)) return at;
    }
    return undefined;
  }

  /**
   * Where the reader is once the hidden slides are shown or left out: the
   * same slide when it is laid out, else the next one laid out after it,
   * else the one before it.
   */
  landing(page: number, expanded: boolean): number {
    if (this.shows(page, expanded)) return page;
    return this.next(page, 1, expanded) ?? this.next(page, -1, expanded) ?? this.first(expanded);
  }
}
