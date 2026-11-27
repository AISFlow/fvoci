/**
 * The time zone of who leaves it to follow the device (`state/time-zone-sync.svelte.ts`):
 * the server is told the device's zone, the choice is kept apart, and a zone picked by
 * hand is not written over.
 */
import { describe, expect, it } from 'vitest';

import { browserTimeZone } from '$lib/regional.ts';
import {
  BackendError,
  chooseTimeZone,
  followsDeviceTimeZone,
  syncDeviceTimeZone,
} from '$lib/state/index.ts';
import { FOLLOWS_DEVICE } from '$lib/state/time-zone-sync.svelte.ts';

import { setup } from './support.ts';

/** A zone the device is not in (the tests run in several: `tools/test-zones.ts`). */
const ELSEWHERE = browserTimeZone() === 'America/Lima' ? 'Europe/Oslo' : 'America/Lima';

async function entered(timeZone: string, follows?: boolean) {
  const made = setup();
  await made.backend.patchSettings({ timeZone });
  if (follows) await made.backend.patchScreenState({ [FOLLOWS_DEVICE]: '1' });
  await made.workspace.load();
  return made;
}

describe("the device's time zone", () => {
  it('is told to the server for who never set one, and "Automatic" is kept as chosen', async () => {
    const { workspace, backend } = await entered('auto');
    expect(await syncDeviceTimeZone(workspace)).toBe(true);
    expect((await backend.settings()).timeZone).toBe(browserTimeZone());
    expect((await backend.screenState())[FOLLOWS_DEVICE]).toBe('1');
    expect(followsDeviceTimeZone(workspace)).toBe(true);
    // Once for the workspace: a second time tells nothing.
    expect(await syncDeviceTimeZone(workspace)).toBe(false);
  });

  it('is told again where the reader follows the device and the zone kept is another', async () => {
    const { workspace, backend } = await entered(ELSEWHERE, true);
    expect(await syncDeviceTimeZone(workspace)).toBe(true);
    expect((await backend.settings()).timeZone).toBe(browserTimeZone());
    expect(followsDeviceTimeZone(workspace)).toBe(true);
  });

  it('is not told over a zone picked by hand', async () => {
    const { workspace, backend } = await entered(ELSEWHERE);
    expect(await syncDeviceTimeZone(workspace)).toBe(false);
    expect((await backend.settings()).timeZone).toBe(ELSEWHERE);
    expect(followsDeviceTimeZone(workspace)).toBe(false);
  });

  it('is what "Automatic" picks, and a zone picked by hand ends following it', async () => {
    const { workspace, backend } = await entered(ELSEWHERE);
    await syncDeviceTimeZone(workspace);

    await chooseTimeZone(workspace, 'auto');
    expect(followsDeviceTimeZone(workspace)).toBe(true);
    expect((await backend.settings()).timeZone).toBe(browserTimeZone());
    expect((await backend.screenState())[FOLLOWS_DEVICE]).toBe('1');

    await chooseTimeZone(workspace, ELSEWHERE);
    expect(followsDeviceTimeZone(workspace)).toBe(false);
    expect((await backend.settings()).timeZone).toBe(ELSEWHERE);
    expect((await backend.screenState())[FOLLOWS_DEVICE]).toBeUndefined();
  });

  it('stays as it was where the server refuses the change', async () => {
    const { workspace, backend } = await entered(ELSEWHERE);
    await syncDeviceTimeZone(workspace);
    backend.failNext('patchSettings', new BackendError(500));
    await chooseTimeZone(workspace, 'auto');
    expect(followsDeviceTimeZone(workspace)).toBe(false);
    expect(workspace.settings.timeZone).toBe(ELSEWHERE);
    expect((await backend.screenState())[FOLLOWS_DEVICE]).toBeUndefined();
  });
});
