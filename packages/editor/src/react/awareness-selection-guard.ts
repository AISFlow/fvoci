/* WHY: y-tiptap's yCursorPlugin dispatches a decoration-only transaction on
 * awareness updates. ProseMirror treats a decoration change as a doc-view
 * update and calls selectionToDOM, which writes state.selection over a native
 * caret that PM has not yet read (Arrow keys, then a peer awareness message
 * before selectionchange). Adopt native only when the live DOM selection
 * differs from PM's last-synced currentSelection — native ahead, not a
 * browser focus reset that left PM ahead. */

export type AwarenessSelectionGuardInput = {
	awarenessUpdated: boolean;
	docChanged: boolean;
	selectionSet: boolean;
	composing: boolean;
	editable: boolean;
	pmIsTextSelection: boolean;
	observedDomSelectionMatchesNative: boolean;
};

export function shouldAdoptNativeOnAwareness(
	input: AwarenessSelectionGuardInput,
): boolean {
	if (!input.awarenessUpdated) return false;
	if (input.docChanged || input.selectionSet) return false;
	if (!input.editable || input.composing) return false;
	if (input.observedDomSelectionMatchesNative) return false;
	return input.pmIsTextSelection;
}
