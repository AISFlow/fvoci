/**
 * The numbers of dragging live in one place, and the parts take their
 * defaults from there.
 */
import { describe, expect, test } from 'vitest';

import { AUTO_SCROLL_DEFAULTS } from '$lib/drag/auto-scroll.ts';
import { POINTER_DRAG_DEFAULTS } from '$lib/drag/pointer-drag.ts';
import { DRAG_SETTINGS } from '$lib/drag/settings.ts';

describe('drag settings', () => {
  test('the values the product owner decided on', () => {
    expect(DRAG_SETTINGS).toEqual({
      mouseDistance: 4,
      touchDelay: 300,
      touchTolerance: 8,
      touchVibration: 15,
      scrollEdge: 56,
      scrollMaxSpeed: 900,
      longestFrame: 50,
      resizeTolerance: 120,
    });
  });

  test('a press becomes a drag by the settings', () => {
    expect(POINTER_DRAG_DEFAULTS).toEqual({
      mouseDistance: DRAG_SETTINGS.mouseDistance,
      touchDelay: DRAG_SETTINGS.touchDelay,
      touchTolerance: DRAG_SETTINGS.touchTolerance,
      touchVibration: DRAG_SETTINGS.touchVibration,
    });
  });

  test('the edges scroll by the settings', () => {
    expect(AUTO_SCROLL_DEFAULTS).toEqual({
      edge: DRAG_SETTINGS.scrollEdge,
      maxSpeed: DRAG_SETTINGS.scrollMaxSpeed,
    });
  });
});
