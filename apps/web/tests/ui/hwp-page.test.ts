import { describe, expect, it } from 'vitest';

import { pagePicture, pageText, withFallback } from '../../src/viewer-frame/hwp-page.ts';

const svg = (body: string): string =>
  `<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="800" height="1100" viewBox="0 0 800 1100">${body}</svg>`;

const picture = (body: string, page = 1): SVGSVGElement => {
  const made = pagePicture(svg(body), page, document);
  if (!made) throw new Error('no picture');
  return made;
};

describe("a page's picture is copied, not taken as it is", () => {
  it('keeps what a page is drawn with, hidden from a screen reader', () => {
    const made = picture(
      '<rect x="0" y="0" width="800" height="1100" fill="#ffffff"/><text x="10" y="20" font-size="13" fill="#000000">가</text>',
    );
    expect(made.getAttribute('viewBox')).toBe('0 0 800 1100');
    expect(made.getAttribute('aria-hidden')).toBe('true');
    expect(made.querySelector('rect')?.getAttribute('fill')).toBe('#ffffff');
    expect(made.querySelector('text')?.textContent).toBe('가');
  });

  it('leaves out scripts, foreign content, event handlers, styles and classes', () => {
    const made = picture(
      [
        '<script>parent.postMessage("x", "*")</script>',
        '<foreignObject width="10" height="10"><div xmlns="http://www.w3.org/1999/xhtml">form</div></foreignObject>',
        '<style>@import url("https://example.invalid/a.css");</style>',
        '<rect width="10" height="10" onclick="alert(1)" onload="alert(2)" style="fill:red" class="page"/>',
        '<set attributeName="fill" to="red"/>',
        '<meta http-equiv="refresh" content="0;url=https://example.invalid"/>',
      ].join(''),
    );
    expect(made.querySelector('script, foreignObject, style, set, meta')).toBeNull();
    expect(made.textContent).not.toContain('form');
    const rect = made.querySelector('rect');
    expect(rect?.getAttributeNames().sort()).toEqual(['height', 'width']);
  });

  it('gives a link up for what it holds', () => {
    const made = picture(
      '<a href="https://example.invalid/login"><text x="1" y="2">원본</text></a>',
    );
    expect(made.querySelector('a')).toBeNull();
    expect(made.querySelector('text')?.textContent).toBe('원본');
  });

  it('keeps a picture the page carries, not one from elsewhere', () => {
    const made = picture(
      [
        '<image width="2" height="2" href="data:image/png;base64,iVBORw0KGgo="/>',
        '<image width="2" height="2" xlink:href="data:image/jpeg;base64,/9j/"/>',
        '<image width="2" height="2" href="https://example.invalid/track.png"/>',
        '<image width="2" height="2" href="javascript:alert(1)"/>',
        '<image width="2" height="2" href="data:text/html,hi"/>',
      ].join(''),
    );
    const sources = [...made.querySelectorAll('image')].map((image) => image.getAttribute('href'));
    expect(sources).toEqual([
      'data:image/png;base64,iVBORw0KGgo=',
      'data:image/jpeg;base64,/9j/',
      null,
      null,
      null,
    ]);
  });

  it("names the page's ids by its number, and its references with them", () => {
    const made = picture(
      [
        '<defs><clipPath id="body-clip-3"><rect width="5" height="5"/></clipPath></defs>',
        '<g clip-path="url(#body-clip-3)"><rect width="9" height="9" fill="url( #grad )"/></g>',
        '<rect width="1" height="1" fill="url(https://example.invalid/x.svg#a)"/>',
      ].join(''),
      7,
    );
    expect(made.querySelector('clipPath')?.id).toBe('p7-body-clip-3');
    expect(made.querySelector('g')?.getAttribute('clip-path')).toBe('url(#p7-body-clip-3)');
    expect(made.querySelector('g rect')?.getAttribute('fill')).toBe('url(#p7-grad)');
    expect(made.querySelectorAll('rect')[2]?.hasAttribute('fill')).toBe(false);
  });

  it('is no picture when the text is no SVG', () => {
    expect(pagePicture('<html><body>no</body></html>', 1, document)).toBeUndefined();
    expect(
      pagePicture('<svg xmlns="http://www.w3.org/2000/svg"><rect', 1, document),
    ).toBeUndefined();
  });
});

describe("the page's fonts fall back to the app's Korean font", () => {
  it('ahead of the generic family a document ends with', () => {
    expect(withFallback("'함초롬바탕','Batang',serif")).toBe(
      "'함초롬바탕','Batang','Noto Sans KR',serif",
    );
    expect(withFallback("'맑은 고딕'")).toBe("'맑은 고딕','Noto Sans KR'");
    expect(withFallback("'Noto Sans KR',sans-serif")).toBe("'Noto Sans KR',sans-serif");
  });

  it('on every text of the picture', () => {
    const made = picture(`<text font-family="'함초롬돋움',sans-serif">가</text>`);
    expect(made.querySelector('text')?.getAttribute('font-family')).toBe(
      "'함초롬돋움','Noto Sans KR',sans-serif",
    );
  });
});

describe("the page's text, over its picture", () => {
  const run = (text: string, x: number, y: number) => ({
    text,
    x,
    y,
    width: 50,
    height: 13,
    size: 13,
  });

  it('is a run of text where the picture draws it, a line break between lines', () => {
    const layer = pageText(
      [run('분기 회의록', 113, 132), run('항목', 124, 180), run('담당', 310, 180)],
      document,
    );
    const spans = [...layer.querySelectorAll('span')];
    // Two cells of a table, a gap apart: a space after the first, which spans the gap.
    expect(spans.map((span) => span.textContent)).toEqual(['분기 회의록', '항목 ', '담당']);
    expect(spans[0]?.style.left).toBe('113px');
    expect(spans[0]?.style.top).toBe('132px');
    expect(spans[0]?.style.fontSize).toBe('13px');
    expect([...layer.children].map((child) => child.localName)).toEqual([
      'span',
      'br',
      'span',
      'wbr',
      'span',
    ]);
  });

  it('no space between runs that touch, or after one that ends with a space', () => {
    const layer = pageText(
      [run('11', 100, 50), run('월', 150, 50), run('20 ', 200, 50), run('일', 300, 50)],
      document,
    );
    const spans = [...layer.querySelectorAll('span')];
    expect(spans.map((span) => span.textContent)).toEqual(['11', '월', '20 ', '일']);
  });

  it('leaves out runs of blanks', () => {
    expect(pageText([run('  ', 0, 0), run('', 0, 1)], document).children).toHaveLength(0);
  });
});
