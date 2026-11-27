/**
 * The window's title, which names what is shown before what holds it:
 * `Item · Container — fvoci`, or `Volume · Date — fvoci` for a diary. The page
 * that shows them names the window while it is shown, and the app's name
 * alone is the title again once it goes.
 */
export const APP_TITLE = 'fvoci';

/** The title of `parts`, the empty ones left out. */
export function windowTitle(parts: readonly (string | undefined)[]): string {
  const named = parts.map((part) => part?.trim() ?? '').filter((part) => part !== '');
  return named.length === 0 ? APP_TITLE : `${named.join(' · ')} — ${APP_TITLE}`;
}

/** Names the window after what `parts` reads, while the component that calls it is shown. */
export function nameWindow(parts: () => readonly (string | undefined)[]): void {
  $effect(() => {
    document.title = windowTitle(parts());
    return () => {
      document.title = APP_TITLE;
    };
  });
}
