/**
 * Makes the back button (and the back gesture) close what is open instead of
 * leaving the page.
 *
 * Opening adds one entry to the history; going back removes it and closes the
 * thing. When the thing is closed some other way (its own button, a tap
 * outside), the entry is taken back out so that the next "back" does what the
 * person expects. Things opened on top of one another close in reverse order.
 *
 * This is for what has no address of its own: the drawer, sheets. A card has
 * an address, and the router opens and closes it. The entries made here keep
 * the address they were made on, so the router sees no change in them.
 */

/** One thing that is open: how it is closed, and whether its entry is in the history yet. */
interface Open {
  close: () => void;
  entered: boolean;
}

const opened: Open[] = [];
/**
 * How many steps back of our own are on their way: asked for, their
 * `popstate` not here yet. Such a `popstate` closes nothing.
 */
let ownSteps = 0;
/**
 * What waits for those steps to arrive before it writes to the history, in
 * the order it was asked for.
 *
 * `history.back()` happens later, and an entry added in the meantime can be
 * counted from the wrong place: the browser may have fixed where the step
 * lands before it heard of the new entry. The step then goes one entry too
 * far, and the next thing to close takes the reader out of the card, or off
 * the page. So nothing of ours is written while a step of ours is under way.
 */
const waiting: (() => void)[] = [];
/** How long writes wait for a step back at most, in milliseconds: a step that never arrives must not hold them for good. */
const WAIT_LIMIT = 2000;
let waitTimer: ReturnType<typeof setTimeout> | undefined;
let listening = false;

const MARK = 'fvociOverlay';

function ensureListening(): void {
  if (listening) return;
  window.addEventListener('popstate', onPopState);
  listening = true;
}

/** Whether a history state is one of the entries this file makes. */
export function isOverlayEntry(state: unknown): boolean {
  return typeof state === 'object' && state !== null && MARK in state;
}

function onWaitTimeout(): void {
  waitTimer = undefined;
  ownSteps = 0;
  runWaiting();
}

function scheduleWaitLimit(): void {
  clearTimeout(waitTimer);
  waitTimer = setTimeout(onWaitTimeout, WAIT_LIMIT);
}

function runWaiting(): void {
  clearTimeout(waitTimer);
  waitTimer = undefined;
  while (ownSteps === 0 && waiting.length > 0) {
    const write = waiting.shift();
    write?.();
  }
  if (waiting.length > 0) {
    waitTimer ??= setTimeout(onWaitTimeout, WAIT_LIMIT);
  }
}

/**
 * Steps back `steps` history entries of our own (`history.back()` or `history.go(-steps)`),
 * holding subsequent `afterOwnSteps` writes until the `popstate` arrives.
 */
export function stepBack(steps = 1): void {
  if (steps <= 0) return;
  ensureListening();
  ownSteps += 1;
  scheduleWaitLimit();
  if (steps === 1) history.back();
  else history.go(-steps);
}

/**
 * Runs `write`, which writes to the history, once no step back of ours is on
 * its way: at once when none is. The router adds and replaces its entries
 * through this, for the reason given at `waiting`.
 */
export function afterOwnSteps(write: () => void): void {
  if (ownSteps === 0 && waiting.length === 0) {
    write();
    return;
  }
  waiting.push(write);
  waitTimer ??= setTimeout(onWaitTimeout, WAIT_LIMIT);
}

function onPopState(): void {
  if (ownSteps > 0) {
    ownSteps -= 1;
    if (ownSteps === 0) runWaiting();
    return;
  }
  opened.pop()?.close();
}

/** Call while a component is being set up. */
export function closeOnBack(isOpen: () => boolean, close: () => void): void {
  // `pre`: such effects run from the outside in. When a card and a sheet over
  // it are both open from the start, the card is entered first and the sheet
  // is what "back" closes first.
  $effect.pre(() => {
    if (!isOpen()) return;
    ensureListening();
    const mine: Open = { close, entered: false };
    opened.push(mine);
    afterOwnSteps(() => {
      // Closed, or let go of by the router, before its turn came: there is nothing to add.
      const at = opened.indexOf(mine);
      if (at === -1) return;
      history.pushState({ ...(history.state as object | null), [MARK]: at + 1 }, '');
      mine.entered = true;
    });
    return () => {
      const at = opened.indexOf(mine);
      // Not in the list: the back button took it out, and the entry with it.
      if (at === -1) return;
      opened.splice(at, 1);
      // Closed before its entry was added: there is none to take back.
      if (!mine.entered) return;
      stepBack();
    };
  });
}

/** Whether something that the back button closes is open: a drawer, a sheet, a dialog. */
export function overlayOpen(): boolean {
  return opened.length > 0;
}

/**
 * Lets go of the entries of everything that is open, and says how many there
 * were. The router calls this when it goes somewhere else while a drawer or
 * a sheet is open (a board was chosen in the drawer): it then puts the new
 * address in place of the overlay's entry, and the overlay, when it closes a
 * moment later, leaves the history alone.
 */
export function releaseOverlayEntries(): number {
  // Only what has an entry counts: something still waiting for its turn gets none.
  const entered = opened.filter((one) => one.entered).length;
  opened.length = 0;
  return entered;
}
