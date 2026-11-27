/**
 * Draws the formulas and diagrams of a body that is already in the page.
 *
 * `renderBody` and the editor only make room for them: a formula is an
 * element that holds its source, a diagram is the code block it is stored
 * as. This finds those places and draws into them. The libraries that do the
 * drawing are fetched only when a place for them is found, so a body with
 * neither costs nothing here.
 *
 * Until something is drawn, and whenever it cannot be, the source stays
 * where it is and readable; a failure adds a line that says so.
 */
import { m } from '$lib/i18n/paraglide/messages.js';

import type { DiagramFailure, DiagramResult } from './diagram.ts';
import { diagramKind } from './diagram-kind.ts';
import { DIAGRAM_CODE_SELECTOR, MATH_CLASS, MATH_SLOTS, MATH_SOURCE_ATTRIBUTE } from './drawing.ts';
import type { drawMath, MathLibrary, MathResult } from './math.ts';

/**
 * How long drawing formulas may hold the page at a stretch before it lets
 * the page breathe, and how long all the formulas of one body may take
 * together, in milliseconds of the library's own time. One formula cannot be
 * interrupted (its length and the library's limits bound it); these two
 * bound what many formulas add up to. Past the budget the rest stay source.
 */
const MATH_SLICE = 24;
const MATH_BUDGET = 3000;
/** A drawn formula taller than this many pixels is shown as its source instead. */
const MATH_MAX_HEIGHT = 1600;
/** How long after the last change of a diagram's source it is drawn again, while editing. */
export const DIAGRAM_REDRAW_DELAY = 350;

const MATH_SELECTOR = Object.values(MATH_SLOTS)
  .map((slot) => `${slot.tag}.${MATH_CLASS}[data-type="${slot.type}"][${MATH_SOURCE_ATTRIBUTE}]`)
  .join(', ');

const mathFailure: Record<'too-long' | 'invalid' | 'too-large' | 'over-budget', () => string> = {
  'too-long': m.editor_math_too_long,
  invalid: m.editor_math_invalid,
  'too-large': m.editor_math_too_large,
  'over-budget': m.editor_math_not_drawn,
};
const diagramFailure: Record<DiagramFailure, () => string> = {
  'too-long': m.editor_diagram_too_long,
  'not-allowed': m.editor_diagram_not_allowed,
  invalid: m.editor_diagram_invalid,
  timeout: m.editor_diagram_timeout,
  failed: m.editor_diagram_invalid,
};

const pause = (): Promise<void> =>
  new Promise((resolve) => {
    setTimeout(resolve, 0);
  });

/** A line that says what went wrong, to stand next to the source. */
function notice(page: Document, text: string, tag: 'span' | 'p' = 'span'): HTMLElement {
  const element = page.createElement(tag);
  element.setAttribute('class', 'doc-embed-problem');
  element.setAttribute('role', 'note');
  element.append(page.createTextNode(text));
  return element;
}

/**
 * Puts the source back into a formula's place, with why behind it: what went
 * wrong (`error`), or that the formula was not drawn at all (`skipped`).
 */
function showMathSource(slot: HTMLElement, reason: keyof typeof mathFailure): void {
  const page = slot.ownerDocument;
  slot.setAttribute('data-state', reason === 'over-budget' ? 'skipped' : 'error');
  slot.replaceChildren(
    page.createTextNode(slot.getAttribute(MATH_SOURCE_ATTRIBUTE) ?? ''),
    notice(page, mathFailure[reason]()),
  );
}

function applyMath(slot: HTMLElement, library: MathLibrary, draw: DrawMath): MathResult {
  const latex = slot.getAttribute(MATH_SOURCE_ATTRIBUTE) ?? '';
  const display = slot.getAttribute('data-type') === MATH_SLOTS.blockMath.type;
  const result = draw(library, slot, latex, display);
  if (result.ok) {
    slot.setAttribute('data-state', 'drawn');
    // The drawing says nothing to a reader of the markup; the source does.
    slot.setAttribute('aria-label', latex);
    slot.setAttribute('role', 'math');
  } else showMathSource(slot, result.reason);
  return result;
}

