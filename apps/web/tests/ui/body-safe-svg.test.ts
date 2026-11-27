/**
 * The filter between the diagram library and the page (safe-svg.ts), fed
 * with pictures written by hand to be hostile. Whatever the library might
 * one day let through, none of this may reach the page: scripts, handlers,
 * links, anything that fetches from an address, HTML inside the picture,
 * styles that reach outside it.
 *
 * (That the library itself, run on hostile diagram text, yields nothing of
 * the kind is tested in a browser: e2e/editor-attacks.spec.ts.)
 */
import { afterEach, describe, expect, it } from 'vitest';

import { isSafeCssValue, safeDeclarations, safeStyleSheet, safeSvg } from '$lib/editor/safe-svg.ts';

const EVIL = 'https://evil.example';
const svg = (inner: string, attributes = ''): string =>
  `<svg id="pic" xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10" ${attributes}>${inner}</svg>`;

/** The picture as the page would get it, written out, for looking through. */
function filtered(markup: string): { element: SVGSVGElement; text: string } {
  const element = safeSvg(markup);
  if (!element) throw new Error('refused outright');
  return { element, text: element.outerHTML };
}

const host = document.createElement('div');
document.body.append(host);
afterEach(() => {
  host.replaceChildren();
  delete (window as { pwned?: unknown }).pwned;
});

describe('a picture that is fine', () => {
  it('comes through with its shapes, text, markers, gradients and styles', () => {
    const { element, text } = filtered(
      svg(
        '<style>#pic .node rect{fill:#eee;stroke:rgb(0,0,0);stroke-width:1px;}#pic .edge{stroke:url(#pic-gradient);}' +
          '@keyframes dash{to{stroke-dashoffset:0;}}</style>' +
          '<defs><marker id="pic_arrow" viewBox="0 0 10 10" refX="5" refY="5" markerWidth="8" markerHeight="8" orient="auto">' +
          '<path d="M 0 0 L 10 5 L 0 10 z" class="arrowMarkerPath"></path></marker>' +
          '<linearGradient id="pic-gradient" gradientUnits="objectBoundingBox"><stop offset="0%" stop-color="#fff"></stop></linearGradient>' +
          '<filter id="pic-shadow"><feDropShadow dx="1" dy="1" stdDeviation="2" flood-opacity="0.2"></feDropShadow></filter></defs>' +
          '<g class="node default" transform="translate(10, 20)" data-look="classic">' +
          '<rect class="basic" x="-5" y="-5" width="10" height="10" rx="2" style="fill: #fafafa; stroke-width: 2px !important;"></rect>' +
          '<text y="1" text-anchor="middle" dominant-baseline="middle"><tspan x="0" dy="1em" font-weight="bold">시작 &lt;b&gt;</tspan></text></g>' +
          '<path d="M0,0L10,10" class="edge" marker-end="url(#pic_arrow)" filter="url(#pic-shadow)" stroke-dasharray="3 3"></path>' +
          '<circle cx="1" cy="1" r="1"></circle><ellipse cx="1" cy="1" rx="1" ry="2"></ellipse><line x1="0" y1="0" x2="1" y2="1"></line>' +
          '<polygon points="0,0 1,1 0,1"></polygon><polyline points="0,0 1,1"></polyline>' +
          '<use href="#pic_arrow"></use><title>제목</title><desc>설명</desc>',
        'role="graphics-document document" aria-roledescription="flowchart-v2" width="100%" style="max-width: 320px;" class="flowchart"',
      ),
    );
    expect(element.namespaceURI).toBe('http://www.w3.org/2000/svg');
    expect(element.getAttribute('viewBox')).toBe('0 0 10 10');
    expect(element.getAttribute('role')).toBe('graphics-document document');
    expect(element.getAttribute('aria-roledescription')).toBe('flowchart-v2');
    expect(element.style.getPropertyValue('max-width')).toBe('320px');
    const kinds = new Set(Array.from(element.querySelectorAll('*')).map((node) => node.localName));
    expect([...kinds].sort()).toEqual(
      [
        'circle',
        'defs',
        'desc',
        'ellipse',
        'feDropShadow',
        'filter',
        'g',
        'line',
        'linearGradient',
        'marker',
        'path',
        'polygon',
        'polyline',
        'rect',
        'stop',
        'style',
        'text',
        'title',
        'tspan',
        'use',
      ].sort(),
    );
    expect(element.querySelector('tspan')?.textContent).toBe('시작 <b>');
    expect(element.querySelector('path.edge')?.getAttribute('marker-end')).toBe('url(#pic_arrow)');
    expect(element.querySelector('path.edge')?.getAttribute('filter')).toBe('url(#pic-shadow)');
    expect(element.querySelector('use')?.getAttribute('href')).toBe('#pic_arrow');
    expect(element.querySelector('g')?.getAttribute('data-look')).toBe('classic');
    expect(element.querySelector('g')?.getAttribute('transform')).toBe('translate(10, 20)');
    const rect = element.querySelector<SVGElement>('rect');
    expect(rect?.style.getPropertyValue('stroke-width')).toBe('2px');
    expect(rect?.style.getPropertyPriority('stroke-width')).toBe('important');
    const sheet = element.querySelector('style')?.textContent ?? '';
    expect(sheet).toContain('#pic .node rect{fill:#eee;stroke:rgb(0,0,0);stroke-width:1px}');
    expect(sheet).toContain('#pic .edge{stroke:url(#pic-gradient)}');
    expect(sheet).toContain('@keyframes pic-dash{to{stroke-dashoffset:0}}');
    expect(text).not.toContain('xmlns:xlink');
  });

  it('is built in the document it is asked to be built in and enforces expectedId when given', () => {
    const other = document.implementation.createHTMLDocument('');
    expect(safeSvg(svg('<rect></rect>'), { document: other })?.ownerDocument).toBe(other);
    expect(safeSvg(svg('<rect></rect>'), { expectedId: 'pic' })).toBeDefined();
    expect(safeSvg(svg('<rect></rect>'), { expectedId: 'doc-diagram-9' })).toBeUndefined();
  });
});

