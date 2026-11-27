/**
 * Draws the blocks of two bodies merged by a comparison (logic/body-diff.ts)
 * as `renderBody` draws a body, with what changed marked on them:
 *
 * - A block added or removed whole carries `data-diff` (`added`, `removed`);
 *   an image or a rule, which hold nothing, is put in an element that does.
 * - A piece of text added or removed is an `<ins>` or a `<del>`, the same
 *   text with other marks a `<span>`; each has `data-change`.
 * - A block of another kind or other attributes than it was says how in a
 *   line before it (`doc-diff-note`): a heading's level, a code block's
 *   language, a task checked, an image's text.
 *
 * Every mark has `data-diff-label`, the words the style sheet (diff.css)
 * puts before it for whoever hears the page rather than sees it: the marks
 * are told apart by more than their colour. Every block that changed is a
 * stop (`DIFF_STOP`) that the way from change to change goes through, and
 * takes the focus there.
 *
 * Like render.ts, it builds elements and text nodes only.
 */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { BodyDiff } from '$lib/logic/body-diff.ts';
import type { DiffNode } from '$lib/logic/body-nodes.ts';

import { codeLanguageOf } from './code-languages.ts';
import { drawBlocks, type RenderBodyOptions } from './render.ts';
import { DIAGRAM_LANGUAGE } from './schema.ts';

/** The attribute of each block that changed: the stops of the way through the changes. */
export const DIFF_STOP = 'data-diff-stop';

/** Blocks that hold nothing, put in an element that can carry the marks. */
const EMPTY = new Set(['IMG', 'HR']);
/** Blocks of text, of which one changed in place is a stop. */
const TEXT = new Set(['P', 'H1', 'H2', 'H3', 'PRE']);

const LABELS: Record<string, () => string> = {
  added: m.body_diff_added,
  removed: m.body_diff_removed,
  format: m.body_diff_format,
};
const BLOCK_LABELS: Record<string, () => string> = {
  added: m.body_diff_added_block,
  removed: m.body_diff_removed_block,
};

/** What a block of a kind is called, in a note of what it was. */
function kindName(type: string, attrs: Record<string, unknown> | undefined): string {
  const level = attrs?.level;
  if (type === 'heading')
    return m.body_diff_heading({ level: typeof level === 'number' ? String(level) : '' });
  if (type === 'paragraph') return m.body_diff_paragraph();
  if (type === 'bulletList') return m.body_diff_bullet_list();
  if (type === 'orderedList') return m.body_diff_ordered_list();
  return type;
}

const languageName = (value: unknown): string =>
  typeof value === 'string' ? (codeLanguageOf(value)?.label ?? value) : m.body_diff_nothing();

/** A list's first number: 1 where none is stored. */
const startOf = (value: unknown): string => (typeof value === 'number' ? String(value) : '1');

const altText = (value: unknown): string =>
  typeof value === 'string' && value !== '' ? `“${value}”` : m.body_diff_nothing();

/** What is said of a block of another kind or other attributes than it was; nothing when it is not worth saying. */
export function noteOf(node: DiffNode): string | undefined {
  const was = node.was;
  if (!was) return undefined;
  if (was.type !== node.type)
    return m.body_diff_kind({
      from: kindName(was.type, was.attrs),
      to: kindName(node.type, node.attrs),
    });
  const before = was.attrs ?? {};
  const after = node.attrs ?? {};
  switch (node.type) {
    case 'heading':
      return before.level === after.level
        ? undefined
        : m.body_diff_kind({ from: kindName('heading', before), to: kindName('heading', after) });
    case 'codeBlock':
      return before.language === after.language
        ? undefined
        : m.body_diff_language({
            from: languageName(before.language),
            to: languageName(after.language),
          });
    case 'taskItem':
      if (before.checked === after.checked) return undefined;
      return after.checked === true ? m.body_diff_checked() : m.body_diff_unchecked();
    case 'image':
      return before.alt === after.alt
        ? undefined
        : m.body_diff_alt({ from: altText(before.alt), to: altText(after.alt) });
    case 'orderedList':
      // A list's own start is 1, which is not stored.
      return startOf(before.start) === startOf(after.start)
        ? undefined
        : m.body_diff_start({ from: startOf(before.start), to: startOf(after.start) });
    default:
      return undefined;
  }
}

