/**
 * What the reader's settings decide about a drag by pointer, for every list
 * that is dragged: whether the device buzzes when a resting finger has
 * picked something up.
 *
 * It is a getter because a sortable reads its options when it is attached,
 * inside the effect that attaches it: a change of the setting attaches it
 * anew. It is here and not in lib/drag because that layer does not know the
 * reader's settings (src/lib/README.md).
 */
import { display } from '$lib/display.svelte.ts';

import type { PointerDragOptions } from '$lib/drag/pointer-drag.ts';
import { DRAG_SETTINGS } from '$lib/drag/settings.ts';

export const readerPointer: Pick<PointerDragOptions, 'touchVibration'> = {
  get touchVibration(): number {
    return display.resolved.haptics ? DRAG_SETTINGS.touchVibration : 0;
  },
};