describe('what is not a picture', () => {
  it.each([
    ['nothing', ''],
    ['text', 'just text'],
    ['HTML', '<div>hello</div>'],
    [
      'a frame, as the library gives in its sandbox mode',
      '<iframe src="data:text/html;base64,AAAA" sandbox=""></iframe>',
    ],
    ['a picture and something next to it', `${svg('')}<script>window.pwned = 1</script>`],
    ['two pictures', svg('') + svg('')],
    ['a picture without an identifier', '<svg xmlns="http://www.w3.org/2000/svg"></svg>'],
    [
      'a picture whose identifier could end a selector',
      '<svg id="a,body" xmlns="http://www.w3.org/2000/svg"></svg>',
    ],
    ['an element that is only called svg', '<div><svg id="pic"></svg></div>'],
  ])('refuses %s', (_name, markup) => {
    expect(safeSvg(markup)).toBeUndefined();
  });

  it('refuses a picture that is too deep or too large', () => {
    expect(safeSvg(svg('<g>'.repeat(70) + '</g>'.repeat(70)))).toBeUndefined();
    expect(safeSvg(svg('<g>'.repeat(40) + '</g>'.repeat(40)))).toBeDefined();
    expect(safeSvg(svg('<rect></rect>'.repeat(50_001)))).toBeUndefined();
  });
});

