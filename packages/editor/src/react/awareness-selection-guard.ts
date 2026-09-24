/* WHY: y-tiptap's yCursorPlugin dispatches a decoration-only transaction on
 * awareness updates. ProseMirror treats a decoration change as a doc-view
 * update and calls selectionToDOM, which writes state.selection over the
 * native Range. placeContentCaret and Arrow keys move the native caret
 * before selectionchange adopts it; an in-flight peer caret then pins PM
 * (and the user-visible caret) at the stale position — CI: expected [5,5],
 * stayed [1,1] for 5s. This gate decides when an awareness-only transaction
 * must adopt the native caret so selectionToDOM cannot clobber it. */

export type AwarenessSelectionGuardInput = {
	awarenessUpdated: boolean;
	docChanged: boolean;
	selectionSet: boolean;
	composing: boolean;
	editable: boolean;
	pmIsTextSelection: boolean;
};

export function shouldAdoptNativeOnAwareness(
	input: AwarenessSelectionGuardInput,
): boolean {
	if (!input.awarenessUpdated) return false;
	if (input.docChanged || input.selectionSet) return false;
	if (!input.editable || input.composing) return false;
	return input.pmIsTextSelection;
}

export type NativeSelectionChangeGuardInput = {
	composing: boolean;
	editable: boolean;
	nativeInside: boolean;
	pmIsTextSelection: boolean;
};

export function shouldAdoptNativeOnSelectionChange(
	input: NativeSelectionChangeGuardInput,
): boolean {
	if (!input.editable || input.composing || !input.nativeInside) return false;
	return input.pmIsTextSelection;
}

export type WriteSelectionToDomInput = {
	force: boolean;
	selectionSet: boolean;
	native: { from: number; to: number } | null;
	writeFrom: number;
	writeTo: number;
};

/* WHY: selectionToDOM writes state.selection over the native Range on
 * decoration-only updates and on PM's 20ms-after-focus timeout. Skip that
 * write when PM did not set the selection and the native caret already
 * maps to a different text position — native is the user-visible owner
 * until PM adopts it. Forced writes and selectionSet transactions still
 * update the DOM. */
export function shouldWriteSelectionToDom(input: WriteSelectionToDomInput): boolean {
	if (input.force || input.selectionSet) return true;
	if (!input.native) return true;
	if (input.native.from !== input.native.to || input.writeFrom !== input.writeTo) {
		return true;
	}
	return input.native.from === input.writeFrom && input.native.to === input.writeTo;
}
