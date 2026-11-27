/**
 * The small pieces of the shell that decide without drawing: the keys for
 * undo and redo, the readout's parameter and memory, the fold of the
 * sidebar, and the lists of the settings dialog.
 */
import { afterEach, describe, expect, it } from 'vitest';

import { devParam } from '$lib/features/shell/dev-param.ts';
import { DEV_STORAGE_KEY, devReadout } from '$lib/features/shell/dev.svelte.ts';
import {
  languageName,
  timeZoneChoices,
  timeZoneLabel,
} from '$lib/features/shell/settings-logic.ts';
import { SIDEBAR_STORAGE_KEY, SidebarState } from '$lib/features/shell/sidebar-state.svelte.ts';
import { inField, undoKey } from '$lib/features/shell/undo-keys.ts';

afterEach(() => {
  localStorage.clear();
  document.body.replaceChildren();
});

function press(
  init: KeyboardEventInit,
  target: Element = document.body,
): Parameters<typeof undoKey>[0] {
  const event = new KeyboardEvent('keydown', { bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(event);
  return event;
}

function element(html: string): Element {
  document.body.innerHTML = html;
  const made = document.body.firstElementChild;
  if (!made) throw new Error('no element');
  return made;
}

describe('the keys for undo and redo', () => {
  for (const platform of ['MacIntel', 'iPhone', 'iPad', 'Win32', 'Linux x86_64']) {
    const apple = ['MacIntel', 'iPhone', 'iPad'].includes(platform);
    it(`${platform} uses only its primary modifier, with Shift for redo`, () => {
      const modifier = apple ? { metaKey: true } : { ctrlKey: true };
      const other = apple ? { ctrlKey: true } : { metaKey: true };
      expect(undoKey(press({ key: 'z', ...modifier }), platform)).toBe('undo');
      expect(undoKey(press({ key: 'Z', ...modifier, shiftKey: true }), platform)).toBe('redo');
      expect(undoKey(press({ key: 'z', ...other }), platform)).toBeUndefined();
      expect(undoKey(press({ key: 'z', ctrlKey: true, metaKey: true }), platform)).toBeUndefined();
      expect(
        undoKey(press({ key: 'z', ...modifier, isComposing: true }), platform),
      ).toBeUndefined();
      // eslint-disable-next-line @typescript-eslint/no-deprecated -- Safari's composition confirmation uses 229
      expect(undoKey(press({ key: 'z', ...modifier, keyCode: 229 }), platform)).toBeUndefined();
    });
  }

  it('a plain z, another letter, or Alt with it is not asked for', () => {
    expect(undoKey(press({ key: 'z' }))).toBeUndefined();
    expect(undoKey(press({ key: 'y', ctrlKey: true }))).toBeUndefined();
    expect(undoKey(press({ key: 'z', ctrlKey: true, altKey: true }))).toBeUndefined();
  });

  it('a key held down asks once: its repeats are not asked for', () => {
    expect(undoKey(press({ key: 'z', ctrlKey: true, repeat: true }))).toBeUndefined();
    expect(
      undoKey(press({ key: 'Z', ctrlKey: true, shiftKey: true, repeat: true })),
    ).toBeUndefined();
  });

  it('inside a field the keys are the field’s own', () => {
    for (const html of [
      '<input />',
      '<textarea></textarea>',
      '<select></select>',
      '<div contenteditable="true"><p>text</p></div>',
    ]) {
      const field = element(html);
      expect(undoKey(press({ key: 'z', ctrlKey: true }, field.firstElementChild ?? field))).toBe(
        undefined,
      );
    }
  });

  it('a part that says it cannot be edited is not a field', () => {
    const plain = element('<div contenteditable="false"><span>text</span></div>');
    expect(inField(plain.firstElementChild)).toBe(false);
    expect(undoKey(press({ key: 'z', ctrlKey: true }, plain))).toBe('undo');
  });

  it('an editor of a body read-only for now is still a field', () => {
    const editor = element('<div data-body-editor contenteditable="false"><p>text</p></div>');
    expect(inField(editor.firstElementChild)).toBe(true);
    expect(undoKey(press({ key: 'z', ctrlKey: true }, editor))).toBeUndefined();
  });

  it('a press something else has already handled is left alone', () => {
    const event = new KeyboardEvent('keydown', { key: 'z', ctrlKey: true, cancelable: true });
    event.preventDefault();
    expect(undoKey(event)).toBeUndefined();
  });
});

describe('the layout readout', () => {
  it('the address turns it on with ?dev=1 and off with ?dev=0', () => {
    expect(devParam('?dev=1')).toBe(true);
    expect(devParam('?dev')).toBe(true);
    expect(devParam('?x=1&dev=true')).toBe(true);
    expect(devParam('?dev=0')).toBe(false);
    expect(devParam('?dev=false')).toBe(false);
  });

  it('an address without the parameter says nothing', () => {
    expect(devParam('')).toBeUndefined();
    expect(devParam('?developer=1')).toBeUndefined();
  });

  it('is off unless asked for', () => {
    devReadout.start('');
    expect(devReadout.shown).toBe(false);
  });

  it('the address decides, and the device remembers the answer', () => {
    devReadout.start('?dev=1');
    expect(devReadout.shown).toBe(true);
    expect(localStorage.getItem(DEV_STORAGE_KEY)).toBe('1');

    devReadout.start('');
    expect(devReadout.shown).toBe(true);

    devReadout.start('?dev=0');
    expect(devReadout.shown).toBe(false);
    devReadout.start('');
    expect(devReadout.shown).toBe(false);
  });

  it('the switch of the settings is remembered the same way', () => {
    devReadout.set(true);
    expect(localStorage.getItem(DEV_STORAGE_KEY)).toBe('1');
    devReadout.set(false);
    expect(localStorage.getItem(DEV_STORAGE_KEY)).toBe('0');
  });
});

describe('the fold of the sidebar', () => {
  it('starts unfolded, with the drawer closed', () => {
    const state = new SidebarState();
    expect(state.collapsed).toBe(false);
    expect(state.drawerOpen).toBe(false);
  });

  it('the device remembers the fold, and not the drawer', () => {
    const state = new SidebarState();
    state.setCollapsed(true);
    state.drawerOpen = true;
    expect(localStorage.getItem(SIDEBAR_STORAGE_KEY)).toBe('collapsed');

    const next = new SidebarState();
    expect(next.collapsed).toBe(true);
    expect(next.drawerOpen).toBe(false);

    next.setCollapsed(false);
    expect(new SidebarState().collapsed).toBe(false);
  });

  it('something else under the key reads as unfolded', () => {
    localStorage.setItem(SIDEBAR_STORAGE_KEY, '{"collapsed":true}');
    expect(new SidebarState().collapsed).toBe(false);
  });
});

describe('the lists of the settings', () => {
  it('the time zones are those the browser knows, with UTC and the chosen one, each once, in order', () => {
    expect(timeZoneChoices(['Europe/Berlin', 'Asia/Seoul'], 'auto')).toEqual([
      'Asia/Seoul',
      'Europe/Berlin',
      'UTC',
    ]);
    expect(timeZoneChoices(['Europe/Berlin', 'Asia/Seoul', 'UTC'], 'Asia/Seoul')).toEqual([
      'Asia/Seoul',
      'Europe/Berlin',
      'UTC',
    ]);
  });

  it('a chosen zone the browser does not list is still there to be seen', () => {
    expect(timeZoneChoices(['Asia/Seoul'], 'Asia/Calcutta')).toEqual([
      'Asia/Calcutta',
      'Asia/Seoul',
      'UTC',
    ]);
  });

  it('a zone is written with spaces', () => {
    expect(timeZoneLabel('America/Argentina/Buenos_Aires')).toBe('America/Argentina/Buenos Aires');
    expect(timeZoneLabel('UTC')).toBe('UTC');
  });

  it('a language is named in itself, with a capital where it has them', () => {
    expect(languageName('ko')).toBe('한국어');
    expect(languageName('en')).toBe('English');
    expect(languageName('de')).toBe('Deutsch');
    expect(languageName('fr')).toBe('Français');
    expect(languageName('es')).toBe('Español');
  });

  it('a tag the browser has no name for is shown as it is', () => {
    expect(languageName('not a tag')).toBe('not a tag');
  });
});