/** Hostile pictures, by what they try. Each must come out without a trace of it. */
const HOSTILE: [string, string][] = [
  ['a script', svg('<script>window.pwned = 1</script><rect></rect>')],
  [
    'a script by address',
    svg(`<script href="${EVIL}/x.js"></script><script xlink:href="${EVIL}/y.js"></script>`),
  ],
  ['a handler on the picture', svg('<rect></rect>', 'onload="window.pwned = 1"')],
  [
    'handlers on shapes',
    svg(
      '<rect onclick="window.pwned = 1" onmouseover="window.pwned = 1" onfocus="window.pwned = 1" ONLOAD="window.pwned = 1"></rect>' +
        '<g onpointerdown="window.pwned = 1"><text onbegin="window.pwned = 1">t</text></g>',
    ),
  ],
  [
    'animation that sets a handler',
    svg(
      '<rect><set attributeName="onmouseover" to="window.pwned = 1"></set><animate attributeName="href" values="javascript:window.pwned=1"></animate></rect>',
    ),
  ],
  ['a link', svg(`<a href="${EVIL}/link" target="_blank"><text>click</text></a>`)],
  [
    'a link that is a script',
    svg(
      '<a href="javascript:window.pwned = 1"><text>click</text></a><a xlink:href="javascript:window.pwned = 1"><text>click</text></a>',
    ),
  ],
  [
    'a picture by address',
    svg(
      `<image href="${EVIL}/pixel.png" width="1" height="1"></image><image xlink:href="${EVIL}/pixel2.png"></image>`,
    ),
  ],
  [
    'a filter that fetches',
    svg(
      `<filter id="f"><feImage href="${EVIL}/fe.png"></feImage></filter><rect filter="url(#f)"></rect>`,
    ),
  ],
  [
    'a reference outside the picture',
    svg(
      `<use href="${EVIL}/sprite.svg#icon"></use><use xlink:href="//evil.example/sprite.svg#icon"></use><use href="data:image/svg+xml,&lt;svg id='x'/&gt;#x"></use>`,
    ),
  ],
  [
    'a gradient and a pattern taken from elsewhere',
    svg(
      `<linearGradient id="g" href="${EVIL}/g.svg#g"></linearGradient><pattern id="p" xlink:href="${EVIL}/p.svg#p"></pattern><textPath href="${EVIL}/t.svg#t">t</textPath>`,
    ),
  ],
  [
    'an address in a paint attribute',
    svg(
      `<rect fill="url(${EVIL}/paint.svg#p)" stroke="url('${EVIL}/s.svg#s')" mask="url(${EVIL}/m.svg#m)" clip-path="url(${EVIL}/c.svg#c)" marker-end="url(${EVIL}/e.svg#e)" filter="url(${EVIL}/f.svg#f)"></rect>`,
    ),
  ],
  [
    'an address in a style attribute',
    svg(
      `<rect style="fill: red; background-image: url(${EVIL}/bg.png); mask-image: url('${EVIL}/mask.png'); cursor: url(${EVIL}/c.cur), auto"></rect>`,
      `style="background: url(${EVIL}/root.png)"`,
    ),
  ],
  [
    'an address without the word url',
    svg(
      `<rect style="mask-image: image-set('${EVIL}/set.png' 1x); background: -webkit-image-set('${EVIL}/set2.png' 1x); content: src('${EVIL}/src.png')"></rect>`,
    ),
  ],
  [
    'an address spelled with escapes',
    svg(
      `<rect style="mask-image: \\75rl(${EVIL}/escaped.png); background: u\\72 l(${EVIL}/escaped2.png)"></rect>`,
    ),
  ],
  [
    'an address behind a comment',
    svg(
      `<rect style="background: u/**/rl(${EVIL}/comment.png); mask: url/**/(${EVIL}/comment2.png)"></rect>`,
    ),
  ],
  [
    'an import in the style sheet',
    svg(
      `<style>@import url(${EVIL}/import.css); @import "${EVIL}/import2.css"; #pic rect{fill:red}</style>`,
    ),
  ],
  [
    'a font fetched by the style sheet',
    svg(
      `<style>@font-face{font-family:x;src:url(${EVIL}/font.woff2)} #pic text{font-family:x}</style>`,
    ),
  ],
  [
    'an address in a rule of the style sheet',
    svg(
      `<style>#pic{background:url(${EVIL}/sheet.png)} #pic rect{mask-image:url("${EVIL}/sheet2.png");fill:red}</style><rect></rect>`,
    ),
  ],
  [
    'a style sheet with escapes',
    svg(`<style>#pic{background:\\75rl(${EVIL}/sheet-escaped.png)} #pic rect{fill:red}</style>`),
  ],
  [
    'an at-rule that could carry anything',
    svg(
      `<style>@namespace x url(${EVIL}/ns); @media all{body{display:none}} @supports (display:grid){#pic{background:url(${EVIL}/supports.png)}}</style>`,
    ),
  ],
  [
    'HTML inside the picture',
    svg(
      `<foreignObject width="10" height="10"><div xmlns="http://www.w3.org/1999/xhtml" onclick="window.pwned = 1">label <img src="${EVIL}/fo.png"><iframe src="${EVIL}/frame"></iframe><form action="${EVIL}/form"><input name="q"></form></div></foreignObject>`,
    ),
  ],
  [
    'a nested picture with all of it again',
    svg(
      `<svg onload="window.pwned = 1"><script>window.pwned = 1</script><image href="${EVIL}/nested.png"></image><a href="${EVIL}/nested"><rect></rect></a></svg>`,
    ),
  ],
  [
    'names that would shadow the page’s own',
    svg('<rect id="app" name="body"></rect><g id="a b" name="cookie"></g><rect id="x)y"></rect>'),
  ],
  [
    'attributes the editor reads on paste',
    svg(
      '<text data-mention="" data-user-id="u1" data-file-id="f1" data-type="inline-math" data-latex="x">t</text>',
    ),
  ],
];

