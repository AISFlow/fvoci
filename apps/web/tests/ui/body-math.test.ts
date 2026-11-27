/**
 * Formulas in the read-only view, drawn by the real formula library in the
 * simulated page: what a formula turns into, what a hostile one cannot do,
 * and that a body without formulas never fetches the library.
 *
 * (How long a formula takes and how it looks need a real browser:
 * e2e/embeds.spec.ts and e2e/editor-attacks.spec.ts.)
 */
import { render } from '@testing-library/svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import BodyView from '$lib/editor/BodyView.svelte';
import { drawEmbeds } from '$lib/editor/embeds.ts';
import * as mathModule from '$lib/editor/math.ts';
import { drawMath, KATEX_OPTIONS, loadMath } from '$lib/editor/math.ts';
import { renderBody } from '$lib/editor/render.ts';
import { MAX_LATEX, type DocNode } from '$lib/editor/schema.ts';

import { doc, math, mathBlock, paragraph, text } from '../support/body-docs.ts';

// Counts how often the library is asked for, and otherwise changes nothing.
vi.mock('$lib/editor/math.ts', async (original) => {
  const actual = await original<typeof mathModule>();
  return { ...actual, loadMath: vi.fn(actual.loadMath) };
});
// A diagram needs a browser; none is drawn here.
const drawDiagram = vi.fn(() => Promise.resolve({ ok: false, reason: 'failed' }));
vi.mock('$lib/editor/diagram.ts', () => ({ drawDiagram: () => drawDiagram() }));

const EVIL = 'https://evil.example';
const host = document.createElement('div');
document.body.append(host);

beforeEach(() => {
  vi.mocked(loadMath).mockClear();
  drawDiagram.mockClear();
});
/** The clock held still by `show`, while it draws. */
let clock: { mockRestore: () => void } | undefined;
afterEach(() => {
  // Also when a drawing never ended: the clock goes on for the next test.
  clock?.mockRestore();
  clock = undefined;
  host.replaceChildren();
  delete (window as { pwned?: unknown }).pwned;
});

/**
 * Puts a body into the page and draws what is to be drawn in it. The page
 * stops drawing formulas once the library has taken its budget of time
 * (embeds.ts `MATH_BUDGET`), and leaves the rest as they are: on a busy
 * machine the deep formulas here took it all. The clock stands still while
 * they are drawn, so what is drawn does not depend on how busy the machine is.
 */
async function show(body: DocNode): Promise<HTMLElement> {
  const rendered = renderBody(body);
  if (!rendered.ok) throw new Error(rendered.problems.join(' / '));
  host.replaceChildren(rendered.fragment);
  clock = vi.spyOn(performance, 'now').mockReturnValue(0);
  drawEmbeds(host);
  await settled();
  clock.mockRestore();
  clock = undefined;
  return host;
}

/**
 * Waits until no formula is left waiting to be drawn: until every slot says
 * how it ended (`data-state`). Only the test's own time limit bounds it; a
 * poll with a limit of its own (a second, `vi.waitFor`) gave up while a busy
 * machine was still drawing the deep ones.
 */
async function settled(): Promise<void> {
  const waiting = (): boolean =>
    Array.from(host.querySelectorAll('.doc-math')).some((slot) => !slot.hasAttribute('data-state'));
  if (!waiting()) return;
  await new Promise<void>((resolve) => {
    const observer = new MutationObserver(() => {
      if (waiting()) return;
      observer.disconnect();
      resolve();
    });
    observer.observe(host, { subtree: true, childList: true, attributeFilter: ['data-state'] });
  });
}

describe('how the formula library is run', () => {
  it('does not trust its input, and has limits', () => {
    expect(KATEX_OPTIONS.trust).toBe(false);
    expect(KATEX_OPTIONS.throwOnError).toBe(true);
    expect(KATEX_OPTIONS.maxExpand).toBeLessThanOrEqual(1000);
    expect(KATEX_OPTIONS.maxSize).toBeLessThanOrEqual(50);
    expect(KATEX_OPTIONS.globalGroup).toBe(false);
    expect(KATEX_OPTIONS.strict('htmlExtension')).toBe('error');
    expect(KATEX_OPTIONS.strict('unicodeTextInMathMode')).toBe('ignore');
    // No colour of its own: what it paints red takes the app's token.
    expect(KATEX_OPTIONS.errorColor).toBe('var(--destructive)');
    expect('macros' in KATEX_OPTIONS).toBe(false);
  });
});

