import { describe, expect, it } from 'vitest';

import { readFromDocxWorker } from '../../src/viewer-frame/docx-protocol.ts';
import { NodeCount, parseXml, TooMuch } from '../../src/viewer-frame/worker-xml.ts';
import {
  basedOnTableStylesDocx,
  footerAsHeaderDocx,
  languageDocx,
  linkedStylesDocx,
  manyListLevelsDocx,
  nestedSuperscriptDocx,
  notesInNotesDocx,
  numberingXml,
  picturesDocx,
  proofMarksDocx,
  styleChainDocx,
  wideTableHeaderDocx,
} from '../support/hostile-docx.ts';
import { DOCX_TEXT, koreanDocx, repeatedHeaderDocx } from '../support/viewer-files.ts';
import { drawAsWorker, stepsAsMarkup } from '../support/worker-globals.ts';

const refused = { kind: 'failed', reason: 'too-large' };

describe('the Word worker draws a document on its own DOM', () => {
  it('the Korean sample: its pages, header, table, picture and styles', async () => {
    const answer = await drawAsWorker(koreanDocx());
    if (answer.kind !== 'drawn') throw new Error(JSON.stringify(answer));
    const body = stepsAsMarkup(answer.body);
    expect(body.match(/<section class="docx"/g)).toHaveLength(2);
    for (const text of [DOCX_TEXT.title, DOCX_TEXT.header, DOCX_TEXT.second, ...DOCX_TEXT.cells]) {
      expect(body).toContain(text);
    }
    expect(body).toMatch(/<img src="TOKEN-0"/);
    expect(answer.images).toHaveLength(1);
    expect(stepsAsMarkup(answer.styles)).toMatch(/^<style>/);
  });

  it('a picture shown in many places has one key', async () => {
    const answer = await drawAsWorker(picturesDocx(50));
    if (answer.kind !== 'drawn') throw new Error(JSON.stringify(answer));
    expect(stepsAsMarkup(answer.body).match(/src="TOKEN-0"/g)).toHaveLength(50);
    expect(answer.images).toHaveLength(1);
  });

  it('every shape is marked for the frame to measure, a shape in a shape too', async () => {
    const answer = await drawAsWorker(nestedSuperscriptDocx(2));
    if (answer.kind !== 'drawn') throw new Error(JSON.stringify(answer));
    const markup = stepsAsMarkup(answer.body);
    expect(markup.match(/<svg /g)).toHaveLength(
      markup.match(/<svg data-fvoci-measure=""/g)?.length ?? 0,
    );
    expect(markup.match(/<svg /g)?.length).toBeGreaterThanOrEqual(2);
  });

  it("a run's language is kept", async () => {
    const answer = await drawAsWorker(languageDocx('en-US'));
    if (answer.kind !== 'drawn') throw new Error(JSON.stringify(answer));
    expect(stepsAsMarkup(answer.body)).toContain('lang="en-US"');
  });
});

describe('documents that make docx-preview do a great deal end within the limits', () => {
  // Each a few kilobytes; what the reviews of the viewer found. Without the worker's
  // ceilings each drew for minutes, or without end, on the frame's main thread.
  const cases: [string, () => Buffer, 'drawn' | 'too-large'][] = [
    ['runs in superscript nested 30 deep', () => nestedSuperscriptDocx(30), 'too-large'],
    [
      'a header table of 5,000 columns on 100 pages',
      () => wideTableHeaderDocx(5000, 100),
      'too-large',
    ],
    ['a footnote of endnotes, referred to often', () => notesInNotesDocx(30, 30, 300), 'too-large'],
    ['300 numberings of a 300-level list', () => manyListLevelsDocx(300, 300), 'too-large'],
    ['a footer that is the header too, on 50 pages', () => footerAsHeaderDocx(200, 50), 'drawn'],
    ['a chain of 20,000 styles', () => styleChainDocx(20_000), 'too-large'],
    ['a header of 2,000 paragraphs on 600 pages', () => repeatedHeaderDocx(600, 2000), 'too-large'],
  ];

  it.each(cases)(
    '%s',
    async (_, make, expected) => {
      const file = make();
      expect(file.length).toBeLessThan(200_000);
      const answer = await drawAsWorker(file);
      if (expected === 'too-large') {
        expect(answer).toEqual(refused);
        return;
      }
      if (answer.kind !== 'drawn') throw new Error(JSON.stringify(answer));
      expect(stepsAsMarkup(answer.body)).toContain('바닥글');
    },
    20_000,
  );
});

describe('what the worker refuses before docx-preview makes it', () => {
  it('a numbering part that would expand past its ceiling, whatever its root is called', () => {
    for (const root of ['w:numbering', 'w:numberingX', 'other']) {
      const text = numberingXml(root, 200, 200);
      expect(() => parseXml(text, new NodeCount()), root).toThrow(TooMuch);
    }
    expect(() => parseXml(numberingXml('w:numbering', 100, 100), new NodeCount())).not.toThrow();
  });

  it('style sheets of gigabytes: styles linked to a long one, styles based on a table style', async () => {
    // Small enough that only the reckoning of style sheets refuses it: drawn, it fits the steps.
    expect(await drawAsWorker(linkedStylesDocx(400, 400))).toEqual(refused);
    expect(await drawAsWorker(basedOnTableStylesDocx(20_000))).toEqual(refused);
    // A few styles of the same kinds are drawn.
    expect((await drawAsWorker(linkedStylesDocx(10, 10))).kind).toBe('drawn');
  });

  it("a numbering's style sheet that would be gigabytes: long ids, many counters", () => {
    const counters = numberingXml('w:numbering', 1, 200)
      .replace('<w:numFmt w:val="decimal"/>', `<w:lvlText w:val="${'%1'.repeat(8000)}"/>`)
      .replace(/w:numId="(\d+)"/g, (_, id: string) => `w:numId="${id.padStart(4000, 'n')}"`);
    expect(() => parseXml(counters, new NodeCount())).toThrow(TooMuch);
  });

  it('more XML nodes than its ceiling, of a kind that draws nothing', async () => {
    const file = proofMarksDocx(2_100_000);
    expect(file.length).toBeLessThan(300_000);
    expect(await drawAsWorker(file)).toEqual(refused);
    expect((await drawAsWorker(proofMarksDocx(1000))).kind).toBe('drawn');
  }, 30_000);
});

describe("the worker's answer is taken only in its shapes", () => {
  it('a drawing, or a failure for a reason it has', () => {
    const blob = new Blob(['x']);
    const drawn = { kind: 'drawn', styles: [], body: [3, 'text'], images: [['k-0', blob]] };
    expect(readFromDocxWorker(drawn)).toEqual(drawn);
    expect(readFromDocxWorker(refused)).toEqual(refused);
    for (const wrong of [
      undefined,
      'drawn',
      { kind: 'failed', reason: 'tired' },
      { kind: 'drawn', styles: '<style>', body: [], images: [] },
      { kind: 'drawn', styles: [], body: [], images: [['k-0', 'not a blob']] },
    ]) {
      expect(readFromDocxWorker(wrong), JSON.stringify(wrong)).toBeUndefined();
    }
  });
});
