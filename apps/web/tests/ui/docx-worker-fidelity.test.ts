/**
 * What the Word worker draws, built by the frame (`docx-build.ts`), against
 * docx-preview drawing the same document on a real DOM (jsdom), node for
 * node: the same tree, attributes, styles and text, but for the pictures'
 * addresses. Neither side goes through an HTML parser.
 */
import { parseAsync, renderDocument } from 'docx-preview';
import { describe, expect, it } from 'vitest';

import { BrokenSteps, build } from '../../src/viewer-frame/docx-build.ts';
import { OPTIONS } from '../../src/viewer-frame/docx-options.ts';
import { CLOSE, OPEN, TEXT } from '../../src/viewer-frame/docx-protocol.ts';
import { picturesDocx, variedDocx } from '../support/hostile-docx.ts';
import { koreanDocx } from '../support/viewer-files.ts';
import { drawAsWorker } from '../support/worker-globals.ts';

/** A tree as text: each element with its attributes sorted (its style as parsed), then its children. */
function canonical(root: ParentNode): string {
  const lines: string[] = [];
  const walk = (node: ChildNode, depth: number): void => {
    const pad = ' '.repeat(depth);
    if (node instanceof Text) {
      if (node.data !== '') lines.push(`${pad}"${node.data}"`);
      return;
    }
    if (!(node instanceof Element)) return;
    const attributes = [...node.attributes]
      .filter((attribute) => attribute.name !== 'style' && attribute.name !== 'src')
      .map((attribute) => `${attribute.name}=${attribute.value}`)
      .sort();
    const style =
      node instanceof HTMLElement || node instanceof SVGElement ? node.style.cssText : '';
    lines.push(
      `${pad}<${node.namespaceURI ?? ''} ${node.localName} ${attributes.join(' ')} {${style}}`,
    );
    for (const child of node.childNodes) walk(child, depth + 1);
  };
  for (const child of root.childNodes) walk(child, 0);
  return lines.join('\n');
}

/** docx-preview drawing on jsdom's DOM: the style sheets and the pages, as it makes them. */
async function native(file: Buffer): Promise<{ styles: HTMLElement; body: HTMLElement }> {
  const nodes = await renderDocument(
    await parseAsync(Uint8Array.from(file).buffer, OPTIONS),
    OPTIONS,
  );
  const isStyle = (node: Node): boolean => node.nodeName === 'STYLE';
  const styles = document.createElement('div');
  const body = document.createElement('div');
  styles.append(...nodes.filter(isStyle));
  body.append(...nodes.filter((node) => !isStyle(node)));
  return { styles, body };
}

/** The worker's drawing, built by the frame's builder. */
async function built(file: Buffer): Promise<{ styles: HTMLElement; body: HTMLElement }> {
  const answer = await drawAsWorker(file);
  if (answer.kind !== 'drawn') throw new Error(JSON.stringify(answer));
  const styles = document.createElement('div');
  const body = document.createElement('div');
  await build(answer.styles, styles);
  await build(answer.body, body);
  return { styles, body };
}

describe('the Word worker draws what docx-preview draws', () => {
  it.each([
    ['the Korean sample', koreanDocx],
    ['pictures inside paragraphs', () => picturesDocx(3)],
    ['tables, lists, links, bookmarks, notes, sections, tabs, symbols', variedDocx],
  ])('%s, node for node', async (_, make) => {
    const file = make();
    const [expected, drawn] = [await native(file), await built(file)];
    const tree = canonical(drawn.body);
    expect(tree.split('\n').length).toBeGreaterThan(10);
    expect(tree).toBe(canonical(expected.body));
    expect(drawn.styles.textContent).toBe(expected.styles.textContent);
  });

  it('a picture stays inside its paragraph', async () => {
    const { body } = await built(picturesDocx(1));
    expect(body.querySelector('p > span > div > img')).not.toBeNull();
  });
});

describe('the frame builds only plain names, and nothing that runs', () => {
  it('leaves out odd tags (keeping their children) and odd or script attributes', async () => {
    const into = document.createElement('div');
    await build(
      [
        OPEN,
        'p',
        0,
        3,
        'class',
        'a',
        'onclick',
        'alert(1)',
        'x y',
        'z',
        OPEN,
        'bad tag',
        0,
        0,
        TEXT,
        'kept',
        CLOSE,
        OPEN,
        'svg',
        1,
        1,
        'onload',
        'alert(1)',
        CLOSE,
        CLOSE,
      ],
      into,
    );
    expect(into.innerHTML).toBe('<p class="a">kept<svg></svg></p>');
    expect(into.querySelector('svg')?.namespaceURI).toBe('http://www.w3.org/2000/svg');
  });

  it('filters href, src, and xlink:href to safe URL schemes only', async () => {
    const into = document.createElement('div');
    await build(
      [
        OPEN,
        'div',
        0,
        0,
        OPEN,
        'a',
        0,
        1,
        'href',
        'javascript:alert(1)',
        TEXT,
        'bad',
        CLOSE,
        OPEN,
        'a',
        0,
        1,
        'href',
        'https://example.com/ok',
        TEXT,
        'ok',
        CLOSE,
        OPEN,
        'a',
        0,
        1,
        'href',
        '#mark',
        TEXT,
        'anchor',
        CLOSE,
        OPEN,
        'img',
        0,
        1,
        'src',
        'data:image/svg+xml;base64,PHN2Zz4=',
        CLOSE,
        OPEN,
        'img',
        0,
        1,
        'src',
        'blob:null/1234',
        CLOSE,
        OPEN,
        'img',
        0,
        1,
        'src',
        'data:image/png;base64,iVBORw0KGgo=',
        CLOSE,
        CLOSE,
      ],
      into,
    );
    const links = [...into.querySelectorAll('a')].map((a) => a.getAttribute('href'));
    expect(links).toEqual([null, 'https://example.com/ok', '#mark']);
    const images = [...into.querySelectorAll('img')].map((img) => img.getAttribute('src'));
    expect(images).toEqual([null, 'blob:null/1234', 'data:image/png;base64,iVBORw0KGgo=']);
  });

  it('refuses steps out of shape', async () => {
    await expect(build([OPEN, 'p', 0, 1, 'class'], document.createElement('div'))).rejects.toThrow(
      BrokenSteps,
    );
    await expect(build([9], document.createElement('div'))).rejects.toThrow(BrokenSteps);
  });
});