describe('a formula in the view', () => {
  it('is its source until it is drawn, then the drawing, with the source as its name', async () => {
    const rendered = renderBody(
      doc(paragraph(text('a '), math('x^2'), text(' b')), mathBlock('\\frac{1}{2}')),
    );
    if (!rendered.ok) throw new Error('not drawn');
    host.replaceChildren(rendered.fragment);
    const inline = host.querySelector<HTMLElement>('span.doc-math');
    const block = host.querySelector<HTMLElement>('div.doc-math');
    expect(inline?.outerHTML).toBe(
      '<span data-type="inline-math" data-latex="x^2" class="doc-math">x^2</span>',
    );
    expect(block?.outerHTML).toBe(
      '<div data-type="block-math" data-latex="\\frac{1}{2}" class="doc-math">\\frac{1}{2}</div>',
    );

    drawEmbeds(host);
    await settled();
    for (const slot of [inline, block]) {
      expect(slot?.getAttribute('data-state')).toBe('drawn');
      expect(slot?.getAttribute('role')).toBe('math');
      expect(slot?.querySelector('.katex-html')).not.toBeNull();
      // MathML for a screen reader, next to the drawing.
      expect(slot?.querySelector('math')).not.toBeNull();
    }
    expect(inline?.getAttribute('aria-label')).toBe('x^2');
    expect(inline?.querySelector('.katex-display')).toBeNull();
    expect(block?.querySelector('.katex-display')).not.toBeNull();
    // The words around it are untouched.
    expect(host.querySelector('p')?.firstChild?.nodeValue).toBe('a ');
  });

  it('shows the source of the formulas left once the budget of time is spent, and says so', async () => {
    const sources = ['a', 'b', 'c', 'd', 'e', 'f'];
    const rendered = renderBody(doc(paragraph(...sources.map((latex) => math(latex)))));
    if (!rendered.ok) throw new Error('not drawn');
    host.replaceChildren(rendered.fragment);
    // Every reading of the clock is a second later: the budget of three is soon spent.
    let tick = 0;
    clock = vi.spyOn(performance, 'now').mockImplementation(() => (tick += 1000));
    drawEmbeds(host);
    await settled();
    clock.mockRestore();
    clock = undefined;
    const slots = Array.from(host.querySelectorAll<HTMLElement>('span.doc-math'));
    const states = slots.map((slot) => slot.getAttribute('data-state'));
    // The first ones are drawn until the budget is spent; every one after says it is not.
    const drawn = states.filter((state) => state === 'drawn').length;
    expect(drawn).toBeGreaterThan(0);
    expect(drawn).toBeLessThan(sources.length);
    expect(states).toEqual(sources.map((_source, index) => (index < drawn ? 'drawn' : 'skipped')));
    for (const [index, slot] of slots.slice(drawn).entries()) {
      // Its source, and a line that says it was not drawn, read where it stands.
      expect(slot.firstChild?.nodeValue).toBe(sources[index + drawn]);
      expect(slot.querySelector('[role="note"]')?.textContent).toBe(
        'Not drawn: drawing the formulas here took too long.',
      );
      expect(slot.querySelector('.katex')).toBeNull();
    }
  });

  it('takes text of any script without complaint', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const root = await show(
      doc(paragraph(math('속도 = \\frac{거리}{시간}'), math('面積 = \\pi r^2'))),
    );
    expect(root.querySelectorAll('[data-state="drawn"]')).toHaveLength(2);
    expect(root.textContent).toContain('속도');
    expect(warn).not.toHaveBeenCalled();
    warn.mockRestore();
  });

  it.each([
    ['a source that cannot be read', '\\frac{1}{'],
    ['a command that does not exist', '\\nosuchcommand{1}'],
    ['a macro that calls itself without end', '\\def\\a{\\a\\a}\\a'],
    [
      'macros that multiply',
      '\\def\\a{xxxxxxxxxx}\\def\\b{\\a\\a\\a\\a\\a\\a\\a\\a\\a\\a}\\def\\c{\\b\\b\\b\\b\\b\\b\\b\\b\\b\\b}\\def\\d{\\c\\c\\c\\c\\c\\c\\c\\c\\c\\c}\\def\\e{\\d\\d\\d\\d\\d\\d\\d\\d\\d\\d}\\e',
    ],
  ])('shows the source and a notice for %s', async (_name, latex) => {
    const root = await show(doc(mathBlock(latex)));
    const slot = root.querySelector('.doc-math');
    expect(slot?.getAttribute('data-state')).toBe('error');
    expect(slot?.firstChild?.nodeValue).toBe(latex);
    expect(slot?.querySelector('[role="note"]')?.textContent).toBe('This formula cannot be drawn.');
    expect(slot?.querySelector('.katex')).toBeNull();
    expect(slot?.hasAttribute('role')).toBe(false);
  });

  it('does not draw a source longer than the limit, however it got here', async () => {
    const element = document.createElement('span');
    element.textContent = 'before';
    const library = await mathModule.loadMath();
    expect(drawMath(library, element, 'x'.repeat(MAX_LATEX + 1), false)).toEqual({
      ok: false,
      reason: 'too-long',
      ms: 0,
    });
    expect(element.textContent).toBe('before');
    expect(drawMath(library, element, 'x'.repeat(40), false).ok).toBe(true);
    // The validator refuses such a document before anything is drawn.
    expect(renderBody(doc(mathBlock('x'.repeat(MAX_LATEX + 1)))).ok).toBe(false);
  });

  it('does not let a formula into the page that is nested too deep to lay out', async () => {
    const nested = (levels: number): string => `${'\\sqrt{'.repeat(levels)}x${'}'.repeat(levels)}`;
    // (As deep as the limit on the source allows, 570, is tried in a real
    // browser; the simulated page is far too slow at building such a tree.)
    const root = await show(
      doc(mathBlock(nested(30)), mathBlock(nested(40)), paragraph(math(nested(80)))),
    );
    const [fine, deep, deepest] = Array.from(root.querySelectorAll('.doc-math'));
    expect(fine?.getAttribute('data-state')).toBe('drawn');
    for (const slot of [deep, deepest]) {
      expect(slot?.getAttribute('data-state')).toBe('error');
      expect(slot?.querySelector('.katex')).toBeNull();
      expect(slot?.querySelector('[role="note"]')?.textContent).toBe(
        'This formula is too large to show.',
      );
    }
    expect(deepest?.firstChild?.nodeValue).toBe(nested(80));
    // Building the three trees takes the simulated page seconds on a busy machine:
    // twice it ran past the default five while every core was taken.
  }, 30_000);

  it('keeps a definition inside the formula that made it', async () => {
    const root = await show(
      doc(
        mathBlock('\\gdef\\frac#1#2{PWNED}\\def\\x{y}\\frac{1}{2}'),
        mathBlock('\\frac{1}{2}'),
        mathBlock('\\x'),
      ),
    );
    const [first, second, third] = Array.from(root.querySelectorAll('.doc-math'));
    expect(first?.textContent).toContain('PWNED');
    expect(second?.getAttribute('data-state')).toBe('drawn');
    expect(second?.textContent).not.toContain('PWNED');
    expect(second?.querySelector('.mfrac')).not.toBeNull();
    // `\x` was defined in the first formula only: here it is unknown.
    expect(third?.getAttribute('data-state')).toBe('error');
  });

  it('leaves alone what is no place for a formula', async () => {
    host.innerHTML =
      '<p><span class="doc-math">no type</span><span data-type="inline-math" class="doc-math">no source</span>' +
      '<b data-type="inline-math" data-latex="x" class="doc-math">wrong element</b><span data-type="inline-math" data-latex="x">no class</span></p>';
    const before = host.innerHTML;
    drawEmbeds(host);
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(host.innerHTML).toBe(before);
    expect(loadMath).not.toHaveBeenCalled();
  });
});

