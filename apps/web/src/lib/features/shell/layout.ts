/**
 * How the shell lays the sidebar out, from the shape the stage asks for and
 * what the reader did with it.
 *
 * The list of boards is one element in every case; only where it sits
 * differs. `beside`: a column of the page that the board stands next to.
 * `over`: laid over the page from the left edge, out of sight until it is
 * opened. The strip is a second, narrow column that stays when the list is
 * not beside the board, and is how a medium page keeps every board one tap
 * away.
 */
import type { SidebarShape } from '$lib/layout/presentation.ts';

export interface ShellLayout {
  /** Where the list of boards is. */
  list: 'beside' | 'over';
  /** The list can be slid in over the page (and so something opens it that way). */
  drawer: boolean;
  /** The narrow column with one button a board is shown. */
  strip: boolean;
  /** The top bar has the button that opens the drawer: nothing else on the page does. */
  menuButton: boolean;
}

/**
 * `collapsed` is the reader's choice on an expanded page to fold the fixed
 * sidebar down to the strip. It means nothing on the other stages, and is
 * kept through them.
 */
export function shellLayout(shape: SidebarShape, collapsed: boolean): ShellLayout {
  switch (shape) {
    case 'drawer':
      return { list: 'over', drawer: true, strip: false, menuButton: true };
    case 'strip':
      return { list: 'over', drawer: true, strip: true, menuButton: false };
    case 'fixed':
      return collapsed
        ? { list: 'over', drawer: false, strip: true, menuButton: false }
        : { list: 'beside', drawer: false, strip: false, menuButton: false };
  }
}
