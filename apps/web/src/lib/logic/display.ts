/**
 * The reader's display settings: how bright the screen is, how a card
 * opens, how much room things take, and whether the device may buzz.
 *
 * They sit beside the regional settings (`regional.ts`) and travel the same
 * way: part of the reader's account, written as the server stores them, with
 * `auto` meaning "follow this device". `resolveDisplay` turns them into the
 * plain values the screens use. This module does not read the browser: the
 * caller passes what the device says.
 *
 * A setting exists only where two people can pick different answers and
 * both be right. Things the device already has a switch for (reduced motion,
 * reduced transparency) are followed, not asked again.
 */

export type ThemeSetting = 'auto' | 'light' | 'dark';
/**
 * How a card opens on a wide screen. Only `center` is built; `side` (a pane
 * beside the board) and `full` are places kept for later and are shown as
 * `center` until then.
 */
export type CardOpenSetting = 'center' | 'side' | 'full';
export type DensitySetting = 'auto' | 'compact' | 'comfortable';

export const THEME_SETTINGS: readonly ThemeSetting[] = ['auto', 'light', 'dark'];
export const CARD_OPEN_SETTINGS: readonly CardOpenSetting[] = ['center', 'side', 'full'];
/** The ways a card can open that are built. */
export const CARD_OPEN_BUILT: readonly CardOpenSetting[] = ['center'];
export const DENSITY_SETTINGS: readonly DensitySetting[] = ['auto', 'compact', 'comfortable'];

export interface DisplaySettings {
  /** `auto` follows the device's light or dark setting. */
  theme: ThemeSetting;
  cardOpen: CardOpenSetting;
  /** `auto` follows the pointer: roomy for a finger, compact for a mouse. */
  density: DensitySetting;
  /** A short buzz when a long press picks something up. */
  haptics: boolean;
}

export const DEFAULT_DISPLAY_SETTINGS: DisplaySettings = {
  theme: 'auto',
  cardOpen: 'center',
  density: 'auto',
  haptics: true,
};

function oneOf<T extends string>(value: unknown, allowed: readonly T[], fallback: T): T {
  return allowed.includes(value as T) ? (value as T) : fallback;
}

/**
 * The settings out of what the server (or the device's memory) holds. A
 * member that is missing or has a value this version does not know reads as
 * its default.
 */
export function readDisplaySettings(stored: unknown): DisplaySettings {
  const source = (typeof stored === 'object' && stored !== null ? stored : {}) as Record<
    string,
    unknown
  >;
  return {
    theme: oneOf(source.theme, THEME_SETTINGS, DEFAULT_DISPLAY_SETTINGS.theme),
    cardOpen: oneOf(source.cardOpen, CARD_OPEN_SETTINGS, DEFAULT_DISPLAY_SETTINGS.cardOpen),
    density: oneOf(source.density, DENSITY_SETTINGS, DEFAULT_DISPLAY_SETTINGS.density),
    haptics:
      typeof source.haptics === 'boolean' ? source.haptics : DEFAULT_DISPLAY_SETTINGS.haptics,
  };
}

/** What the device says about itself. */
export interface DeviceTraits {
  /** The device is set to a dark appearance. */
  prefersDark: boolean;
  /** The main pointer is a finger. */
  coarsePointer: boolean;
}

export interface Display {
  theme: 'light' | 'dark';
  /** The way a card opens, among those that are built. */
  cardOpen: CardOpenSetting;
  density: 'compact' | 'comfortable';
  haptics: boolean;
}

/** The settings with every `auto` settled for this device. */
export function resolveDisplay(settings: DisplaySettings, device: DeviceTraits): Display {
  return {
    theme: settings.theme === 'auto' ? (device.prefersDark ? 'dark' : 'light') : settings.theme,
    cardOpen: CARD_OPEN_BUILT.includes(settings.cardOpen) ? settings.cardOpen : 'center',
    density:
      settings.density === 'auto'
        ? device.coarsePointer
          ? 'comfortable'
          : 'compact'
        : settings.density,
    haptics: settings.haptics,
  };
}

/**
 * The colour of the page in each theme, for the browser's own bars
 * (`theme-color`). It has to be a plain colour: the element is read before
 * and without the style sheet. The same two values are in index.html; a test
 * holds all of them to the `--background` token.
 */
export const THEME_COLORS = { light: '#ffffff', dark: '#0a0a0a' } as const;
