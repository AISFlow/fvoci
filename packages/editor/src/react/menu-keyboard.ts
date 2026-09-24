import {
	type KeyboardEvent,
	type MouseEvent,
	type RefObject,
	useEffect,
} from "react";
import { flushSync } from "react-dom";

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

function menuItems(root: HTMLElement | null): HTMLElement[] {
	if (!root) return [];
	return [
		...root.querySelectorAll<HTMLElement>(
			'[role^="menuitem"]:not([aria-disabled="true"]):not(:disabled)',
		),
	];
}

function activate(items: HTMLElement[], index: number): void {
	for (const [i, item] of items.entries()) item.tabIndex = i === index ? 0 : -1;
	const active = items[index];
	active?.focus({ preventScroll: true });
	const root = active?.closest<HTMLElement>('[role="menu"]');
	if (active && root)
		root.scrollTop = Math.max(
			active.offsetTop + active.offsetHeight - root.clientHeight,
			Math.min(root.scrollTop, active.offsetTop),
		);
}

/**
 * WHY: WAI-ARIA APG Menu — 활성 항목만 tabindex=0 이고 ↑/↓/Home/End 로 순환한다.
 * 열릴 때 첫 항목으로 포커스가 들어가야 포인터 없이 메뉴에 진입할 수 있다.
 */
export function useRovingMenu(
	ref: RefObject<HTMLElement | null>,
	open: boolean,
): (event: KeyboardEvent<HTMLElement>) => void {
	useEffect(() => {
		if (!open) return;
		activate(menuItems(ref.current), 0);
	}, [ref, open]);
	return (event) => {
		if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
		const items = menuItems(ref.current);
		if (items.length === 0) return;
		/* WHY: `indexOf` 는 동등 비교만 한다 — `activeElement` 가 null 이거나
		 * `items` 밖 원소여도 -1 로 안전하다. 캐스트는 타입만 좁힌다. */
		const current = items.indexOf(document.activeElement as HTMLElement);
		const last = items.length - 1;
		const next =
			event.key === "Home"
				? 0
				: event.key === "End"
					? last
					: event.key === "ArrowDown"
						? (current + 1) % items.length
						: current <= 0
							? last
							: current - 1;
		event.preventDefault();
		activate(items, next);
	};
}

/** WHY: 명령을 click 으로 옮겼으므로 mousedown 기본동작이 ProseMirror 선택을 지우면 안 된다. */
export function preventSelectionLoss(event: MouseEvent): void {
	event.preventDefault();
}
