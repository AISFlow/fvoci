/**
 * Changing the width of a column: by dragging the handle at the edge of its
 * heading, by the arrow keys on that handle, or by "Wider" and "Narrower" of
 * the heading's menu.
 *
 * While the handle is dragged the new width is only shown (`live`); it is
 * stored once, when the handle is let go, so that one drag is one change to
 * undo. Every change starts from the width the column shows, which for the
 * title column can be less than the stored one: it never takes more than
 * half of the pane.
 */
import { pane } from '$lib/layout/room.svelte.ts';
import { TITLE_COLUMN_ID } from '$lib/logic/wire.ts';

import { clampWidth, TABLE_SETTINGS } from './columns.ts';

export interface ResizeHandlers {
  /** The handle was taken hold of. */
  onResizeStart: () => void;
  /** The handle was dragged to this width; nothing is stored yet. */
  onResizePreview: (width: number) => void;
  /** The handle was let go: store the last width, or forget it. */
  onResizeEnd: (commit: boolean) => void;
  /** An arrow key on the handle: one step wider (1) or narrower (-1). */
  onResizeStep: (direction: 1 | -1) => void;
}

export interface ResizeSource {
  /** The heading of a column, to read the width it shows. */
  heading: (columnId: string) => HTMLElement | undefined;
  /** Stores the width of a column in the view. */
  save: (columnId: string, width: number) => void;
}

export class ColumnResize {
  /** The column being dragged wider or narrower: its width now, and before. */
  live = $state.raw<{ id: string; width: number; from: number } | undefined>(undefined);
  readonly #source: ResizeSource;

  constructor(source: ResizeSource) {
    this.#source = source;
  }

  /** The widest a column may be made now: half of the pane for the title column. */
  #cap(id: string): number | undefined {
    if (id !== TITLE_COLUMN_ID) return undefined;
    return pane.measured ? pane.width / 2 : undefined;
  }

  #shown(id: string): number | undefined {
    return this.#source.heading(id)?.getBoundingClientRect().width;
  }

  /** Makes a column wider or narrower by `delta` pixels, from the width it shows. */
  by(id: string, delta: number): void {
    const shown = this.#shown(id);
    if (shown === undefined) return;
    const next = clampWidth(shown + delta, this.#cap(id));
    if (next !== Math.round(shown)) this.#source.save(id, next);
  }

  /** What the handle of a heading calls. */
  handlers(id: string): ResizeHandlers {
    return {
      onResizeStart: () => {
        const shown = this.#shown(id);
        if (shown === undefined) return;
        this.live = { id, width: Math.round(shown), from: Math.round(shown) };
      },
      onResizePreview: (width) => {
        const live = this.live;
        if (live?.id === id) this.live = { ...live, width: clampWidth(width, this.#cap(id)) };
      },
      onResizeEnd: (commit) => {
        const live = this.live;
        this.live = undefined;
        if (commit && live?.id === id && live.width !== live.from) {
          this.#source.save(id, live.width);
        }
      },
      onResizeStep: (direction) => {
        this.by(id, direction * TABLE_SETTINGS.keyStep);
      },
    };
  }
}
