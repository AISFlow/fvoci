/**
 * The on-screen keyboard, where the browser does not make room for it.
 *
 * index.html asks the browser to shrink the page when the keyboard comes up
 * (`interactive-widget=resizes-content`). A browser that does not do that
 * lays the keyboard over the bottom of the page instead, and tells of it only
 * through `visualViewport`. This keeps the height that is covered in the
 * `--keyboard` variable of the root element; sheets, the comment field of a
 * card and anything else pinned to the bottom edge lift themselves by it.
 * Where the page was shrunk the variable stays at 0.
 */

/** The height the keyboard covers, from what the window and the visual viewport report. */
export function coveredHeight(
  layoutHeight: number,
  visual: { height: number; offsetTop: number; scale: number },
): number {
  // Zoomed in, the visual viewport is smaller for another reason.
  if (visual.scale > 1.01) return 0;
  const covered = layoutHeight - visual.height - visual.offsetTop;
  // A few pixels are rounding, or a toolbar on its way out.
  return covered > 48 ? Math.round(covered) : 0;
}

/**
 * Starts keeping `--keyboard` up to date, and tells `onChange` the height in
 * pixels whenever it changes. Returns the function that stops.
 */
export function watchKeyboard(onChange?: (height: number) => void): () => void {
  const viewport = window.visualViewport;
  if (!viewport) return () => undefined;
  const root = document.documentElement;
  let last = -1;
  const update = (): void => {
    const height = coveredHeight(window.innerHeight, viewport);
    if (height === last) return;
    last = height;
    root.style.setProperty('--keyboard', `${height}px`);
    onChange?.(height);
  };
  update();
  viewport.addEventListener('resize', update);
  viewport.addEventListener('scroll', update);
  return () => {
    viewport.removeEventListener('resize', update);
    viewport.removeEventListener('scroll', update);
    root.style.removeProperty('--keyboard');
  };
}
