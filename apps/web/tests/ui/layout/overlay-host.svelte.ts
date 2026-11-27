/** Something that can be open and closes on "back", for the tests of the history. */
import { closeOnBack } from '$lib/layout/back.svelte.ts';

export interface Overlay {
  open: boolean;
  /** How often "back" closed it. */
  closedByBack: number;
  destroy(): void;
}

export function overlay(): Overlay {
  let open = $state(false);
  let closedByBack = $state(0);
  const destroy = $effect.root(() => {
    closeOnBack(
      () => open,
      () => {
        open = false;
        closedByBack += 1;
      },
    );
  });
  return {
    get open() {
      return open;
    },
    set open(next: boolean) {
      open = next;
    },
    get closedByBack() {
      return closedByBack;
    },
    destroy,
  };
}