/**
 * Hostile pictures that are refused outright. HTML inside an SVG picture ends
 * the picture as far as a parser of HTML is concerned: what follows lands
 * next to it, and markup that is more than one picture is not taken at all.
 */
const REFUSED: [string, string][] = [
  [
    'a style sheet that breaks out into markup',
    svg(
      `<style>#pic{fill:red}</style><img src="${EVIL}/breakout.png" onerror="window.pwned = 1"><style>`,
    ),
  ],
  [
    'HTML that left the picture',
    svg(
      `<text>t</text><p>para <img src="${EVIL}/p.png" onerror="window.pwned = 1"></p><iframe src="${EVIL}/frame2"></iframe>`,
    ),
  ],
  [
    'an embedded document',
    svg(
      `<iframe src="${EVIL}/frame3"></iframe><object data="${EVIL}/object"></object><embed src="${EVIL}/embed"><audio src="${EVIL}/audio"></audio><video src="${EVIL}/video" poster="${EVIL}/poster"></video>`,
    ),
  ],
  [
    'a base address and a meta refresh',
    svg(
      `<base href="${EVIL}/"><meta http-equiv="refresh" content="0;url=${EVIL}/refresh"><link rel="stylesheet" href="${EVIL}/link.css">`,
    ),
  ],
];

describe('a hostile picture that is more than a picture', () => {
  it.each(REFUSED)('%s: is refused', (_name, markup) => {
    expect(safeSvg(markup)).toBeUndefined();
    expect((window as { pwned?: unknown }).pwned).toBeUndefined();
  });
});

