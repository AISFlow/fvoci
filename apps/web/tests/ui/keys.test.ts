/** Native modifiers, visible hints and editor closing share one platform choice. */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { closesPlace } from '$lib/editor/place-state.ts';
import { SaveKey } from '$lib/editor/save-key.ts';
import {
  isApplePlatform,
  isSaveKey,
  primaryModifier,
  shortcutKeys,
  shortcutLabel,
} from '$lib/keys.ts';

afterEach(() => vi.restoreAllMocks());

for (const [platform, apple] of [
  ['MacIntel', true],
  ['iPhone', true],
  ['iPad', true],
  ['iPod', true],
  ['Win32', false],
  ['Linux x86_64', false],
  ['Linux armv8l', false],
  ['', false],
] as const) {
  describe(`keys on ${platform || 'an unknown platform'}`, () => {
    it('accepts exactly the native modifier and labels the same keys', () => {
      vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform);
      expect(isApplePlatform()).toBe(apple);
      expect(primaryModifier({ ctrlKey: !apple, metaKey: apple })).toBe(true);
      expect(primaryModifier({ ctrlKey: apple, metaKey: !apple })).toBe(false);
      expect(primaryModifier({ ctrlKey: true, metaKey: true })).toBe(false);
      expect(primaryModifier({ ctrlKey: false, metaKey: false })).toBe(false);
      expect(shortcutLabel('Z')).toBe(apple ? '⌘Z' : 'Ctrl+Z');
      expect(shortcutLabel('Z', { shift: true })).toBe(apple ? '⇧⌘Z' : 'Ctrl+Shift+Z');
      expect(shortcutLabel('Enter', { alt: true })).toBe(apple ? '⌥⌘Enter' : 'Ctrl+Alt+Enter');
      expect(shortcutKeys('Z', { shift: true })).toBe(apple ? 'Meta+Shift+Z' : 'Control+Shift+Z');
    });

    it('closes an editor place with its native modifier, never a composing Enter or Escape', () => {
      vi.spyOn(navigator, 'platform', 'get').mockReturnValue(platform);
      const modifier = { ctrlKey: !apple, metaKey: apple };
      expect(closesPlace(new KeyboardEvent('keydown', { key: 'Enter', ...modifier }))).toBe(true);
      expect(
        closesPlace(
          new KeyboardEvent('keydown', { key: 'Enter', ctrlKey: apple, metaKey: !apple }),
        ),
      ).toBe(false);
      for (const key of ['Enter', 'Escape']) {
        expect(
          closesPlace(new KeyboardEvent('keydown', { key, ...modifier, isComposing: true })),
        ).toBe(false);
        expect(closesPlace(new KeyboardEvent('keydown', { key, ...modifier, keyCode: 229 }))).toBe(
          false,
        );
      }
      expect(closesPlace(new KeyboardEvent('keydown', { key: 'Escape' }))).toBe(true);
    });

    it('keeps a version with S and its native modifier alone, by letter or, on other letters, by place', () => {
      const modifier = { ctrlKey: !apple, metaKey: apple };
      const press = (init: KeyboardEventInit): boolean =>
        isSaveKey(new KeyboardEvent('keydown', { code: 'KeyS', ...init }), platform);
      expect(press({ key: 's', ...modifier })).toBe(true);
      expect(press({ key: 'S', ...modifier })).toBe(true);
      // Hangul's layout, and a syllable being composed: the key where S is.
      expect(press({ key: 'ㄴ', ...modifier })).toBe(true);
      expect(press({ key: 'Process', ...modifier })).toBe(true);
      // Another Latin layout: its letter, wherever the key is.
      expect(press({ key: 'o', ...modifier })).toBe(false);
      expect(press({ key: 's', code: 'KeyO', ...modifier })).toBe(true);
      expect(press({ key: 's', ctrlKey: apple, metaKey: !apple })).toBe(false);
      expect(press({ key: 's', ctrlKey: true, metaKey: true })).toBe(false);
      expect(press({ key: 's', shiftKey: true, ...modifier })).toBe(false);
      expect(press({ key: 's', altKey: true, ...modifier })).toBe(false);
      expect(press({ key: 's' })).toBe(false);
    });

    it("keeps the browser from saving the page in a body's editor, and asks once, after a syllable", () => {
      const modifier = { key: 's', code: 'KeyS', ctrlKey: !apple, metaKey: apple };
      const asked = vi.fn();
      const key = new SaveKey(() => asked);
      const press = (init: KeyboardEventInit, composing = false): KeyboardEvent => {
        const event = new KeyboardEvent('keydown', { cancelable: true, ...init });
        key.down(event, composing, platform);
        return event;
      };
      expect(press(modifier).defaultPrevented).toBe(true);
      expect(asked).toHaveBeenCalledTimes(1);
      // Held down: no more asks, and still no dialog of the browser.
      expect(press({ ...modifier, repeat: true }).defaultPrevented).toBe(true);
      expect(asked).toHaveBeenCalledTimes(1);
      // Other keys are the editor's.
      expect(press({ key: 'z', ctrlKey: !apple, metaKey: apple }).defaultPrevented).toBe(false);
      // During a syllable: asked once it is over, and not again.
      expect(press(modifier, true).defaultPrevented).toBe(true);
      expect(asked).toHaveBeenCalledTimes(1);
      key.composed();
      key.composed();
      expect(asked).toHaveBeenCalledTimes(2);
      // The editor went away during the syllable: nothing is asked.
      press(modifier, true);
      key.drop();
      key.composed();
      expect(asked).toHaveBeenCalledTimes(2);
      // An editor that keeps no version keeps the browser from saving the page all the same.
      const none = new SaveKey(() => undefined);
      const event = new KeyboardEvent('keydown', { cancelable: true, ...modifier });
      none.down(event, false, platform);
      expect(event.defaultPrevented).toBe(true);
    });
  });
}