/** How much a comparison changed, in a line: what was added and removed, and what else changed. */
export function diffCounts(diff: BodyDiff): string {
  const { formatted, reshaped } = diff;
  return [
    m.body_diff_counts({ added: String(diff.added), removed: String(diff.removed) }),
    formatted > 0 ? m.body_diff_formatted({ count: String(formatted) }) : undefined,
    reshaped === 1 ? m.body_diff_reshaped_one() : undefined,
    reshaped > 1 ? m.body_diff_reshaped({ count: String(reshaped) }) : undefined,
  ]
    .filter(Boolean)
    .join(' · ');
}

/**
 * Makes an element a stop of the way through the changes. A stop may hold
 * others (a list made numbered, its items reworded): the way goes to it,
 * then into it (VersionDiff.svelte).
 */
function stopAt(element: HTMLElement): void {
  element.setAttribute(DIFF_STOP, '');
  element.setAttribute('tabindex', '-1');
}

/** An item's own text is in the item's stop: a task checked and reworded is one change to go to. */
function stopAtItem(item: HTMLElement): void {
  for (const inner of item.querySelectorAll(`[${DIFF_STOP}]`)) {
    if (inner.closest('li') !== item) continue;
    inner.removeAttribute(DIFF_STOP);
    inner.removeAttribute('tabindex');
  }
  stopAt(item);
}

/**
 * Whether a block added or removed is put in an element that carries its
 * mark: one that holds nothing, and a diagram, whose picture is put after its
 * source (embeds.ts), which is then hidden.
 */
const wrapped = (node: DiffNode, element: HTMLElement): boolean =>
  EMPTY.has(element.tagName) ||
  (node.type === 'codeBlock' && node.attrs?.language === DIAGRAM_LANGUAGE);

/** Marks a block drawn from a node of the comparison; what it gives back takes the block's place. */
function decorate(page: Document, node: DiffNode, element: HTMLElement): HTMLElement {
  const change = node.diff;
  if (change === undefined) return element;
  if (change === 'added' || change === 'removed') {
    const target = wrapped(node, element) ? page.createElement('div') : element;
    if (target !== element) target.append(element);
    target.setAttribute('data-diff', change);
    target.setAttribute('data-diff-label', BLOCK_LABELS[change]?.() ?? '');
    stopAt(target);
    return target;
  }
  element.setAttribute('data-diff', 'changed');
  const said = noteOf(node);
  if (said === undefined) {
    if (TEXT.has(element.tagName) || EMPTY.has(element.tagName)) stopAt(element);
    return element;
  }
  const note = page.createElement('span');
  note.setAttribute('class', 'doc-diff-note');
  note.append(page.createTextNode(said));
  if (element.tagName === 'LI') {
    // A task's text is in its second part, after its box; an item's at its start.
    const into = element.hasAttribute('data-checked') ? element.lastElementChild : element;
    into?.prepend(note);
    stopAtItem(element);
    return element;
  }
  const holder = page.createElement('div');
  holder.setAttribute('class', 'doc-diff-noted');
  holder.append(note, element);
  stopAt(holder);
  return holder;
}

/**
 * The blocks of a comparison as DOM nodes, to be placed inside an element of
 * class `doc-body`. Both bodies were accepted by the validator before they
 * were compared; nothing else may be given.
 */
export function drawDiffBlocks(
  blocks: DiffNode[],
  options: RenderBodyOptions = {},
): DocumentFragment {
  const page = options.document ?? document;
  const fragment = drawBlocks(blocks, {
    ...options,
    decorate: (node, element) => decorate(page, node, element),
  });
  for (const element of fragment.querySelectorAll('[data-change]')) {
    const label = LABELS[element.getAttribute('data-change') ?? ''];
    if (label) element.setAttribute('data-diff-label', label());
  }
  return fragment;
}
