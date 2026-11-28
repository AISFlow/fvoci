/**
 * A row a finger swipes to the left to put it away (a notification out of
 * the inbox, or back into it from the archive), as a phone's mail does. The
 * row follows the finger, left only; let go past `threshold` pixels it is
 * swiped (`onSwiped`), else it slides back. A finger that goes more up or
 * down than sideways scrolls the list and moves no row (`touch-action:
 * pan-y`, set here). Only a touch swipes: a mouse or a pen selects and
 * clicks as on any row, and the row's menu does the same for any pointer.
 * The click a swipe ends with does not open the row.
 */
import type { Attachment } from 'svelte/attachments';

/** How far a finger goes before it is taken to move sideways, or not. */
const SLOP = 8;

export function swipeAway(onSwiped: () => void, threshold = 96): Attachment<HTMLElement> {
  return (row) => {
    let start: { x: number; y: number; pointerId: number } | undefined;
    let sideways: boolean | undefined;
    let moved = 0;
    let swallowClick = false;
    row.style.touchAction = 'pan-y';

    const back = (): void => {
      row.style.translate = '';
      start = undefined;
      sideways = undefined;
      moved = 0;
    };

    const onDown = (event: PointerEvent): void => {
      if (event.pointerType !== 'touch' || !event.isPrimary) return;
      start = { x: event.clientX, y: event.clientY, pointerId: event.pointerId };
      swallowClick = false;
    };

    const onMove = (event: PointerEvent): void => {
      if (start?.pointerId !== event.pointerId) return;
      const x = event.clientX - start.x;
      const y = event.clientY - start.y;
      if (sideways === undefined && Math.hypot(x, y) > SLOP) sideways = Math.abs(x) > Math.abs(y);
      if (!sideways) return;
      moved = Math.min(0, x);
      row.style.translate = `${String(moved)}px 0`;
    };

    const onUp = (event: PointerEvent): void => {
      if (start?.pointerId !== event.pointerId) return;
      const swiped = sideways === true && moved <= -threshold;
      swallowClick = sideways === true;
      back();
      if (swiped) onSwiped();
    };

    const onCancel = (event: PointerEvent): void => {
      if (start?.pointerId === event.pointerId) back();
    };

    const onClick = (event: MouseEvent): void => {
      if (!swallowClick) return;
      swallowClick = false;
      event.preventDefault();
      event.stopPropagation();
    };

    row.addEventListener('pointerdown', onDown);
    row.addEventListener('pointermove', onMove);
    row.addEventListener('pointerup', onUp);
    row.addEventListener('pointercancel', onCancel);
    row.addEventListener('click', onClick, { capture: true });
    return () => {
      row.removeEventListener('pointerdown', onDown);
      row.removeEventListener('pointermove', onMove);
      row.removeEventListener('pointerup', onUp);
      row.removeEventListener('pointercancel', onCancel);
      row.removeEventListener('click', onClick, { capture: true });
      row.style.touchAction = '';
      row.style.translate = '';
    };
  };
}
