/** The display settings: read leniently, settled per device. */
import { describe, expect, it } from 'vitest';

import {
  DEFAULT_DISPLAY_SETTINGS,
  readDisplaySettings,
  resolveDisplay,
} from '../../src/lib/logic/display.ts';

describe('reading the display settings', () => {
  it('gives the defaults for nothing, and for what is not an object', () => {
    for (const stored of [undefined, null, 'dark', 7, []]) {
      expect(readDisplaySettings(stored)).toEqual(DEFAULT_DISPLAY_SETTINGS);
    }
    expect(DEFAULT_DISPLAY_SETTINGS).toEqual({
      theme: 'auto',
      cardOpen: 'center',
      density: 'auto',
      haptics: true,
    });
  });

  it('keeps the values it knows and replaces the ones it does not', () => {
    expect(
      readDisplaySettings({
        theme: 'dark',
        cardOpen: 'side',
        density: 'enormous',
        haptics: 'yes',
        somethingNew: 1,
      }),
    ).toEqual({ theme: 'dark', cardOpen: 'side', density: 'auto', haptics: true });
    expect(readDisplaySettings({ haptics: false }).haptics).toBe(false);
  });
});

describe('settling the display settings for a device', () => {
  const light = { prefersDark: false, coarsePointer: false };
  const darkTouch = { prefersDark: true, coarsePointer: true };

  it('follows the device where the setting is auto', () => {
    expect(resolveDisplay(DEFAULT_DISPLAY_SETTINGS, light)).toEqual({
      theme: 'light',
      cardOpen: 'center',
      density: 'compact',
      haptics: true,
    });
    expect(resolveDisplay(DEFAULT_DISPLAY_SETTINGS, darkTouch)).toMatchObject({
      theme: 'dark',
      density: 'comfortable',
    });
  });

  it('lets a fixed choice win over the device', () => {
    const fixed = { ...DEFAULT_DISPLAY_SETTINGS, theme: 'light', density: 'compact' } as const;
    expect(resolveDisplay(fixed, darkTouch)).toMatchObject({
      theme: 'light',
      density: 'compact',
    });
  });

  it('opens a card in the middle until the other ways are built', () => {
    for (const cardOpen of ['center', 'side', 'full'] as const) {
      expect(resolveDisplay({ ...DEFAULT_DISPLAY_SETTINGS, cardOpen }, light).cardOpen).toBe(
        'center',
      );
    }
  });
});
