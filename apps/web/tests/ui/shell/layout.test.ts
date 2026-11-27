/** Where the list of boards sits, from the shape the stage asks for and whether the reader folded it. */
import { describe, expect, it } from 'vitest';

import { shellLayout } from '$lib/features/shell/layout.ts';
import { sidebarShape } from '$lib/layout/presentation.ts';

describe('the layout of the shell', () => {
  it('a compact page has a drawer and the button of the top bar, and no strip', () => {
    expect(shellLayout(sidebarShape('compact'), false)).toEqual({
      list: 'over',
      drawer: true,
      strip: false,
      menuButton: true,
    });
  });

  it('a medium page has the strip, which opens the drawer', () => {
    expect(shellLayout(sidebarShape('medium'), false)).toEqual({
      list: 'over',
      drawer: true,
      strip: true,
      menuButton: false,
    });
  });

  it('an expanded page has the list beside the board, and nothing to open', () => {
    expect(shellLayout(sidebarShape('expanded'), false)).toEqual({
      list: 'beside',
      drawer: false,
      strip: false,
      menuButton: false,
    });
  });

  it('folded, an expanded page has the strip; unfolding is not a drawer', () => {
    expect(shellLayout(sidebarShape('expanded'), true)).toEqual({
      list: 'over',
      drawer: false,
      strip: true,
      menuButton: false,
    });
  });

  it('folding means nothing on the other stages, and is kept through them', () => {
    expect(shellLayout('drawer', true)).toEqual(shellLayout('drawer', false));
    expect(shellLayout('strip', true)).toEqual(shellLayout('strip', false));
  });

  it('the button of the top bar is there exactly where nothing else opens the list', () => {
    for (const shape of ['drawer', 'strip', 'fixed'] as const) {
      for (const collapsed of [false, true]) {
        const layout = shellLayout(shape, collapsed);
        expect(layout.menuButton).toBe(layout.list === 'over' && !layout.strip);
      }
    }
  });
});
