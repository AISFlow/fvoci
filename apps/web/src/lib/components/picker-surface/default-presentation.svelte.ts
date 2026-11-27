/**
 * A default for how pickers open: a sheet when the page is compact, a
 * popover otherwise. The surface itself never looks at the page; a caller
 * that has nothing better to go by passes `current` to it.
 *
 * The rule is the one of every menu and picker and lives in
 * lib/layout/presentation.ts. Change it there.
 */
import { overlayPresentation } from '$lib/layout/presentation.ts';
import { page } from '$lib/layout/room.svelte.ts';

import type { Presentation } from './presentation.ts';

export class PickerPresentation {
  get current(): Presentation {
    return overlayPresentation(page.stage);
  }
}
