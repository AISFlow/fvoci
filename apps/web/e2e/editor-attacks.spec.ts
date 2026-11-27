/**
 * Hostile diagrams and formulas in the body of a card, run through the real
 * libraries in a real browser, the way a body written by one member reaches
 * the screen of another: in the editor of a card the reader may change, and
 * in the view of one they may only read.
 *
 * For every attack, in the editor and in the view alike:
 *
 * - the page asks no other host for anything, while drawing or after;
 * - no script runs;
 * - what ends up in the page holds no script, no link, no handler, nothing
 *   that names an address outside it, and no HTML inside a picture;
 * - the page around it is as it was.
 *
 * The filter behind the diagram library is tested on its own, with pictures
 * written by hand, in tests/ui/body-safe-svg.test.ts; the formula library's
 * settings in tests/ui/body-math.test.ts. This is the whole chain.
 *
 * One project only: the attacks do not depend on the size of the screen, and
 * every one of them loads the page anew.
 */
import { expect, test, type Locator, type Page } from '@playwright/test';

import type * as MathModule from '../src/lib/editor/math.ts';

import { diagramBody, openCardWithBody } from './embeds.support.ts';

const EVIL = 'https://evil.example';

/** The board whose cards the reader may change, and the one whose cards they may only read. */
const WRITABLE = 'Getting started';
const READ_ONLY = 'Study notes';

/** What `window.fvociDev` offers the test of the formula library's time (src/app/backend.ts). */
interface MathDev {
  math(): Promise<typeof MathModule>;
}

type Outcome =
  /** A picture is shown. */
  | 'drawn'
  /** Refused before it is drawn: a picture by address (or a setting that survived the lock; none does today). */
  | 'not allowed'
  /** The library could not read it. */
  | 'cannot be drawn';

const NOTICE: Record<Exclude<Outcome, 'drawn'>, string> = {
  'not allowed':
    'This diagram is not drawn: it carries settings of its own or fetches a picture from an address.',
  'cannot be drawn': 'This diagram cannot be drawn.',
};

