/**
 * The keys of a bar of buttons (`role="toolbar"`): Tab stops at one of its
 * buttons, the arrows go from one button to the next, Home and End to the
 * first and the last. A button that cannot be pressed now is passed by.
 */

/** A button of a bar by its `data-format`, and whether it can be pressed now. */
export interface BarButton {
  id: string;
  can: boolean;
}

/** The one button Tab stops at: the one focused last while it can be pressed, else the first that can. */
export function tabStop(
  buttons: readonly BarButton[],
  last: string | undefined,
): string | undefined {
  const pressable = buttons.filter((button) => button.can);
  return (pressable.find((button) => button.id === last) ?? pressable[0])?.id;
}

/** The `data-format` of the button that `event` brought the focus to, if it is one. */
export function focusedFormat(event: FocusEvent): string | undefined {
  return event.target instanceof HTMLElement ? event.target.dataset.format : undefined;
}

const STEPS: Readonly<Record<string, number>> = { ArrowLeft: -1, ArrowRight: 1 };

/** Moves the focus along the bar `event` came from; true when the key was the bar's. */
export function moveAlongBar(event: KeyboardEvent, bar: HTMLElement): boolean {
  const buttons = [...bar.querySelectorAll<HTMLButtonElement>('button:not(:disabled)')];
  const at = buttons.findIndex((button) => button === event.target);
  if (at === -1 || buttons.length === 0) return false;
  const step = STEPS[event.key];
  let next: number;
  if (step !== undefined) next = (at + step + buttons.length) % buttons.length;
  else if (event.key === 'Home') next = 0;
  else if (event.key === 'End') next = buttons.length - 1;
  else return false;
  event.preventDefault();
  buttons[next]?.focus();
  return true;
}

/**
 * A press on a bar that must not take the focus, nor the selection, out of the editor: on a
 * button, between them, or on one that cannot be pressed now. A field in the bar takes it.
 */
export function keepEditorFocus(event: MouseEvent): void {
  if (event.target instanceof Element && event.target.closest('input, textarea')) return;
  event.preventDefault();
}
