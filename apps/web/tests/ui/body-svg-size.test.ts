/** A wide tree can be rejected before copying, with the same element budget. */
import { expect, it } from 'vitest';

import { safeSvg } from '$lib/editor/safe-svg.ts';

const svg = (inside: string): string => `<svg id="pic">${inside}</svg>`;

it('counts the root in the budget, including direct children that will be dropped', () => {
  expect(safeSvg(svg('<x/>'.repeat(49_999)))).toBeDefined();
  expect(safeSvg(svg('<x/>'.repeat(50_000)))).toBeUndefined();
});

it('still counts visited nested elements, but never the children of dropped elements', () => {
  const children = '<x/>'.repeat(50_001);
  expect(safeSvg(svg(`<g>${children}</g>`))).toBeUndefined();
  expect(safeSvg(svg(`<x>${children}</x>`))).toBeDefined();
});
