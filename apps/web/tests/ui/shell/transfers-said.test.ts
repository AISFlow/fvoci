/**
 * What the stack of transfers (Transfers.svelte) tells a screen reader: a
 * line of its log for each start, end and failure, so that two at once are
 * both read and the same words again are read again; what was said before
 * the workspace shown was is not said again when it comes back.
 */
import { cleanup, render, screen } from '@testing-library/svelte';
import { flushSync, tick } from 'svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import Transfers from '$lib/features/shell/Transfers.svelte';
import type { Workspace } from '$lib/state/index.ts';
import { Transfers as TransferList, UploadKept } from '$lib/state/transfers.svelte.ts';

import { ShownWorkspace } from './shown-workspace.svelte.ts';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  document.body.replaceChildren();
});

const never = (): Promise<string> => new Promise(() => undefined);
const file = (name: string): File => new File(['x'], name);

/** One stack seen from two workspaces, as the page shows one and then the other. */
function twoWorkspaces() {
  const shell = {
    report: () => undefined,
    ends: () => false,
    info: { id: 'one' } as { id: string } | undefined,
  };
  const transfers = new TransferList(shell);
  const shown = (id: string) => ({ transfers, info: { id } }) as unknown as Workspace;
  return { shell, transfers, one: shown('one'), two: shown('two') };
}

const lines = (): string[] =>
  [...screen.getByTestId('transfers-said').querySelectorAll('p')].map((p) => p.textContent);

describe('what the stack says', () => {
  it('reads each of two started at once, and the same words twice', async () => {
    const { transfers, one } = twoWorkspaces();
    render(Transfers, { workspace: one });
    expect(screen.getByTestId('transfers-said').getAttribute('role')).toBe('log');
    transfers.upload({ file: file('same.txt'), send: never });
    transfers.upload({ file: file('same.txt'), send: never });
    await tick();
    expect(lines()).toEqual(['Uploading same.txt', 'Uploading same.txt']);
  });

  it('says nothing again of what was said before the workspace shown came back', async () => {
    const { shell, transfers, one, two } = twoWorkspaces();
    const view = render(Transfers, { workspace: one });
    transfers.upload({ file: file('here.txt'), send: never });
    await tick();
    expect(lines()).toEqual(['Uploading here.txt']);
    await view.rerender({ workspace: two });
    expect(lines()).toEqual([]);
    shell.info = { id: 'two' };
    transfers.upload({ file: file('there.txt'), send: never });
    await tick();
    expect(lines()).toEqual(['Uploading there.txt']);
    await view.rerender({ workspace: one });
    expect(lines()).toEqual([]);
    shell.info = { id: 'one' };
    transfers.upload({ file: file('again.txt'), send: never });
    await tick();
    expect(lines()).toEqual(['Uploading again.txt']);
  });

  it('reads what is said as the workspace shown is told anew, the same one', async () => {
    const { shell, transfers } = twoWorkspaces();
    const shown = new ShownWorkspace(transfers, 'one');
    render(Transfers, { workspace: shown.workspace });
    // The page replaces the info (a workspace entered again) as a file starts.
    shown.info = { id: 'one' };
    shell.info = { id: 'one' };
    transfers.upload({ file: file('now.txt'), send: never });
    flushSync();
    expect(lines()).toEqual(['Uploading now.txt']);
  });

  it('names the key that reaches it once', async () => {
    const { transfers, one } = twoWorkspaces();
    render(Transfers, { workspace: one });
    transfers.upload({ file: file('a.txt'), send: never });
    await tick();
    const stack = screen.getByRole('region', { name: 'Transfers (F8)' });
    expect(stack.hasAttribute('aria-keyshortcuts')).toBe(false);
  });
});

describe('the stack reached by F8', () => {
  const refused = (): Promise<string> => Promise.reject(new UploadKept(new Error('down')));

  it('is not held out over a sheet any more once the focus went with the last of it', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const { transfers, one } = twoWorkspaces();
    render(Transfers, { workspace: one });
    const field = document.createElement('input');
    document.body.append(field);
    field.focus();
    transfers.upload({ file: file('a.txt'), send: () => Promise.reject(new Error('down')) });
    await vi.waitFor(() => screen.getByRole('button', { name: 'Dismiss a.txt' }));
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'F8' }));
    await tick();
    const dismiss = screen.getByRole('button', { name: 'Dismiss a.txt' });
    expect(document.activeElement).toBe(dismiss);
    expect(screen.getByTestId('transfers')).toHaveAttribute('data-reached');
    dismiss.click();
    await vi.waitFor(() => {
      expect(document.activeElement).toBe(field);
    });
    expect(screen.queryByTestId('transfers')).toBeNull();
    // The next one, with a sheet open, steps aside again.
    transfers.upload({ file: file('b.txt'), send: never });
    await tick();
    expect(screen.getByTestId('transfers')).not.toHaveAttribute('data-reached');
  });

  it('stays shown while the window is away, the focus coming back to it', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const { transfers, one } = twoWorkspaces();
    render(Transfers, { workspace: one });
    transfers.upload({ file: file('a.txt'), send: () => Promise.reject(new Error('down')) });
    await vi.waitFor(() => screen.getByRole('button', { name: 'Dismiss a.txt' }));
    window.dispatchEvent(new KeyboardEvent('keydown', { key: 'F8' }));
    await tick();
    const dismiss = screen.getByRole('button', { name: 'Dismiss a.txt' });
    expect(document.activeElement).toBe(dismiss);
    // Another window comes to the front: the focus goes out of the page, to nothing in it.
    vi.spyOn(document, 'hasFocus').mockReturnValue(false);
    dismiss.dispatchEvent(new FocusEvent('focusout', { bubbles: true, relatedTarget: null }));
    await tick();
    expect(screen.getByTestId('transfers')).toHaveAttribute('data-reached');
  });

  it('counts one kept, which is in, apart from those not uploaded', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    const { transfers, one } = twoWorkspaces();
    render(Transfers, { workspace: one });
    const kept = transfers.upload({ file: file('kept.txt'), send: refused });
    transfers.dismiss(kept.id);
    transfers.upload({ file: file('lost.txt'), send: () => Promise.reject(new Error('down')) });
    transfers.upload({ file: file('going.txt'), send: never });
    await vi.waitFor(() => {
      expect(transfers.list.map((each) => each.failure)).toEqual(['failed', undefined, 'kept']);
    });
    await tick();
    const group = document.querySelector('[data-transfer-group]');
    expect(group?.textContent).toContain('· 1 not uploaded');
  });
});
