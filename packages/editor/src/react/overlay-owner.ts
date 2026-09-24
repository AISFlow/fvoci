/** WHY: Keep editor portals inside their nearest modal/focus owner. */
export function overlayOwner(element: Element): HTMLElement {
	return (
		element.closest<HTMLElement>(
			'[role="dialog"], [role="alertdialog"], dialog',
		) ?? element.ownerDocument.body
	);
}

/* WHY: Delete/Backspace caret-owner policy is a pure decision so node:test can
 * load it without the FvociEditor tsx graph. The keydown handler that applies
 * it lives in fvoci-editor.tsx editorProps (no extra module, timers, or
 * watchers). When take is true the handler dispatches a selection-only
 * transaction and returns false so Tiptap's stock Backspace/Delete chain runs
 * on the corrected caret. */
export type NativeOwnedDeleteInput = {
	editable: boolean;
	trusted: boolean;
	composing: boolean;
	keyCode: number;
	key: string;
	pmIsTextSelection: boolean;
	pmAnchor: number;
	pmHead: number;
	nativeAnchorInside: boolean;
	nativeFocusInside: boolean;
	nativeAnchorPos: number | null;
	nativeFocusPos: number | null;
};

export type NativeOwnedDeleteDecision =
	| { take: false }
	| { take: true; anchorPos: number; headPos: number };

export function nativeOwnedDeleteDecision(
	input: NativeOwnedDeleteInput,
): NativeOwnedDeleteDecision {
	if (!input.trusted) return { take: false };
	if (!input.editable) return { take: false };
	if (input.composing || input.keyCode === 229) return { take: false };
	if (input.key !== "Delete" && input.key !== "Backspace") {
		return { take: false };
	}
	if (!input.pmIsTextSelection) return { take: false };
	if (!input.nativeAnchorInside || !input.nativeFocusInside) {
		return { take: false };
	}
	if (input.nativeAnchorPos == null || input.nativeFocusPos == null) {
		return { take: false };
	}
	if (
		input.nativeAnchorPos === input.pmAnchor &&
		input.nativeFocusPos === input.pmHead
	) {
		return { take: false };
	}
	return {
		take: true,
		anchorPos: input.nativeAnchorPos,
		headPos: input.nativeFocusPos,
	};
}