/** Formulas that try to do something other than be a formula. */
const HOSTILE: [string, string][] = [
  ['a link', `\\href{${EVIL}/href}{click}`],
  ['a link that is a script', '\\href{javascript:window.pwned=1}{click}'],
  ['a link with a data address', '\\href{data:text/html,<script>window.pwned=1</script>}{click}'],
  ['an address shown as a link', `\\url{${EVIL}/url}`],
  ['a picture by address', `\\includegraphics[width=1em]{${EVIL}/pixel.png}`],
  [
    'a picture with a data address',
    '\\includegraphics{data:image/svg+xml,<svg onload="window.pwned=1"/>}',
  ],
  ['classes of the page', '\\htmlClass{fixed inset-0 doc-body mention}{x}'],
  ['an identifier of the page', '\\htmlId{app}{x}'],
  ['a style', `\\htmlStyle{position:fixed;inset:0;background:url(${EVIL}/style.png)}{x}`],
  [
    'attributes the editor reads',
    '\\htmlData{mention=,user-id=u1,file-id=f1,type=inline-math,latex=y}{x}',
  ],
  ['markup as text', '\\text{<script>window.pwned=1</script><img src=x onerror=window.pwned=1>}'],
  [
    'markup spelled character by character',
    '\\char"3C script\\char"3E window.pwned=1 \\char"3C /script\\char"3E',
  ],
  ['markup in verbatim', `\\verb|<img src="${EVIL}/verb.png" onerror="window.pwned=1">|`],
  ['a colour that carries a style', `\\textcolor{red;background:url(${EVIL}/color.png)}{x}`],
  ['a colour box that carries a style', `\\colorbox{red;background:url(${EVIL}/box.png)}{x}`],
  ['a tag that carries markup', `\\tag{<img src="${EVIL}/tag.png" onerror="window.pwned=1">} x`],
  ['a rule the size of the page', '\\rule{99999em}{99999em}'],
  ['spaces without end', '\\hspace{99999em}x\\kern99999em\\mkern99999mu'],
  ['a quote that would end an attribute', 'x" onmouseover="window.pwned=1" y=\'z\' <b>'],
];