describe('a hostile picture', () => {
  it.each(HOSTILE)('%s: comes out without it', (_name, markup) => {
    const { element, text } = filtered(markup);
    host.append(element);

    expect(text.toLowerCase()).not.toContain('evil.example');
    expect(text.toLowerCase()).not.toContain('javascript:');
    expect(text).not.toContain('pwned');
    expect(text).not.toContain('@import');
    expect(text).not.toContain('@font-face');
    expect(text).not.toContain('\\');

    for (const node of [element, ...Array.from(element.querySelectorAll('*'))]) {
      // Every element is SVG, and of a kind that neither runs nor fetches.
      expect([node.localName, node.namespaceURI]).toEqual([
        node.localName,
        'http://www.w3.org/2000/svg',
      ]);
      expect([
        'script',
        'a',
        'image',
        'feImage',
        'foreignObject',
        'iframe',
        'object',
        'embed',
        'audio',
        'video',
        'img',
        'set',
        'animate',
        'base',
        'meta',
        'link',
        'form',
        'input',
      ]).not.toContain(node.localName);
      for (const { name, value } of Array.from(node.attributes)) {
        expect([name, name.toLowerCase().startsWith('on')]).toEqual([name, false]);
        expect([
          'name',
          'src',
          'srcdoc',
          'action',
          'data',
          'poster',
          'target',
          'xlink:href',
        ]).not.toContain(name);
        if (name === 'href') expect(value).toMatch(/^#[\w.:-]+$/);
        if (name.startsWith('data-')) expect(name).toBe('data-look');
        // An address in a value points into the picture or is not there.
        expect(value.replace(/url\(\s*["']?#[\w.:-]+["']?\s*\)/g, '')).not.toMatch(/url\s*\(/i);
      }
    }
    // A nested picture is no element of its own kind; nothing was run.
    expect((window as { pwned?: unknown }).pwned).toBeUndefined();
  });

  it('keeps what a link held, without the link', () => {
    const { element } = filtered(svg(`<a href="${EVIL}/x"><g><text>label</text></g></a>`));
    expect(element.querySelector('a')).toBeNull();
    expect(element.querySelector('g > text')?.textContent).toBe('label');
  });

  it('keeps the fallback text when the HTML label next to it is dropped', () => {
    const { element } = filtered(
      svg(
        '<switch><foreignObject width="5" height="5"><div xmlns="http://www.w3.org/1999/xhtml">html</div></foreignObject><text>plain</text></switch>',
      ),
    );
    expect(element.querySelector('switch')?.children).toHaveLength(1);
    expect(element.querySelector('switch > text')?.textContent).toBe('plain');
  });

  it('keeps inner identifiers prefixed or suffixed with the root id, and drops dangling #href and url(#id) references', () => {
    const { element } = filtered(
      svg(
        '<style>#pic .bad{fill:url(#outside-grad);stroke:blue}#pic .only-bad{filter:url(#outside-filter)}</style>' +
          '<defs><g id="pic-ok-1"></g><g id="pic_가나"></g>' +
          '<marker id="link-arrow-start-pic"></marker><marker id="link-arrow-end-pic"></marker>' +
          '<marker id="arrow-pic"></marker><marker id="cynefin-arrow-pic"></marker>' +
          '<marker id="ishikawa-arrow-pic"></marker><marker id="em-arrowhead-pic"></marker></defs>' +
          '<rect id="app" fill="url(#outside-paint)" style="filter: url(#outside-filter); animation: pulse 1s linear; stroke: green" aria-labelledby="pic-ok-1 outside-label"></rect>' +
          '<rect id="a b"></rect><rect id="x)y"></rect>' +
          '<path class="wardley" marker-start="url(#link-arrow-start-pic)" marker-end="url(#link-arrow-end-pic)"></path>' +
          '<path class="cynefin" marker-end="url(#cynefin-arrow-pic)"></path>' +
          '<path class="ishikawa" marker-end="url(#ishikawa-arrow-pic)"></path>' +
          '<path class="event-modeling" marker-end="url(#em-arrowhead-pic)"></path>' +
          '<use href="#pic-ok-1"></use><use href="#outside-page-id"></use>' +
          '<linearGradient id="pic-g" href="#outside-grad"></linearGradient>',
      ),
    );
    expect(
      Array.from(element.querySelectorAll('defs > *')).map((node) => node.getAttribute('id')),
    ).toEqual([
      'pic-ok-1',
      'pic_가나',
      'link-arrow-start-pic',
      'link-arrow-end-pic',
      'arrow-pic',
      'cynefin-arrow-pic',
      'ishikawa-arrow-pic',
      'em-arrowhead-pic',
    ]);
    expect(
      Array.from(element.querySelectorAll('rect')).map((rect) => rect.getAttribute('id')),
    ).toEqual([null, null, null]);
    const firstRect = element.querySelector<SVGElement>('rect');
    expect(firstRect?.hasAttribute('fill')).toBe(false);
    expect(firstRect?.style.getPropertyValue('filter')).toBe('');
    expect(firstRect?.style.getPropertyValue('stroke')).toBe('green');
    expect(firstRect?.style.getPropertyValue('animation')).toContain('pic-pulse');
    expect(firstRect?.getAttribute('aria-labelledby')).toBe('pic-ok-1');
    expect(element.querySelector('path.wardley')?.getAttribute('marker-end')).toBe(
      'url(#link-arrow-end-pic)',
    );
    expect(element.querySelector('path.cynefin')?.getAttribute('marker-end')).toBe(
      'url(#cynefin-arrow-pic)',
    );
    expect(element.querySelector('path.ishikawa')?.getAttribute('marker-end')).toBe(
      'url(#ishikawa-arrow-pic)',
    );
    expect(element.querySelector('path.event-modeling')?.getAttribute('marker-end')).toBe(
      'url(#em-arrowhead-pic)',
    );
    expect(element.querySelector('style')?.textContent).toBe('#pic .bad{stroke:blue}');
    expect(Array.from(element.querySelectorAll('use')).map((u) => u.getAttribute('href'))).toEqual([
      '#pic-ok-1',
    ]);
    expect(element.querySelector('linearGradient')?.hasAttribute('href')).toBe(false);
  });
});

describe('style rules', () => {
  it('keeps rules that stay inside the picture and drops those that do not', () => {
    const sheet = safeStyleSheet(
      [
        '#pic{font-family:"trebuchet ms",verdana;font-size:16px;fill:#333;}',
        '#pic .node rect,#pic .node circle{fill:#eee;}',
        '#pic>g{opacity:1}',
        '#pic:hover{opacity:0.9}',
        '#pic [data-look="neo"].node rect{stroke:red}',
        'body{display:none}',
        '#pic ~ *{display:none}',
        '#pic + *{display:none}',
        '#pic .a + .b{fill:red}',
        '#pic a } body{display:none}',
        '#pic .a, body .b{fill:red}',
        '#picture .a{fill:red}',
        '.node{fill:red}',
        '*{visibility:hidden}',
        ':root{--x:1}',
        '#pic :root{--mermaid-font-family:"trebuchet ms"}',
        '/* a comment */ #pic .c{stroke:blue}',
      ].join(''),
      '#pic',
    );
    expect(sheet.split('\n')).toEqual([
      '#pic{font-family:"trebuchet ms",verdana;font-size:16px;fill:#333}',
      '#pic .node rect,#pic .node circle{fill:#eee}',
      '#pic>g{opacity:1}',
      '#pic:hover{opacity:0.9}',
      '#pic [data-look="neo"].node rect{stroke:red}',
      '#pic :root{--mermaid-font-family:"trebuchet ms"}',
      '#pic .c{stroke:blue}',
    ]);
  });

  it('drops the declarations that could fetch and keeps the rest of the rule', () => {
    expect(
      safeStyleSheet(
        `#pic .a{fill:red;background:url(${EVIL}/a.png);stroke:url(#pic-g);filter:drop-shadow(1px 2px 3px rgba(0,0,0,0.3));mask:image-set("${EVIL}/b.png" 1x);transform:rotate(3deg) scale(1.1)}`,
        '#pic',
      ),
    ).toBe(
      '#pic .a{fill:red;stroke:url(#pic-g);filter:drop-shadow(1px 2px 3px rgba(0,0,0,0.3));transform:rotate(3deg) scale(1.1)}',
    );
  });

  it('scopes keyframes under the picture id and drops every other at-rule', () => {
    const sheet = safeStyleSheet(
      `@charset "utf-8";@import "${EVIL}/x.css";@keyframes edge-animation-frame{from{stroke-dashoffset:0;}}` +
        `@-webkit-keyframes dash{0%,50%{opacity:0}to{opacity:1;background:url(${EVIL}/k.png)}}` +
        `@keyframes spin{to{transform:rotate(360deg)}}` +
        `@font-face{font-family:x;src:url(${EVIL}/f.woff2)}@media print{#pic{display:none}}@layer a{#pic{fill:red}}@page{margin:0}` +
        '#pic .a{animation:dash 1s linear infinite;animation-name:spin}',
      '#pic',
    );
    expect(sheet.split('\n')).toEqual([
      '@keyframes pic-edge-animation-frame{from{stroke-dashoffset:0}}',
      '@keyframes pic-dash{0%,50%{opacity:0}to{opacity:1}}',
      '@keyframes pic-spin{to{transform:rotate(360deg)}}',
      '#pic .a{animation:pic-dash 1s linear infinite;animation-name:pic-spin}',
    ]);
  });

  it('drops a sheet whole when it holds an escape or markup', () => {
    expect(safeStyleSheet('#pic{fill:red}#pic .a{background:\\75rl(x)}', '#pic')).toBe('');
    expect(safeStyleSheet('#pic{fill:red}</style><script>', '#pic')).toBe('');
  });

  it('survives a sheet that does not end', () => {
    expect(safeStyleSheet('#pic{fill:red}#pic .a{fill:blue', '#pic')).toBe('#pic{fill:red}');
    // A brace inside a string does not end the rule; the value that holds it is dropped.
    expect(safeStyleSheet('#pic{fill:"}";stroke:red}#pic .a{fill:blue}', '#pic')).toBe(
      '#pic{stroke:red}\n#pic .a{fill:blue}',
    );
    expect(safeStyleSheet('', '#pic')).toBe('');
    expect(safeStyleSheet('}}}{{{', '#pic')).toBe('');
  });
});

describe('style values', () => {
  it.each([
    ['#fafafa'],
    ['rgb(1, 2, 3)'],
    ['hsl(240, 100%, 76.2745098039%)'],
    ['translate(10, 20) rotate(45)'],
    ['url(#pic-gradient)'],
    ['url("#pic_arrow")'],
    ["url( '#a' )"],
    ['drop-shadow(1px 2px 2px rgba(185,185,185,1))'],
    ['"trebuchet ms", verdana, arial, sans-serif'],
    ['calc(100% - var(--gap))'],
    ['M 0 0 L 10 5 L 0 10 z'],
    ['1px !important'],
    [''],
  ])('takes %j', (value) => {
    expect(isSafeCssValue(value)).toBe(true);
  });

  it.each([
    [`url(${EVIL}/x.png)`],
    ['url(//evil.example/x.png)'],
    ['url(x.png)'],
    ['url(data:image/png;base64,AAAA)'],
    ['url(#a) url(https://evil.example/b)'],
    ['URL(https://evil.example/x)'],
    ['url (https://evil.example/x)'],
    ['url(#a\\29 )'],
    ['\\75rl(#a)'],
    ['u/**/rl(x)'],
    [`image-set("${EVIL}/x.png" 1x)`],
    ['-webkit-image-set("x" 1x)'],
    ['image("x.png")'],
    ['src("x.png")'],
    ['element(#a)'],
    ['attr(data-x url)'],
    ['paint(worklet)'],
    ['cross-fade(url(#a), url(#b))'],
    ['expression(alert(1))'],
    ['javascript:alert(1)'],
    ['red; } body { display: none'],
    ['red</style><script>'],
    ['@import "x"'],
    ['(1)'],
    ['x'.repeat(200_001)],
  ])('refuses %j', (value) => {
    expect(isSafeCssValue(value)).toBe(false);
  });

  it('reads declarations past quotes and brackets and drops layout/overlay properties', () => {
    expect(
      safeDeclarations(
        'font-family: "a;b", serif; fill: rgb(1, 2, 3); : x; nope; --x: 1; 9bad: 1; position: fixed; inset: 0; z-index: 9999; pointer-events: all',
      ),
    ).toEqual([
      ['font-family', '"a;b", serif'],
      ['fill', 'rgb(1, 2, 3)'],
      ['--x', '1'],
    ]);
  });
});
