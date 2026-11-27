/**
 * Whether the layout readout is shown: a small label in a corner with the
 * width of the window, the stage and the width of the pane, for whoever is
 * working on the layout.
 *
 * Off unless asked for. It is turned on and off by the switch in the
 * settings and by the address (`?dev=1`, `?dev=0`), and the device remembers
 * the last answer.
 */

import { devParam } from './dev-param.ts';

/** Where the device remembers it. */
export const DEV_STORAGE_KEY = 'fvoci:dev';

function remembered(): boolean {
  try {
    return localStorage.getItem(DEV_STORAGE_KEY) === '1';
  } catch {
    return false;
  }
}

class DevReadout {
  shown = $state(false);

  /** Settles the answer for this visit: the address first, then what the device remembers. */
  start(search: string): void {
    const asked = devParam(search);
    if (asked === undefined) this.shown = remembered();
    else this.set(asked);
  }

  set(shown: boolean): void {
    this.shown = shown;
    try {
      localStorage.setItem(DEV_STORAGE_KEY, shown ? '1' : '0');
    } catch {
      // Not remembered: it still holds for this visit.
    }
  }
}

export const devReadout = new DevReadout();
