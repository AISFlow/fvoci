// packages/editor/src/react/clipboard.ts

// WHY: Missing clipboard access must reject like writeText permission failures so callers can show feedback.
export function copyText(text: string): Promise<void> {
	return (
		navigator.clipboard?.writeText(text) ??
		Promise.reject(new Error("clipboard unavailable"))
	);
}