describe('a hostile formula', () => {
  it.each(HOSTILE)(
    '%s: neither links, fetches, runs, nor reaches into the page',
    async (_name, latex) => {
      const root = await show(
        doc(paragraph(text('before '), math(latex), text(' after')), mathBlock(latex)),
      );
      const slots = Array.from(root.querySelectorAll<HTMLElement>('.doc-math'));
      expect(slots).toHaveLength(2);

      for (const slot of slots) {
        // The source is kept where the view put it, as an attribute and nowhere as markup.
        expect(slot.getAttribute('data-latex')).toBe(latex);
        for (const element of Array.from(slot.querySelectorAll<HTMLElement>('*'))) {
          // No link, no picture, nothing that runs or embeds.
          expect([
            'a',
            'img',
            'image',
            'script',
            'iframe',
            'object',
            'embed',
            'link',
            'style',
            'form',
            'input',
            'foreignObject',
          ]).not.toContain(element.localName);
          for (const { name, value } of Array.from(element.attributes)) {
            expect([name, name.startsWith('on')]).toEqual([name, false]);
            expect(['id', 'href', 'src', 'srcset', 'action', 'name', 'target']).not.toContain(name);
            expect([name, name.startsWith('data-')]).toEqual([name, false]);
            if (name === 'class') {
              // Only the library's own classes and the wrapper's: none of the page's.
              for (const token of value.split(/\s+/).filter(Boolean)) {
                expect(['fixed', 'inset-0', 'doc-body', 'mention']).not.toContain(token);
              }
            }
            if (name === 'style') {
              expect(value).not.toMatch(/url\s*\(|position|inset|background|expression/i);
            }
          }
          // No style reaches outside the formula or asks for more room than `maxSize`.
          // (The MathML half has no styles at all; only the drawing does.)
          const style = (element as Partial<HTMLElement>).style;
          if (!style) continue;
          expect(style.position).toBe('');
          expect(style.backgroundImage).toBe('');
          for (const length of [style.width, style.height, style.marginRight]) {
            const ems = /^(-?[\d.]+)em$/.exec(length)?.[1];
            if (ems !== undefined)
              expect(Math.abs(Number(ems))).toBeLessThanOrEqual(KATEX_OPTIONS.maxSize);
          }
        }
      }
      // Nothing outside the two formulas changed, and nothing ran.
      expect(root.querySelector('p')?.firstChild?.nodeValue).toBe('before ');
      expect(root.querySelector('p')?.lastChild?.nodeValue).toBe(' after');
      expect(root.querySelectorAll('a, img, script, iframe')).toHaveLength(0);
      expect(document.querySelectorAll('script[src], link[href*="evil"]')).toHaveLength(0);
      expect((window as { pwned?: unknown }).pwned).toBeUndefined();
    },
  );

  it('refuses the commands that set attributes outright', async () => {
    const root = await show(
      doc(
        mathBlock('\\htmlClass{fixed}{x}'),
        mathBlock('\\htmlId{app}{x}'),
        mathBlock('\\htmlStyle{color:red}{x}'),
        mathBlock('\\htmlData{a=b}{x}'),
      ),
    );
    expect(root.querySelectorAll('.doc-math[data-state="error"]')).toHaveLength(4);
    expect(root.querySelectorAll('.katex')).toHaveLength(0);
  });

  it('paints a command it refuses in the colour for errors, as text', async () => {
    const root = await show(doc(mathBlock(`\\href{${EVIL}/x}{label}`)));
    const slot = root.querySelector('.doc-math');
    expect(slot?.getAttribute('data-state')).toBe('drawn');
    expect(slot?.querySelector('a')).toBeNull();
    expect(slot?.querySelector('.katex-html')?.textContent).toContain('\\href');
    const colours = Array.from(slot?.querySelectorAll<HTMLElement>('.katex-html *') ?? [])
      .map((element) => element.style.color)
      .filter((value) => value !== '');
    expect(new Set(colours)).toEqual(new Set(['var(--destructive)']));
  });
});

describe('the component', () => {
  it('draws the formulas of a body, and again when the body changes', async () => {
    const { container, rerender } = render(BodyView, { doc: doc(paragraph(math('a^2'))) });
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-math[data-state="drawn"]')).not.toBeNull();
    });
    await rerender({ doc: doc(mathBlock('b^2'), mathBlock('\\frac{')) });
    await vi.waitFor(() => {
      expect(container.querySelectorAll('.doc-math[data-state]')).toHaveLength(2);
    });
    expect(container.querySelector('span.doc-math')).toBeNull();
    expect(container.querySelector('[data-state="drawn"]')?.getAttribute('aria-label')).toBe('b^2');
    expect(container.querySelector('[data-state="error"] [role="note"]')).not.toBeNull();
  });

  it('never asks for the formula library when a body has no formula', async () => {
    const { container, rerender } = render(BodyView, {
      doc: doc(paragraph(text('$x^2$ is text, not a formula')), {
        type: 'codeBlock',
        attrs: { language: 'latex' },
        content: [text('\\frac{1}{2}')],
      }),
    });
    await rerender({ doc: doc(paragraph(text('still none'))) });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(container.querySelector('.doc-body')?.textContent).toBe('still none');
    expect(loadMath).not.toHaveBeenCalled();
    expect(drawDiagram).not.toHaveBeenCalled();

    await rerender({ doc: doc(paragraph(math('x'))) });
    await vi.waitFor(() => {
      expect(loadMath).toHaveBeenCalled();
    });
    expect(drawDiagram).not.toHaveBeenCalled();
  });

  it('asks for a diagram to be drawn only for a code block in the language of diagrams', async () => {
    const code = (language: string | undefined, value: string): DocNode => ({
      type: 'codeBlock',
      ...(language === undefined ? {} : { attrs: { language } }),
      content: [text(value)],
    });
    const { container, rerender } = render(BodyView, {
      doc: doc(
        code('ts', 'flowchart LR'),
        code(undefined, 'graph TD'),
        code('Mermaid', 'pie'),
        code('mermaid2', 'pie'),
      ),
    });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(drawDiagram).not.toHaveBeenCalled();
    expect(container.querySelector('.doc-diagram')).toBeNull();

    await rerender({ doc: doc(code('mermaid', 'flowchart LR\n  A --> B')) });
    await vi.waitFor(() => {
      expect(drawDiagram).toHaveBeenCalledTimes(1);
    });
    // It could not be drawn (here, never): the source stays in sight, with a notice behind it.
    await vi.waitFor(() => {
      expect(container.querySelector('.doc-diagram[data-state="error"]')).not.toBeNull();
    });
    const block = container.querySelector('pre');
    expect(block?.hidden).toBe(false);
    expect(block?.textContent).toBe('flowchart LR\n  A --> B');
    expect(block?.nextElementSibling?.querySelector('[role="note"]')?.textContent).toBe(
      'This diagram cannot be drawn.',
    );
    expect(loadMath).not.toHaveBeenCalled();
  });
});