/** A drawn formula that is far too tall goes back to being its source. */
function refuseOversized(slots: HTMLElement[]): void {
  const tall = slots.filter(
    (slot) =>
      slot.getAttribute('data-state') === 'drawn' &&
      slot.getBoundingClientRect().height > MATH_MAX_HEIGHT,
  );
  for (const slot of tall) {
    slot.removeAttribute('aria-label');
    slot.removeAttribute('role');
    showMathSource(slot, 'too-large');
  }
}

type DrawMath = typeof drawMath;

/** How wide the line is that an element stands in: the nearest ancestor that is a box. */
function lineWidth(element: HTMLElement): number {
  for (let parent = element.parentElement; parent; parent = parent.parentElement) {
    if (parent.clientWidth > 0) return parent.clientWidth;
  }
  return 0;
}

/**
 * A formula in a line cannot be broken everywhere, and one that is wider than
 * its line would widen the whole page. Such a formula is marked
 * (`data-wide`), and editor.css lets it scroll inside itself. Only those:
 * the mark costs the formula its place on the baseline of the text.
 */
export function fitFormulas(slots: Iterable<HTMLElement>): void {
  const drawn = Array.from(slots).filter(
    (slot) => slot.localName === 'span' && slot.getAttribute('data-state') === 'drawn',
  );
  // All the writes, then all the reads, then the writes again: one layout for the lot.
  for (const slot of drawn) slot.removeAttribute('data-wide');
  const wide = drawn.filter((slot) => {
    const available = lineWidth(slot);
    return available > 0 && slot.getBoundingClientRect().width > available;
  });
  for (const slot of wide) slot.setAttribute('data-wide', '');
}

/**
 * Keeps the formulas under `root` fitted as its width changes (a window
 * resized, a device turned). Returns what stops it.
 */
export function watchFormulaFit(root: HTMLElement): () => void {
  if (typeof ResizeObserver === 'undefined') return () => undefined;
  let width = root.clientWidth;
  const observer = new ResizeObserver(() => {
    if (root.clientWidth === width) return;
    width = root.clientWidth;
    fitFormulas(root.querySelectorAll<HTMLElement>(MATH_SELECTOR));
  });
  observer.observe(root);
  return () => {
    observer.disconnect();
  };
}

/**
 * Draws one formula, in the editor: the element holds the source and is
 * redrawn whenever that changes. Resolves once it is drawn or known not to be.
 */
export async function fillMath(slot: HTMLElement): Promise<void> {
  const { loadMath, drawMath } = await import('./math.ts');
  const library = await loadMath();
  if (!slot.isConnected) return;
  applyMath(slot, library, drawMath);
  refuseOversized([slot]);
  fitFormulas([slot]);
}

async function fillAllMath(slots: HTMLElement[], cancelled: () => boolean): Promise<void> {
  const { loadMath, drawMath } = await import('./math.ts');
  const library = await loadMath();
  let spent = 0;
  let slice = 0;
  for (const slot of slots) {
    if (cancelled()) return;
    if (spent > MATH_BUDGET) {
      // Past the budget: the rest stay their source, and say they were not drawn.
      showMathSource(slot, 'over-budget');
      continue;
    }
    const { ms } = applyMath(slot, library, drawMath);
    spent += ms;
    slice += ms;
    if (slice > MATH_SLICE) {
      slice = 0;
      await pause();
    }
  }
  if (cancelled()) return;
  refuseOversized(slots);
  fitFormulas(slots);
}

/** The element a diagram is drawn into; it follows the code block that holds the source. */
function createDiagramFigure(page: Document): HTMLElement {
  const figure = page.createElement('div');
  figure.setAttribute('class', 'doc-diagram');
  figure.setAttribute('data-state', 'pending');
  return figure;
}

/** What went wrong with a diagram, in words, and the library's own words about where. */
export function diagramProblem(
  page: Document,
  result: Extract<DiagramResult, { ok: false }>,
): HTMLElement[] {
  const parts = [notice(page, diagramFailure[result.reason](), 'p')];
  if (result.detail) {
    // The library's own words about where the text went wrong. Text, not markup.
    const detail = page.createElement('pre');
    detail.setAttribute('class', 'doc-embed-detail');
    detail.append(page.createTextNode(result.detail));
    parts.push(detail);
  }
  return parts;
}

