/**
 * The keys that move the picked day of a grid: a day to either side, a week
 * up or down. The days of a grid are one stop of the Tab key; these walk it.
 */
const STEPS: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -7, ArrowDown: 7 };

/** Moves by the step of an arrow key and keeps the key from scrolling the page. */
export function dayKeys(event: KeyboardEvent, onmove: (by: number) => void): void {
  const by = STEPS[event.key];
  if (by === undefined || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return;
  event.preventDefault();
  onmove(by);
}
