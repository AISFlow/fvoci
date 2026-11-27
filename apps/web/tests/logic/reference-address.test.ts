/** The addresses of the app that a body takes as a reference when one is pasted. */
import { describe, expect, it } from 'vitest';

import { referenceAt, type Here } from '$lib/logic/reference-address.ts';

const WS = '01930000-0000-7000-8000-0000000000a1';
const BOARD = '01930000-0000-7000-8000-0000000000b1';
const VIEW = '01930000-0000-7000-8000-0000000000c1';
const CARD = '01930000-0000-7000-8000-0000000000d1';
const WIKI = '01930000-0000-7000-8000-0000000000e1';
const PAGE = '01930000-0000-7000-8000-0000000000f1';
const COURSE = '01930000-0000-7000-8000-000000000011';
const UNIT = '01930000-0000-7000-8000-000000000012';
const here: Here = { origin: 'https://fvoci.example', basePath: '', workspaceId: WS };

describe('the address of a card, a page, or a unit', () => {
  it('names the card, page, or unit of its address', () => {
    expect(referenceAt(`https://fvoci.example/w/${WS}/boards/${BOARD}/${VIEW}/${CARD}`, here)).toBe(
      CARD,
    );
    expect(referenceAt(`https://fvoci.example/w/${WS}/wikis/${WIKI}/${PAGE}`, here)).toBe(PAGE);
    expect(referenceAt(`https://fvoci.example/w/${WS}/courses/${COURSE}/${UNIT}`, here)).toBe(UNIT);
    // A query and a fragment are no part of what is named.
    expect(
      referenceAt(`https://fvoci.example/w/${WS}/boards/${BOARD}/${VIEW}/${CARD}?x=1#top`, here),
    ).toBe(CARD);
  });

  it('names nothing for a board, a view, a wiki, a course without a unit, or anything longer', () => {
    for (const path of [
      `/w/${WS}`,
      `/w/${WS}/boards/${BOARD}`,
      `/w/${WS}/boards/${BOARD}/${VIEW}`,
      `/w/${WS}/boards/${BOARD}/${VIEW}/${CARD}/more`,
      `/w/${WS}/wikis/${WIKI}`,
      `/w/${WS}/wikis/${WIKI}/${PAGE}/more`,
      `/w/${WS}/courses/${COURSE}`,
      `/w/${WS}/courses/${COURSE}/${UNIT}/more`,
      `/w/${WS}/other/${WIKI}/${PAGE}`,
      `/shared/${BOARD}/${VIEW}/${CARD}`,
      '/',
    ]) {
      expect([path, referenceAt(`https://fvoci.example${path}`, here)]).toEqual([path, undefined]);
    }
  });

  it('names nothing of another site, of another workspace, or with no workspace entered', () => {
    const path = `/w/${WS}/boards/${BOARD}/${VIEW}/${CARD}`;
    expect(referenceAt(`https://other.example${path}`, here)).toBeUndefined();
    expect(referenceAt(`http://fvoci.example${path}`, here)).toBeUndefined();
    expect(
      referenceAt(`https://fvoci.example/w/${BOARD}/boards/${BOARD}/${VIEW}/${CARD}`, here),
    ).toBeUndefined();
    expect(
      referenceAt(`https://fvoci.example${path}`, { ...here, workspaceId: undefined }),
    ).toBeUndefined();
    for (const text of ['', 'a card', path, 'mailto:a@b.example']) {
      expect([text, referenceAt(text, here)]).toEqual([text, undefined]);
    }
  });

  it('reads the address under the base path the app is served at, and no other', () => {
    const based: Here = { ...here, basePath: '/boards/app/' };
    const path = `/w/${WS}/boards/${BOARD}/${VIEW}/${CARD}`;
    expect(referenceAt(`https://fvoci.example/boards/app${path}`, based)).toBe(CARD);
    expect(referenceAt(`https://fvoci.example${path}`, based)).toBeUndefined();
    expect(referenceAt(`https://fvoci.example/boards/apps${path}`, based)).toBeUndefined();
  });
});
