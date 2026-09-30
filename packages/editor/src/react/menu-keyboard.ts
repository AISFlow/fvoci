import {
	type KeyboardEvent,
	type MouseEvent,
	type RefObject,
	useEffect,
} from "react";
import { flushSync } from "react-dom";
import { focusFirstMenuItem, moveMenuFocus } from "../menu-roving.js";

/** WHY: Restore the opener before native Tab navigation so a closed menu cannot trap focus. */
export function leaveMenu(
	event: KeyboardEvent<HTMLElement>,
	opener: Element | null,
	onClose: () => void,
): void {
	event.stopPropagation();
	flushSync(onClose);
	if (opener instanceof HTMLElement) opener.focus({ preventScroll: true });
}

/** The roving menu of menu-roving.ts, entered at its first item when it opens. */
export function useRovingMenu(
	ref: RefObject<HTMLElement | null>,
	open: boolean,
): (event: KeyboardEvent<HTMLElement>) => void {
	useEffect(() => {
		if (!open) return;
		focusFirstMenuItem(ref.current);
	}, [ref, open]);
	return (event) => moveMenuFocus(ref.current, event);
}

/** WHY: 명령을 click 으로 옮겼으므로 mousedown 기본동작이 ProseMirror 선택을 지우면 안 된다. */
export function preventSelectionLoss(event: MouseEvent): void {
	event.preventDefault();
}
