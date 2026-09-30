// packages/editor/src/clipboard.ts

// WHY: Missing clipboard access must reject like writeText permission failures so callers can show feedback.
export function copyText(text: string): Promise<void> {
  // DOM typings assume the secure-context API exists; runtime support is optional.
  const clipboard: unknown = navigator.clipboard;
  if (clipboard == null) return Promise.reject(new Error("clipboard unavailable"));
  return navigator.clipboard.writeText(text);
}
