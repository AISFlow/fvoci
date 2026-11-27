/**
 * The keys of a group of which one is chosen: a radio group or a list of
 * tabs made by hand.
 *
 * Tab reaches the group once, at the chosen answer (the first, where none
 * is). The arrow keys go to the neighbour and around the ends, Home and End
 * to the first and the last. Going to an answer also chooses it, as in any
 * radio group; where choosing closes the surface the group is in (a list of
 * colours), `choose: false` only moves, and Enter or Space chooses.
 *
 * It is attached to the element with the role of the group and reads the
 * markup: the answers are its `role="radio"` or `role="tab"` elements, the
 * chosen one says so with `aria-checked` or `aria-selected`, and choosing is
 * a click. So a group needs nothing more than `{@attach arrowChoice()}`.
 */
import type { Attachment } from 'svelte/attachments';

export interface ArrowChoiceOptions {
  /** Going to an answer chooses it. False: it only moves there. */
  choose?: boolean;
}

const ANSWERS = '[role="radio"], [role="tab"]';

function isChosen(answer: HTMLElement): boolean {
  return (
    answer.getAttribute('aria-checked') === 'true' ||
    answer.getAttribute('aria-selected') === 'true'
  );
}

/** How far a key goes: a step, or to an end. Undefined for a key that is not one of these. */
function moveOf(key: string): number | 'first' | 'last' | undefined {
  if (key === 'ArrowRight' || key === 'ArrowDown') return 1;
  if (key === 'ArrowLeft' || key === 'ArrowUp') return -1;
  if (key === 'Home') return 'first';
  if (key === 'End') return 'last';
  return undefined;
}

export function arrowChoice({ choose = true }: ArrowChoiceOptions = {}): Attachment<HTMLElement> {
  return (group) => {
    const answers = (): HTMLElement[] =>
      [...group.querySelectorAll<HTMLElement>(ANSWERS)].filter(
        (answer) => !(answer instanceof HTMLButtonElement && answer.disabled),
      );

    /** One Tab stop: where the focus is, else the chosen answer, else the first. */
    const rove = (): void => {
      const all = answers();
      const stop =
        all.find((answer) => answer === document.activeElement) ?? all.find(isChosen) ?? all[0];
      for (const answer of all) answer.tabIndex = answer === stop ? 0 : -1;
    };

    const onKeyDown = (event: KeyboardEvent): void => {
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      const move = moveOf(event.key);
      if (move === undefined) return;
      const all = answers();
      const from = all.findIndex((answer) => answer === event.target);
      if (from === -1) return;
      const to =
        move === 'first'
          ? 0
          : move === 'last'
            ? all.length - 1
            : (from + move + all.length) % all.length;
      const next = all[to];
      if (!next) return;
      event.preventDefault();
      next.focus();
      if (choose && next !== all[from] && !isChosen(next)) next.click();
      // Choosing may draw the group anew; the focus stays with the same answer where it still is.
      rove();
    };

    rove();
    // The chosen answer changes from outside too, and answers come and go.
    const watcher = new MutationObserver(rove);
    watcher.observe(group, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ['aria-checked', 'aria-selected', 'disabled'],
    });
    group.addEventListener('keydown', onKeyDown);
    group.addEventListener('focusin', rove);
    return () => {
      watcher.disconnect();
      group.removeEventListener('keydown', onKeyDown);
      group.removeEventListener('focusin', rove);
    };
  };
}
