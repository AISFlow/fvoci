/**
 * Builds in the frame the tree the Word worker drew (`worker-html.ts`
 * `flatten`): element by element, with `createElementNS` and `setAttribute`,
 * so that the tree is the one docx-preview made (no HTML parser in between
 * to rebuild it) and nothing in it is read as markup.
 *
 * The building goes a slice of steps at a time, and gives the thread back
 * between slices: the frame shares it with the app, which keeps answering
 * (its timers, "Close") while a long document is built. The tree is built
 * apart from the page and joined to it once, so the page lays it out once.
 *
 * A tag or attribute name that is not a plain name, a tag that could run,
 * load or navigate, and an attribute that would run script (`on…`), are
 * left out, whatever the worker says: a defense besides the frame's policy.
 */
import { isAllowedHref } from '$lib/editor/schema.ts';

import { CLOSE, MEASURE, NAMESPACES, OPEN, TEXT, type Step } from './docx-protocol.ts';
import { yieldThread } from './yield-thread.ts';

/** How many steps are built before the thread is given back. */
const SLICE = 20_000;
const TAG = /^[A-Za-z][A-Za-z0-9-]*$/;
/**
 * Tags docx-preview never makes and that could run, load or navigate: left
 * out whatever the worker says (the frame's policy would stop most of them).
 */
const BARRED = new Set([
  'script',
  'iframe',
  'frame',
  'frameset',
  'object',
  'embed',
  'applet',
  'base',
  'meta',
  'link',
  'form',
  'input',
  'button',
  'textarea',
  'select',
  'portal',
  'animate',
  'set',
  'animatemotion',
  'animatetransform',
  'use',
  'handler',
  'listener',
]);
const ATTRIBUTE = /^[A-Za-z_:][A-Za-z0-9_.:-]*$/;
const ADDRESS_ATTRIBUTES = new Set(['href', 'src', 'xlink:href']);
const SAFE_DATA_IMAGE = /^data:image\/(?!svg)[a-z0-9.+-]+[;,]/i;

function isSafeDocxAddress(value: string): boolean {
  const trimmed = value.trim();
  if (trimmed.startsWith('#') || /^blob:/i.test(trimmed) || SAFE_DATA_IMAGE.test(trimmed)) {
    return true;
  }
  return /^(?:https?:|mailto:)/i.test(trimmed) && isAllowedHref(trimmed);
}

/** Thrown when the steps are not in their shape: the worker's answer is not taken. */
export class BrokenSteps extends Error {}

/**
 * Builds the steps' nodes into `into` (apart from the page), a slice at a
 * time; `fill` rewrites each text and attribute value (the pictures' keys).
 */
export async function build(
  steps: readonly Step[],
  into: ParentNode,
  fill: (value: string) => string = (value) => value,
): Promise<void> {
  const open: ParentNode[] = [into];
  // An element left out still has its children: they go to its parent.
  const skipped: boolean[] = [];
  let at = 0;
  const take = (): Step => {
    const step = steps[at];
    if (step === undefined) throw new BrokenSteps();
    at += 1;
    return step;
  };
  const text = (): string => {
    const value = take();
    if (typeof value !== 'string') throw new BrokenSteps();
    return value;
  };
  while (at < steps.length) {
    const until = at + SLICE;
    while (at < steps.length && at < until) {
      const parent = open.at(-1) ?? into;
      const kind = take();
      if (kind === TEXT) parent.append(fill(text()));
      else if (kind === CLOSE) {
        if (skipped.pop() === false) open.pop();
      } else if (kind === OPEN) {
        const tag = text();
        const namespace = NAMESPACES[Number(take())] ?? NAMESPACES[0];
        const count = Number(take());
        const plain = TAG.test(tag) && !BARRED.has(tag.toLowerCase());
        const element = plain ? document.createElementNS(namespace, tag) : undefined;
        for (let attribute = 0; attribute < count; attribute += 1) {
          const [name, value] = [text(), text()];
          if (element && ATTRIBUTE.test(name) && !/^on/i.test(name)) {
            const filled = fill(value);
            const lower = name.toLowerCase();
            if (
              ADDRESS_ATTRIBUTES.has(lower) ||
              lower.endsWith(':href') ||
              lower.endsWith(':src')
            ) {
              if (isSafeDocxAddress(filled)) element.setAttribute(name, filled);
            } else {
              element.setAttribute(name, filled);
            }
          }
        }
        skipped.push(!element);
        if (element) {
          parent.append(element);
          open.push(element);
        }
      } else throw new BrokenSteps();
    }
    if (at < steps.length) await yieldThread();
  }
}

/**
 * Sizes the shapes docx-preview would have measured on the next frame, as
 * it would: each to the box of what it draws, the innermost first (a shape
 * holds the text boxes of shapes). Called once they are in the page, where
 * there is layout; at each depth every box is read before any is written,
 * so the page is laid out once for each depth, not for each shape.
 */
export function measureShapes(root: ParentNode): void {
  const shapes = [...root.querySelectorAll<SVGSVGElement>(`svg[${MEASURE}]`)];
  const depth = new Map<SVGSVGElement, number>();
  for (const shape of shapes) {
    const outer = shape.parentElement?.closest<SVGSVGElement>(`svg[${MEASURE}]`);
    depth.set(shape, outer ? (depth.get(outer) ?? 0) + 1 : 0);
  }
  const deepest = Math.max(-1, ...depth.values());
  for (let level = deepest; level >= 0; level -= 1) {
    const boxes = shapes
      .filter((shape) => depth.get(shape) === level)
      .map((shape) => {
        const drawn = shape.firstElementChild;
        return [shape, drawn instanceof SVGGraphicsElement ? drawn.getBBox() : undefined] as const;
      });
    for (const [shape, box] of boxes) {
      shape.removeAttribute(MEASURE);
      if (!box) continue;
      shape.setAttribute('width', `${Math.ceil(box.x + box.width)}`);
      shape.setAttribute('height', `${Math.ceil(box.y + box.height)}`);
    }
  }
}