/** Diagram sources, by what they try, and what becomes of them. */
const DIAGRAMS: [name: string, outcome: Outcome, source: string][] = [
  // Links and clicks.
  [
    'a click that opens an address',
    'drawn',
    `flowchart LR\n  A-->B\n  click A href "${EVIL}/click" _blank`,
  ],
  [
    'a click that runs a script address',
    'drawn',
    'flowchart LR\n  A-->B\n  click A href "javascript:window.pwned=1"',
  ],
  ['a click that calls a function', 'drawn', 'flowchart LR\n  A-->B\n  click A call alert(1)'],
  ['a click with a callback', 'drawn', 'flowchart LR\n  A-->B\n  click A callback "tip"'],
  [
    'links on a participant',
    'drawn',
    `sequenceDiagram\n  participant A\n  link A: Dash @ ${EVIL}/seq\n  links A: {"x": "javascript:window.pwned=1"}\n  A->>A: hi`,
  ],
  [
    'a click and a link on a class',
    'drawn',
    `classDiagram\n  class A\n  click A href "${EVIL}/class" "tip"\n  link A "javascript:window.pwned=1"`,
  ],
  ['a callback on a class', 'drawn', 'classDiagram\n  class A\n  callback A "alert" "tip"'],
  [
    'clicks on a task',
    'drawn',
    `gantt\n  dateFormat YYYY-MM-DD\n  section S\n  T :a1, 2026-01-01, 3d\n  click a1 href "${EVIL}/gantt"\n  click a1 call alert()`,
  ],
  ['a click on a state', 'drawn', `stateDiagram-v2\n  A --> B\n  click A href "${EVIL}/state"`],
  [
    'a link on a person',
    'drawn',
    `C4Context\n  Person(a, "A", $link="${EVIL}/c4")\n  System(b, "B", $sprite="img:${EVIL}/sprite.png")`,
  ],
  // HTML in labels.
  ['a script in a label', 'drawn', 'flowchart LR\n  A["<script>window.pwned=1</script>x"]-->B'],
  [
    'a picture with a handler in a label',
    'drawn',
    `flowchart LR\n  A["<img src='${EVIL}/img.png' onerror='window.pwned=1'>"]-->B`,
  ],
  [
    'a picture in a picture in a label',
    'drawn',
    `flowchart LR\n  A["<svg onload='window.pwned=1'><image href='${EVIL}/svg.png'/></svg>"]-->B`,
  ],
  ['a frame in a label', 'drawn', `flowchart LR\n  A["<iframe src='${EVIL}/frame'></iframe>"]-->B`],
  [
    'a style sheet in a label',
    'drawn',
    `flowchart LR\n  A["<style>@import url(${EVIL}/import.css);</style>t"]-->B`,
  ],
  [
    'a script link in a label',
    'drawn',
    `flowchart LR\n  A["<a href='javascript:window.pwned=1'>x</a>"]-->B`,
  ],
  [
    'markup in a Markdown label',
    'drawn',
    'flowchart LR\n  A["`**bold** <img src=x onerror=window.pwned=1>`"]-->B',
  ],
  [
    'markup spelled with entities',
    'drawn',
    'flowchart LR\n  A["#lt;img src=x onerror=window.pwned=1#gt;"]-->B',
  ],
  [
    'markup in a message',
    'drawn',
    `sequenceDiagram\n  A->>B: <img src="${EVIL}/seqimg.png" onerror="window.pwned=1">`,
  ],
  [
    'markup in a mind map',
    'drawn',
    `mindmap\n  root\n    A\n    ::icon(fa fa-book)\n    B["<img src='${EVIL}/mm.png'>"]`,
  ],
  [
    'formulas in labels that link and fetch',
    'drawn',
    `flowchart LR\n  A["$$\\\\href{javascript:window.pwned=1}{x}$$"]-->B["$$\\\\includegraphics{${EVIL}/katex.png}$$"]`,
  ],
  // Styles that fetch.
  [
    'a fill by address',
    'cannot be drawn',
    `flowchart LR\n  A-->B\n  style A fill:url(${EVIL}/fill.svg#a)`,
  ],
  [
    'a background and a mask by address',
    'cannot be drawn',
    `flowchart LR\n  A-->B\n  style A background-image:url(${EVIL}/bg.png),mask-image:url(${EVIL}/mask.png)`,
  ],
  [
    'a class with a mask by address',
    'cannot be drawn',
    `flowchart LR\n  A:::x-->B\n  classDef x mask-image:url(${EVIL}/classdef.png),fill:#f00`,
  ],
  [
    'a class with an image set',
    'cannot be drawn',
    `flowchart LR\n  A:::x-->B\n  classDef x mask-image:image-set("${EVIL}/imageset.png" 1x)`,
  ],
  [
    'a class with an escaped address',
    'cannot be drawn',
    `flowchart LR\n  A:::x-->B\n  classDef x mask-image:\\75rl(${EVIL}/escaped.png)`,
  ],
  [
    'a link style by address',
    'cannot be drawn',
    `flowchart LR\n  A-->B\n  linkStyle 0 stroke:url(${EVIL}/link.svg#a),marker-end:url(${EVIL}/marker.svg#m)`,
  ],
  [
    'an entity styled by address',
    'cannot be drawn',
    `erDiagram\n  A ||--o{ B : x\n  style A fill:url(${EVIL}/er.svg#a),mask-image:url(${EVIL}/er.png)`,
  ],
  // Settings of its own: every setting the library knows is locked, so they are dropped and the
  // diagram is drawn with ours. (What matters is asserted for every row: nothing fetched, nothing run.)
  [
    'a style sheet of its own',
    'drawn',
    `%%{init: {"themeCSS": "svg{background:url(${EVIL}/themecss.png)} body{display:none}"}}%%\nflowchart LR\n  A-->B`,
  ],
  [
    'a style sheet of its own that imports',
    'drawn',
    `%%{init: {"themeCSS": "@import url(${EVIL}/import2.css);"}}%%\nflowchart LR\n  A-->B`,
  ],
  [
    'a font that carries a background',
    'drawn',
    `%%{init: {"fontFamily": "x;background:url(${EVIL}/font.png)"}}%%\nflowchart LR\n  A-->B`,
  ],
  [
    'colours that carry a background',
    'drawn',
    `%%{init: {"themeVariables": {"primaryColor": "red;background:url(${EVIL}/var.png)", "fontFamily": "x;background-image:url(${EVIL}/var2.png)"}}}%%\nflowchart LR\n  A-->B`,
  ],
  [
    'a looser security level',
    'drawn',
    `%%{init: {"securityLevel": "loose", "htmlLabels": true, "flowchart": {"htmlLabels": true}}}%%\nflowchart LR\n  A["<img src='${EVIL}/loose.png'>"]-->B\n  click A href "javascript:window.pwned=1"`,
  ],
  [
    'HTML labels by a directive',
    'drawn',
    `%%{init: {"htmlLabels": true}}%%\nflowchart LR\n  A["<img src='${EVIL}/html.png'>"]-->B`,
  ],
  [
    'a directive spelled initialize',
    'drawn',
    `%%{initialize: {"htmlLabels": true, "securityLevel": "antiscript"}}%%\nflowchart LR\n  A["<img src='${EVIL}/initialize.png'>"]-->B`,
  ],
  [
    'settings in front matter',
    'drawn',
    `---\nconfig:\n  securityLevel: loose\n  htmlLabels: true\n  themeCSS: "svg{background:url(${EVIL}/front.png)}"\n---\nflowchart LR\n  A["<img src='${EVIL}/front2.png'>"]-->B`,
  ],
  [
    'settings in front matter, the key quoted',
    'drawn',
    `---\n"config": {"htmlLabels": true, "securityLevel": "loose"}\n---\nflowchart LR\n  A["<img src='${EVIL}/front3.png'>"]-->B`,
  ],
  // A setting the library does not know is dropped by it as well.
  [
    'a setting nobody knows',
    'drawn',
    '%%{init: {"notASettingAnyoneKnows": true}}%%\nflowchart LR\n  A-->B',
  ],
  [
    'an unknown setting in front matter',
    'drawn',
    '---\nconfig:\n  somethingNew:\n    htmlLabels: true\n---\nflowchart LR\n  A-->B',
  ],
  // Pictures the library would fetch while it draws.
  [
    'a node drawn as a picture',
    'not allowed',
    `flowchart LR\n  A@{ img: "${EVIL}/shape.png", label: "x", w: 60, h: 60 }\n  A-->B`,
  ],
  [
    'a node drawn as a picture, the key quoted',
    'not allowed',
    `flowchart LR\n  A@{ "img": "${EVIL}/shape2.png", "label": "x" }\n  A-->B`,
  ],
  [
    'a node drawn as a picture, the key escaped',
    'not allowed',
    `flowchart LR\n  A@{ "\\x69mg": "${EVIL}/shape3.png", label: "x" }\n  A-->B`,
  ],
  [
    'a node drawn as a picture behind a brace in a label',
    'not allowed',
    `flowchart LR\n  A@{ label: "}", img: "${EVIL}/shape4.png" }\n  A-->B`,
  ],
  [
    'an icon from a pack that is not there',
    'drawn',
    'architecture-beta\n  service db(logos:aws)[DB]',
  ],
];