/** What went wrong with a formula that was drawn aside (math-block.ts): its notice, again. */
export function mathProblem(page: Document, slot: HTMLElement): HTMLElement[] {
  // Without a notice the library did not arrive: the formula is not drawn all the same.
  const said = slot.querySelector('.doc-embed-problem')?.textContent ?? mathFailure.invalid();
  return [notice(page, said, 'p')];
}

/**
 * Draws a diagram into its figure, read-only. Gives back whether a picture is
 * shown; a failure shows why instead. The editor draws its own (diagram-block.ts).
 */
async function fillDiagram(
  figure: HTMLElement,
  source: string,
  isCurrent: () => boolean,
): Promise<boolean> {
  const page = figure.ownerDocument;
  let result: DiagramResult;
  if (source.trim() === '') {
    // Nothing to draw: an empty figure, and no complaint.
    figure.setAttribute('data-state', 'empty');
    figure.replaceChildren();
    return false;
  }
  try {
    const { drawDiagram } = await import('./diagram.ts');
    const width = figure.getBoundingClientRect().width || figure.parentElement?.clientWidth || 0;
    result = await drawDiagram(source, width, page);
  } catch (error) {
    result = { ok: false, reason: 'failed', detail: error instanceof Error ? error.message : '' };
  }
  if (!figure.isConnected || !isCurrent()) return false;
  if (result.ok) {
    figure.setAttribute('data-state', 'drawn');
    figure.setAttribute('role', 'img');
    figure.setAttribute('aria-label', diagramKind(source));
    figure.replaceChildren(result.picture);
    return true;
  }
  figure.setAttribute('data-state', 'error');
  figure.removeAttribute('role');
  figure.removeAttribute('aria-label');
  figure.replaceChildren(...diagramProblem(page, result));
  return false;
}

/**
 * Calls back when the page changes between light and dark (the class of the
 * root element changes), which is when diagrams have to be drawn again:
 * their colours are worked out from the page's when they are drawn.
 */
export function onLookChange(page: Document, callback: () => void): () => void {
  if (typeof MutationObserver === 'undefined') return () => undefined;
  let last = page.documentElement.getAttribute('class');
  const observer = new MutationObserver(() => {
    const now = page.documentElement.getAttribute('class');
    if (now === last) return;
    last = now;
    callback();
  });
  observer.observe(page.documentElement, { attributes: true, attributeFilter: ['class'] });
  return () => {
    observer.disconnect();
  };
}

/**
 * Draws every formula and diagram under `root`, a body that `renderBody`
 * drew. Read-only: a diagram that could be drawn takes the place of its
 * source, which is hidden but stays in the page. Returns what undoes the
 * watching; call it before the body is replaced or removed.
 */
export function drawEmbeds(root: HTMLElement): () => void {
  const formulas = Array.from(root.querySelectorAll<HTMLElement>(MATH_SELECTOR));
  const diagrams = Array.from(root.querySelectorAll<HTMLElement>(DIAGRAM_CODE_SELECTOR));
  if (formulas.length === 0 && diagrams.length === 0) return () => undefined;

  let cancelled = false;
  const isCancelled = (): boolean => cancelled;
  const undo: (() => void)[] = [];

  if (formulas.length > 0) {
    // A failure to fetch the library leaves the sources as they are.
    fillAllMath(formulas, isCancelled).catch(() => undefined);
    undo.push(watchFormulaFit(root));
  }

  if (diagrams.length > 0) {
    const page = root.ownerDocument;
    const drawn = diagrams.flatMap((code) => {
      const block = code.parentElement;
      if (!block) return [];
      const figure = createDiagramFigure(page);
      block.after(figure);
      return [{ block, figure, source: code.textContent }];
    });
    const drawAll = (): void => {
      for (const { block, figure, source } of drawn) {
        void fillDiagram(figure, source, () => !cancelled).then((shown) => {
          if (!cancelled) block.hidden = shown;
        });
      }
    };
    drawAll();
    undo.push(onLookChange(page, drawAll));
    // Should the same body be drawn into again: back to how `renderBody` left it.
    undo.push(() => {
      for (const { block, figure } of drawn) {
        figure.remove();
        block.hidden = false;
      }
    });
  }

  return () => {
    cancelled = true;
    for (const step of undo) step();
  };
}
