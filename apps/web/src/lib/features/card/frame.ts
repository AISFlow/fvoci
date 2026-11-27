/**
 * Where the one dialog of a card sits in each of its three frames.
 *
 * The card is a single dialog at every stage of the page; only these classes
 * differ. Nothing inside it is put together again when the window crosses a
 * stage, so what is typed, the caret, the focus and the scroll position
 * stay. Which frame it is comes from `cardFrame`
 * (lib/layout/presentation.ts).
 *
 * Every class that differs between the frames is written out for each of
 * them, so that they override the dialog's own and one another's cleanly.
 * The classes are whole words on purpose: the style sheet is made from the
 * class names it finds in the source, and does not find one that is put
 * together from parts.
 *
 * - The panel is 30rem wide (480px), plus the room the device keeps at its
 *   right edge, and always leaves 3.5rem of the board to tap on.
 * - The dialog is at most 48rem wide (768px) and 56rem high.
 * - The height follows `--keyboard`: where the browser lays the on-screen
 *   keyboard over the page instead of shrinking it, the card ends at the top
 *   of the keyboard and the comment field, its last row, stays in sight.
 * - `--doc-surface` is the colour the frame is, which the body's tables
 *   take (editor.css).
 */
import type { CardFrame } from '$lib/layout/presentation.ts';

export const FRAME_CLASS: Record<CardFrame, string> = {
  // The whole screen, inside the edges the device keeps.
  full: 'top-0 right-auto left-0 h-[calc(100dvh-var(--keyboard))] w-full max-w-none translate-x-0 translate-y-0 rounded-none border-l-0 bg-background [--doc-surface:var(--background)] pt-(--safe-top) pr-(--safe-right) pl-(--safe-left) shadow-none ring-0 sm:max-w-none',
  // A panel at the right edge; what is left of the board beside it is dimmed and closes the card.
  panel:
    'top-0 right-0 left-auto h-[calc(100dvh-var(--keyboard))] w-[min(calc(30rem+var(--safe-right)),calc(100%-3.5rem))] max-w-none translate-x-0 translate-y-0 rounded-none border-l bg-background [--doc-surface:var(--background)] pt-(--safe-top) pr-(--safe-right) pl-0 shadow-lg ring-0 sm:max-w-none',
  // A dialog in the middle of what the keyboard and the edges of the device leave.
  dialog:
    'top-[calc((100dvh-var(--keyboard))/2)] right-auto left-[calc(50%+(var(--safe-left)-var(--safe-right))/2)] h-[min(calc(100dvh-var(--keyboard)-4rem),56rem)] w-full max-w-[min(48rem,calc(100%-2rem-var(--safe-left)-var(--safe-right)))] -translate-x-1/2 -translate-y-1/2 rounded-xl border-l-0 bg-popover [--doc-surface:var(--popover)] pt-0 pr-0 pl-0 shadow-none ring-1 sm:max-w-[min(48rem,calc(100%-2rem-var(--safe-left)-var(--safe-right)))]',
};
