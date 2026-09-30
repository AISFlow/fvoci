/** WHY: Keep editor portals inside their nearest modal/focus owner. */
export function overlayOwner(element: Element): HTMLElement {
	return (
		element.closest<HTMLElement>(
			'[role="dialog"], [role="alertdialog"], dialog',
		) ?? element.ownerDocument.body
	);
}
