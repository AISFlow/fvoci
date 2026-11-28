/**
 * A toggle and a callout of a body drawn for reading (render.ts).
 *
 * A toggle is the page's own: `<details>` around its summary and what it
 * hides, closed until the reader opens it, whose summary the keyboard and
 * assistive technology know as a button that opens it. The chevron is the
 * summary's marker (editor.css). A callout is its blocks under the picture
 * of its tone, which says the tone's name.
 *
 * The parts carry the classes and attributes the editor's carry, so that
 * editor.css styles both alike; what the editor adds around them is its
 * own (the chevron's button, the tones to choose from).
 */
import { toneIcon } from './block-icons.ts';
import { toneName, toneOf } from './callout-tones.ts';
import type { DocNode } from './schema.ts';

/** Draws a part of a block: a line of what stands in a line, or a holder of blocks. */
export type DrawPart = (tag: string, part: DocNode, line: boolean) => HTMLElement;

function marked(element: HTMLElement, name: string, attribute: string): HTMLElement {
  element.classList.add(name);
  element.setAttribute(attribute, '');
  return element;
}

function drawDetails(
  page: Document,
  node: DocNode,
  childrenOf: (node: DocNode) => DocNode[],
  draw: DrawPart,
): HTMLElement | undefined {
  const [summary, content] = childrenOf(node);
  if (summary?.type !== 'detailsSummary' || content?.type !== 'detailsContent') return undefined;
  const details = marked(page.createElement('details'), 'doc-details', 'data-details');
  details.append(
    marked(draw('summary', summary, true), 'doc-details-summary', 'data-details-summary'),
    marked(draw('div', content, false), 'doc-details-content', 'data-details-content'),
  );
  return details;
}

function drawCallout(page: Document, node: DocNode, draw: DrawPart): HTMLElement {
  const tone = toneOf(node.attrs?.tone);
  const callout = marked(page.createElement('div'), 'doc-callout', 'data-callout');
  callout.setAttribute('data-tone', tone);
  const icon = page.createElement('span');
  icon.className = 'doc-callout-icon';
  icon.setAttribute('role', 'img');
  icon.setAttribute('aria-label', toneName(tone));
  icon.append(toneIcon(page, tone));
  const content = draw('div', node, false);
  content.className = 'doc-callout-content';
  callout.append(icon, content);
  return callout;
}

/** A toggle or a callout, or nothing for another node or a toggle without its parts. */
export function drawBlock(
  page: Document,
  node: DocNode,
  childrenOf: (node: DocNode) => DocNode[],
  draw: DrawPart,
): HTMLElement | undefined {
  if (node.type === 'details') return drawDetails(page, node, childrenOf, draw);
  if (node.type === 'callout') return drawCallout(page, node, draw);
  return undefined;
}