/** Requests to anywhere but the server the page came from. */
function outside(page: Page): string[] {
  const seen: string[] = [];
  const own = new URL(test.info().project.use.baseURL ?? 'http://127.0.0.1').origin;
  page.on('request', (request) => {
    const url = request.url();
    if (!url.startsWith(own) && !url.startsWith('data:') && !url.startsWith('blob:'))
      seen.push(url);
  });
  return seen;
}

/** What is wrong with what a body shows for a diagram or a formula; empty when nothing is. */
function findings(body: Locator, selector: string): Promise<string[]> {
  return body.evaluate((root, drawnSelector) => {
    const found: string[] = [];
    const local = /url\(\s*["']?#[^)"']+["']?\s*\)/g;
    for (const drawn of Array.from(root.querySelectorAll(drawnSelector))) {
      for (const element of Array.from(drawn.querySelectorAll('*'))) {
        const name = element.localName.toLowerCase();
        if (
          [
            'script',
            'a',
            'image',
            'img',
            'foreignobject',
            'iframe',
            'object',
            'embed',
            'feimage',
            'form',
            'link',
            'base',
            'meta',
          ].includes(name)
        )
          found.push(`<${element.localName}>`);
        for (const { name: attribute, value } of Array.from(element.attributes)) {
          if (attribute.toLowerCase().startsWith('on')) found.push(`${attribute} on <${name}>`);
          if (attribute === 'href' || attribute === 'xlink:href') {
            if (!/^#[^\s]+$/.test(value)) found.push(`${attribute}="${value}"`);
          }
          if (/evil\.example|javascript:/i.test(value))
            found.push(`${attribute}="${value.slice(0, 60)}"`);
          if (/url\s*\(/i.test(value.replace(local, '')))
            found.push(`${attribute} fetches: ${value.slice(0, 60)}`);
        }
        if (name === 'style') {
          const sheet = element.textContent;
          if (/@import|@font-face|\\/.test(sheet)) found.push('style sheet: at-rule or escape');
          if (/url\s*\(/i.test(sheet.replace(local, ''))) found.push('style sheet: an address');
          if (/evil\.example/i.test(sheet)) found.push('style sheet: names the other host');
        }
      }
    }
    return found;
  }, selector);
}

/** Whether the page around the body is as it should be: visible, and nothing ran. */
async function pageIsWhole(page: Page): Promise<void> {
  const state = await page.evaluate(() => ({
    pwned: (window as { pwned?: unknown }).pwned ?? null,
    body: getComputedStyle(document.body).display,
    background: getComputedStyle(document.body).backgroundImage,
    scripts: Array.from(document.scripts).filter((script) =>
      /evil|pwned/.test(script.src + script.text),
    ).length,
    stage: document.querySelectorAll('.doc-diagram-stage').length,
  }));
  expect(state).toEqual({ pwned: null, body: 'block', background: 'none', scripts: 0, stage: 0 });
}

/** The document an editor holds, as it is stored. */
function documentOf(editor: Locator): Promise<unknown> {
  return editor.evaluate(
    (element: HTMLElement & { editor?: { getJSON: () => unknown } }) =>
      element.editor?.getJSON() ?? null,
  );
}

/** A card the reader may change, with this body, in its editor (not the view shown while it comes). */
async function inTheEditor(page: Page, doc: object): Promise<Locator> {
  const body = await openCardWithBody(page, WRITABLE, doc);
  await expect(body).toHaveAttribute('data-editor', 'ready');
  return body.locator('.ProseMirror');
}

/** A card the reader may only read, with this body. */
async function inTheView(page: Page, doc: object): Promise<Locator> {
  const body = await openCardWithBody(page, READ_ONLY, doc);
  await expect(body).toHaveAttribute('data-editor', 'reading');
  return body;
}

/** A body of one empty line, to paste into. */
const EMPTY = { type: 'doc', content: [{ type: 'paragraph' }] };

/** Pastes markup into an editor, as the clipboard hands it over. */
async function paste(editor: Locator, markup: string): Promise<void> {
  await editor.click();
  await editor.evaluate((element, html) => {
    const data = new DataTransfer();
    data.setData('text/html', html);
    element.dispatchEvent(
      new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }),
    );
  }, markup);
}

/** Lets anything that was set in motion ask for something, then holds that nothing did. */
async function nothingAskedOrRan(page: Page, requests: string[]): Promise<void> {
  await page.waitForLoadState('networkidle');
  await page.waitForTimeout(250);
  expect(requests).toEqual([]);
  await pageIsWhole(page);
}

test.beforeEach(({ page: _page }, testInfo) => {
  test.skip(
    testInfo.project.name !== 'desktop',
    'run once: the attacks do not depend on the screen',
  );
});

test.describe.configure({ mode: 'parallel', timeout: 90_000 });

test.describe('a hostile diagram', () => {
  for (const [name, outcome, source] of DIAGRAMS) {
    test(`${name}: ${outcome}`, async ({ page }) => {
      const requests = outside(page);
      for (const where of ['editor', 'view'] as const) {
        const body =
          where === 'editor'
            ? await inTheEditor(page, diagramBody(source))
            : await inTheView(page, diagramBody(source));
        const figure = body.locator('.doc-diagram');
        await expect(figure).toHaveCount(1);
        await expect(figure).toHaveAttribute(
          'data-state',
          outcome === 'drawn' ? 'drawn' : 'error',
          { timeout: 20_000 },
        );
        if (outcome === 'drawn') {
          // One picture. (It may hold smaller ones: an icon is drawn as a picture inside it.)
          await expect(figure.locator('> svg')).toHaveCount(1);
        } else {
          await expect(figure.getByRole('note')).toHaveText(NOTICE[outcome]);
          await expect(figure.locator('svg')).toHaveCount(0);
          // The source stays readable in the figure, where the picture would be.
          await expect(figure.locator('.doc-diagram-copy')).toBeVisible();
        }
        expect(await findings(body, '.doc-diagram')).toEqual([]);
        if (where === 'editor') {
          // The document is the code block, verbatim: nothing about a diagram is stored but its text.
          expect(await documentOf(body)).toEqual(diagramBody(source));
        }
        await nothingAskedOrRan(page, requests);
      }
    });
  }

  test('a diagram longer than the limit is not handed to the library at all', async ({ page }) => {
    const requests: string[] = [];
    page.on('request', (request) => requests.push(request.url()));
    const source = `flowchart LR\n${Array.from({ length: 1500 }, (_, index) => `  n${index} --> n${index + 1}`).join('\n')}`;
    expect(source.length).toBeGreaterThan(20_000);
    for (const where of ['editor', 'view'] as const) {
      const body =
        where === 'editor'
          ? await inTheEditor(page, diagramBody(source))
          : await inTheView(page, diagramBody(source));
      await expect(body.locator('.doc-diagram').getByRole('note')).toHaveText(
        'This diagram is too long to draw.',
      );
      await page.waitForLoadState('networkidle');
    }
    // Refused by its length alone: the library itself was never fetched.
    expect(requests.filter((url) => url.includes('mermaid'))).toEqual([]);
  });

  test('a diagram with more edges than the library allows is refused by it', async ({ page }) => {
    const source = `flowchart LR\n${Array.from({ length: 520 }, (_, index) => `  a${index}-->b${index}`).join('\n')}`;
    expect(source.length).toBeLessThan(20_000);
    const view = await inTheView(page, diagramBody(source));
    await expect(view.locator('.doc-diagram')).toHaveAttribute('data-state', 'error', {
      timeout: 30_000,
    });
    await expect(view.locator('.doc-diagram svg')).toHaveCount(0);
  });
});

/** Formulas, by what they try. In the browser the point is that nothing is fetched and nothing runs. */
const FORMULAS: [string, string][] = [
  ['a link', `\\href{${EVIL}/href}{click}`],
  ['a link that is a script', '\\href{javascript:window.pwned=1}{click}'],
  ['an address shown as a link', `\\url{${EVIL}/url}`],
  ['a picture by address', `\\includegraphics[width=1em]{${EVIL}/pixel.png}`],
  [
    'a picture with a data address',
    '\\includegraphics{data:image/svg+xml,<svg onload="window.pwned=1"/>}',
  ],
  ['classes of the page', '\\htmlClass{fixed inset-0 doc-body}{x}'],
  ['an identifier of the page', '\\htmlId{app}{x}'],
  [
    'a style that fetches',
    `\\htmlStyle{position:fixed;inset:0;background:url(${EVIL}/style.png)}{x}`,
  ],
  ['attributes the editor reads', '\\htmlData{mention=,user-id=u1,file-id=f1}{x}'],
  ['markup as text', '\\text{<script>window.pwned=1</script><img src=x onerror=window.pwned=1>}'],
  ['markup in verbatim', `\\verb|<img src="${EVIL}/verb.png" onerror="window.pwned=1">|`],
  ['a colour that carries a style', `\\textcolor{red;background:url(${EVIL}/color.png)}{x}`],
  ['a macro that calls itself without end', '\\def\\a{\\a\\a}\\a'],
  ['a rule the size of the page', '\\rule{99999em}{99999em}'],
];

/** Where a formula may be reached: everything a formula says that became an attribute of the page. */
function reachOf(body: Locator): Promise<string[]> {
  return body.evaluate((root) =>
    Array.from(root.querySelectorAll('.doc-math *')).flatMap((element) => [
      ...(element.id ? [`id=${element.id}`] : []),
      ...Array.from(element.attributes)
        .filter((attribute) => attribute.name.startsWith('data-'))
        .map((attribute) => attribute.name),
      ...Array.from(element.classList).filter((name) =>
        ['fixed', 'inset-0', 'doc-body'].includes(name),
      ),
      ...(getComputedStyle(element).position === 'fixed' ? ['position: fixed'] : []),
      ...(getComputedStyle(element).backgroundImage !== 'none' ? ['a background image'] : []),
    ]),
  );
}

test.describe('a hostile formula', () => {
  for (const [name, latex] of FORMULAS) {
    test(name, async ({ page }) => {
      const requests = outside(page);
      // Pasted as the markup a formula is copied as: the only way one comes in from outside.
      const attribute = latex
        .replaceAll('&', '&amp;')
        .replaceAll('"', '&quot;')
        .replaceAll('<', '&lt;');
      const editor = await inTheEditor(page, EMPTY);
      await paste(
        editor,
        `<p>before <span data-type="inline-math" data-latex="${attribute}">x</span> after</p><div data-type="block-math" data-latex="${attribute}">x</div>`,
      );
      await expect
        .poll(async () => JSON.stringify(await documentOf(editor)))
        .toContain(JSON.stringify({ type: 'blockMath', attrs: { latex } }));
      // What the editor stored, as a member who may only read it is shown it.
      const doc = (await documentOf(editor)) as object;

      for (const where of ['editor', 'view'] as const) {
        const body = where === 'editor' ? editor : await inTheView(page, doc);
        await expect(body.locator('.doc-math')).toHaveCount(2);
        await expect(body.locator('.doc-math:not([data-state])')).toHaveCount(0);
        expect(await findings(body, '.doc-math')).toEqual([]);
        // Nothing the formula says became an attribute of the page: no identifier,
        // no class from outside the library, no data attribute, no position.
        expect(await reachOf(body)).toEqual([]);
        // The page does not grow sideways, whatever the formula asks for.
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
        ).toBe(true);
        await nothingAskedOrRan(page, requests);
      }
    });
  }

  test('the costliest formulas that fit the limit are drawn and laid out in well under a second, or refused', async ({
    page,
  }) => {
    // Straight to the module the editor and the view draw through. Timed are
    // the library building the formula and, when it is let into the page, the
    // browser laying it out: nesting costs the browser far more than the library.
    await page.goto('/');
    await expect(page.getByTestId('board-page')).toBeVisible();
    const timings = await page.evaluate(async () => {
      const { loadMath, drawMath } = await (
        window as unknown as { fvociDev: MathDev }
      ).fvociDev.math();
      const library = await loadMath();
      const nest = (unit: string, close: string, depth?: number): string => {
        const levels = depth ?? Math.floor(3999 / (unit.length + close.length));
        return `${unit.repeat(levels)}x${close.repeat(levels)}`;
      };
      const cases: Record<string, string> = {
        // As deep as the source limit allows.
        'roots in each other, 570': nest('\\sqrt{', '}'),
        'fractions in each other, 399': nest('\\frac{1}{', '}'),
        'boxes in each other, 499': nest('\\boxed{', '}'),
        'accents in each other, 666': nest('\\hat{', '}'),
        'scripts in each other': nest('{x^', '}'),
        'delimiters in each other, 285': nest('\\left(', '\\right)'),
        'choices in each other': nest('\\mathchoice{', '}{}{}{}'),
        // Just inside what is let into the page.
        'boxes in each other, 24': nest('\\boxed{', '}', 24),
        'roots in each other, 30': nest('\\sqrt{', '}', 30),
        'fractions in each other, 26': nest('\\frac{1}{', '}', 26),
        // Wide rather than deep.
        'many symbols': 'x^2+'.repeat(999),
        'a large matrix': `\\begin{matrix}${`${'a&'.repeat(30)}a\\\\`.repeat(62)}\\end{matrix}`,
        'macros that multiply':
          '\\def\\a{xxxxxxxxxx}\\def\\b{\\a\\a\\a\\a\\a\\a\\a\\a\\a\\a}\\def\\c{\\b\\b\\b\\b\\b\\b\\b\\b\\b\\b}\\def\\d{\\c\\c\\c\\c\\c\\c\\c\\c\\c\\c}\\d\\d\\d\\d\\d\\d\\d\\d\\d',
      };
      const results: Record<
        string,
        { length: number; outcome: string; build: number; layout: number }
      > = {};
      for (const [name, latex] of Object.entries(cases)) {
        const element = document.createElement('div');
        element.className = 'doc-body';
        const started = performance.now();
        const result = drawMath(library, element, latex, true);
        const built = performance.now();
        let layout = 0;
        if (result.ok) {
          document.body.append(element);
          element.getBoundingClientRect();
          layout = performance.now() - built;
          element.remove();
        }
        results[name] = {
          length: latex.length,
          outcome: result.ok ? 'drawn' : result.reason,
          build: Math.round(built - started),
          layout: Math.round(layout),
        };
      }
      return results;
    });
    const outcomes: Record<string, string> = {};
    for (const [name, { length, outcome, build, layout }] of Object.entries(timings)) {
      console.warn(
        `formula, ${name}: ${length} characters, ${outcome}; built in ${build} ms, laid out in ${layout} ms`,
      );
      expect(length).toBeLessThanOrEqual(4000);
      expect([name, build + layout < 1000]).toEqual([name, true]);
      outcomes[name] = outcome;
    }
    // Nested as deep as the source allows: never let into the page. (Whether it
    // is this code that refuses or the library that gives up first depends on
    // how deep the library's own recursion can go.)
    for (const name of [
      'roots in each other, 570',
      'fractions in each other, 399',
      'boxes in each other, 499',
      'accents in each other, 666',
      'scripts in each other',
      'delimiters in each other, 285',
    ]) {
      expect([name, ['too-large', 'invalid']]).toEqual([
        name,
        expect.arrayContaining([outcomes[name]]),
      ]);
    }
    expect(outcomes['roots in each other, 570']).toBe('too-large');
    expect(outcomes['macros that multiply']).toBe('invalid');
    // What a person might write, and what is wide rather than deep, is drawn.
    for (const name of [
      'choices in each other',
      'boxes in each other, 24',
      'roots in each other, 30',
      'fractions in each other, 26',
      'many symbols',
      'a large matrix',
    ]) {
      expect([name, outcomes[name]]).toEqual([name, 'drawn']);
    }
  });

  test('a formula nested deeper than a browser can lay out is shown as its source, and the page lives', async ({
    page,
  }) => {
    // As deep as the limit on the source allows: 570 roots are 3,991 characters.
    // Let into the page, this one crashes it (it did, before the limit on depth).
    const depth = 570;
    const latex = `${'\\sqrt{'.repeat(depth)}x${'}'.repeat(depth)}`;
    const editor = await inTheEditor(page, EMPTY);
    await paste(editor, `<div data-type="block-math" data-latex="${latex}">x</div>`);
    await expect
      .poll(async () => JSON.stringify(await documentOf(editor)))
      .toContain(JSON.stringify({ type: 'blockMath', attrs: { latex } }));
    const doc = (await documentOf(editor)) as object;
    for (const where of ['editor', 'view'] as const) {
      const body = where === 'editor' ? editor : await inTheView(page, doc);
      const formula = body.locator('div.doc-math');
      await expect(formula).toHaveAttribute('data-state', 'error', { timeout: 15_000 });
      await expect(formula.getByRole('note')).toHaveText('This formula is too large to show.');
      await expect(formula.locator('.katex')).toHaveCount(0);
      // The body did not become a tower.
      expect(await body.evaluate((element) => element.getBoundingClientRect().height)).toBeLessThan(
        3000,
      );
    }
  });
});
