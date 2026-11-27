/**
 * How things that open over the page are presented at each stage.
 *
 * This is the one place that decides it. A menu, a picker, the editor of a
 * property and the frame of a card all ask here, so a different answer (a
 * popover on a compact page after all, a card that is a pane beside the
 * board) is a change of this file.
 */
import type { CardOpenSetting } from '../logic/display.ts';

import type { Stage } from './room.svelte.ts';

/** A menu or a picker: on a sheet that rises from the bottom edge, or next to what opened it. */
export type OverlayPresentation = 'sheet' | 'popover';

/**
 * Menus and pickers: a sheet on a compact page, a popover otherwise, without
 * exceptions.
 */
export function overlayPresentation(stage: Stage): OverlayPresentation {
  return stage === 'compact' ? 'sheet' : 'popover';
}

/** The frame an open card is drawn in. */
export type CardFrame = 'full' | 'panel' | 'dialog';

/**
 * An open card: the whole screen on a compact page, a panel at the side on a
 * medium one, and on an expanded one what the reader chose. Of the choices
 * only the dialog in the middle is built; the others are shown as it.
 */
export function cardFrame(stage: Stage, _setting: CardOpenSetting): CardFrame {
  if (stage === 'compact') return 'full';
  if (stage === 'medium') return 'panel';
  // `setting` is `center`, `side` or `full`. Until the other two are built
  // (lib/logic/display.ts, CARD_OPEN_BUILT) every one of them is the dialog.
  return 'dialog';
}

/** The shape of the sidebar. */
export type SidebarShape = 'drawer' | 'strip' | 'fixed';

/** The sidebar: a drawer over the page, a strip that opens a drawer, or fixed beside the board. */
export function sidebarShape(stage: Stage): SidebarShape {
  if (stage === 'compact') return 'drawer';
  return stage === 'medium' ? 'strip' : 'fixed';
}
