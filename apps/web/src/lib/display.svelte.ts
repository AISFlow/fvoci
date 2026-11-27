/**
 * The display settings in effect: how bright, how a card opens, how roomy,
 * and whether the device may buzz.
 *
 * `display.settings` is what the reader chose (part of their account, see
 * `logic/display.ts`), `display.resolved` is what is on screen once every
 * `auto` has been settled for this device. `watchDisplay` keeps the root
 * element in step: the `dark` class (through `theme.svelte.ts`), the
 * `data-density` attribute the `roomy:` variant of the style sheet looks at,
 * and the colour of the browser's own bars.
 *
 * The last settings are also remembered on the device. The script at the
 * top of index.html reads them before anything is drawn, so that a reader
 * who chose the dark screen does not see a light one first.
 */
import { MediaQuery } from 'svelte/reactivity';

import {
  DEFAULT_DISPLAY_SETTINGS,
  readDisplaySettings,
  resolveDisplay,
  THEME_COLORS,
  type Display,
  type DisplaySettings,
} from './logic/display.ts';
import { applyTheme } from './theme.svelte.ts';

/** Where the device remembers the settings. index.html reads the same key. */
export const DISPLAY_STORAGE_KEY = 'fvoci:display';

function remembered(): DisplaySettings {
  try {
    return readDisplaySettings(JSON.parse(localStorage.getItem(DISPLAY_STORAGE_KEY) ?? '{}'));
  } catch {
    // No storage, or something else's text under the key: start from the defaults.
    return DEFAULT_DISPLAY_SETTINGS;
  }
}

class DisplayState {
  settings = $state<DisplaySettings>(remembered());
  readonly #dark = new MediaQuery('(prefers-color-scheme: dark)');
  readonly #coarse = new MediaQuery('(pointer: coarse)');

  /** The settings with every `auto` settled for this device. */
  readonly resolved = $derived<Display>(
    resolveDisplay(this.settings, {
      prefersDark: this.#dark.current,
      coarsePointer: this.#coarse.current,
    }),
  );
}

export const display = new DisplayState();

/** Replaces the settings: called when the reader's account settings arrive or change. */
export function setDisplaySettings(next: DisplaySettings): void {
  display.settings = next;
  try {
    localStorage.setItem(DISPLAY_STORAGE_KEY, JSON.stringify(next));
  } catch {
    // Remembering is a convenience; the settings still apply for this visit.
  }
}

function apply(resolved: Display, settings: DisplaySettings): void {
  const root = document.documentElement;
  applyTheme(settings.theme === 'auto' ? 'system' : settings.theme);
  root.dataset.density = resolved.density;
  root.style.colorScheme = resolved.theme;
  document
    .querySelector('meta[name="theme-color"]')
    ?.setAttribute('content', THEME_COLORS[resolved.theme]);
}

/** Starts keeping the root element in step with the settings. Returns the function that stops. */
export function watchDisplay(): () => void {
  return $effect.root(() => {
    $effect(() => {
      apply(display.resolved, display.settings);
    });
  });
}
