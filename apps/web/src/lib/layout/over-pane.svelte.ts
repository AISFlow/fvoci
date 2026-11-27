/**
 * A drawer of the screen in the pane, out over that screen.
 *
 * What the shell draws round the pane (its top bar, the sidebar's strip) is no
 * part of the screen, which cannot keep it away from inside: the screen says
 * here that its drawer is out, and the shell makes those parts inert for as
 * long as it is, as the screen does with the rest of itself.
 */
import { untrack } from 'svelte';

/**
 * How many such drawers are out. A count the screen keeps up, and not its own
 * answer asked from here: what a screen derives is not to be read once the
 * screen is on its way out, when what it was given is gone.
 */
let out = $state(0);

/** Whether a drawer of the screen in the pane is out over it. */
export function paneCovered(): boolean {
  return out > 0;
}

/**
 * Call while a component is being set up: `isOut` says whether its drawer is
 * out. Counted while it is and the component is there.
 */
export function coverPane(isOut: () => boolean): void {
  $effect.pre(() => {
    if (!isOut()) return;
    untrack(() => {
      out += 1;
    });
    return () => {
      out -= 1;
    };
  });
}
