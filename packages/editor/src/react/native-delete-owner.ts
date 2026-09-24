/* WHY: collaboration Delete/Backspace must use one caret owner. Arrow keys move
 * the native Selection while PM adopts it later via selectionchange, but PM
 * handles Delete/Backspace synchronously against state.selection. This pure gate
 * decides whether a keydown is eligible for aligning PM to the native caret; the
 * handler in fvoci-editor.tsx maps the native caret the way PM's own
 * selectionFromDOM does and falls through to the stock keymap. Kept separate from
 * the tsx graph so node:test can load it. */
export type NativeDeleteKey = {
	trusted: boolean;
	editable: boolean;
	composing: boolean;
	keyCode: number;
	key: string;
	pmIsTextSelection: boolean;
};

export function isNativeOwnedDeleteKey(input: NativeDeleteKey): boolean {
	// Android readDOMChange synthesizes untrusted Backspace; composition and
	// keyCode 229 belong to the IME path.
	if (!input.trusted || !input.editable) return false;
	if (input.composing || input.keyCode === 229) return false;
	if (input.key !== "Delete" && input.key !== "Backspace") return false;
	return input.pmIsTextSelection;
}
