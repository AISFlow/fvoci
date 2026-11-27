/**
 * The layout layer: the stage of the page comes from the width of a box and
 * the boundaries of the style sheet, one rule says how overlays open, and the
 * height the on-screen keyboard covers is worked out from what the browser
 * reports.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { coveredHeight } from '$lib/layout/keyboard.ts';
import { cardFrame, overlayPresentation, sidebarShape } from '$lib/layout/presentation.ts';
import { PageRoom, PaneRoom } from '$lib/layout/room.svelte.ts';

const root = document.documentElement;
const BOUNDARIES: Record<string, string> = {
  '--container-medium': '37.5rem',
  '--container-expanded': '52.5rem',
  '--container-columns': '34rem',
  '--container-header': '62rem',
  '--stage-min-height': '31.25rem',
};

beforeEach(() => {
  root.style.fontSize = '16px';
  for (const [name, value] of Object.entries(BOUNDARIES)) root.style.setProperty(name, value);
});

afterEach(() => {
  root.removeAttribute('style');
});

function pageOf(width: number, height = 800): PageRoom {
  const page = new PageRoom();
  page.width = width;
  page.height = height;
  return page;
}

describe('the stage of a page', () => {
  it('changes at 600 and 840 pixels of width', () => {
    expect(pageOf(360).stage).toBe('compact');
    expect(pageOf(599).stage).toBe('compact');
    expect(pageOf(600).stage).toBe('medium');
    expect(pageOf(839).stage).toBe('medium');
    expect(pageOf(840).stage).toBe('expanded');
    expect(pageOf(1280).stage).toBe('expanded');
  });

  it('moves with the text size of the reader: the boundaries are in rem', () => {
    root.style.fontSize = '20px';
    expect(pageOf(700).stage).toBe('compact');
    expect(pageOf(750).stage).toBe('medium');
  });

  it('is compact on a page too low for the wide layouts, however wide', () => {
    expect(pageOf(844, 390).stage).toBe('compact');
    expect(pageOf(890, 626).stage).toBe('expanded');
  });

  it('is the widest while nothing has been measured, and follows a change of size', () => {
    const page = new PageRoom();
    expect(page.measured).toBe(false);
    expect(page.stage).toBe('expanded');
    page.width = 626;
    page.height = 890;
    expect(page.stage).toBe('medium');
    page.width = 890;
    page.height = 626;
    expect(page.stage).toBe('expanded');
  });
});

describe('the pane of the board', () => {
  it('says when two columns and when the whole header fit', () => {
    const pane = new PaneRoom();
    pane.width = 543;
    expect([pane.columns, pane.header]).toEqual([false, false]);
    pane.width = 544;
    expect([pane.columns, pane.header]).toEqual([true, false]);
    pane.width = 992;
    expect([pane.columns, pane.header]).toEqual([true, true]);
  });
});

describe('how things are presented at each stage', () => {
  it('opens menus and pickers on a sheet on a compact page only', () => {
    expect(overlayPresentation('compact')).toBe('sheet');
    expect(overlayPresentation('medium')).toBe('popover');
    expect(overlayPresentation('expanded')).toBe('popover');
  });

  it('frames a card by the stage, and by the setting where the page is wide', () => {
    expect(cardFrame('compact', 'center')).toBe('full');
    expect(cardFrame('medium', 'center')).toBe('panel');
    expect(cardFrame('expanded', 'center')).toBe('dialog');
    // The other two ways are places kept for later.
    expect(cardFrame('expanded', 'side')).toBe('dialog');
    expect(cardFrame('expanded', 'full')).toBe('dialog');
  });

  it('shapes the sidebar by the stage', () => {
    expect(sidebarShape('compact')).toBe('drawer');
    expect(sidebarShape('medium')).toBe('strip');
    expect(sidebarShape('expanded')).toBe('fixed');
  });
});

describe('the height the on-screen keyboard covers', () => {
  const visual = (height: number, offsetTop = 0, scale = 1) => ({ height, offsetTop, scale });

  it('is what the visual viewport is short of the window', () => {
    expect(coveredHeight(740, visual(444))).toBe(296);
    expect(coveredHeight(740, visual(440, 4))).toBe(296);
  });

  it('is nothing where the browser shrank the page, or only a toolbar moved', () => {
    expect(coveredHeight(444, visual(444))).toBe(0);
    expect(coveredHeight(740, visual(700))).toBe(0);
  });

  it('is nothing while the page is zoomed in', () => {
    expect(coveredHeight(740, visual(370, 0, 2))).toBe(0);
  });
});
