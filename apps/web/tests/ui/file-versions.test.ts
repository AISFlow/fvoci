/**
 * The list of a file's versions in the viewer (FileVersions.svelte): a row a
 * version with when, how large and by whom; what a reader and an editor may
 * do with one; and what it says when a write was refused.
 */
import { cleanup, render, screen, within } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import FileVersions from '$lib/components/file-viewer/FileVersions.svelte';
import {
  VersionsList,
  type FileVersion,
  type VersionsSource,
  type WriteRefusal,
} from '$lib/file-viewer/versions.svelte.ts';

const version = (number: number, more: Partial<FileVersion> = {}): FileVersion => ({
  version: number,
  // Noon: the same day wherever the test runs.
  createdAt: Date.UTC(2026, 0, number, 12),
  createdBy: 'u1',
  size: 1024 * number,
  isCurrent: false,
  restoredFrom: null,
  ...more,
});

const upload = { fileId: 'up.txt', name: 'mine.txt', size: 4, contentType: 'text/plain' };

function sourceOf(more: Partial<VersionsSource> = {}): VersionsSource {
  return {
    canWrite: true,
    list: () =>
      Promise.resolve([
        version(3, { isCurrent: true, restoredFrom: 1, createdBy: '' }),
        version(2, { createdBy: 'gone' }),
        version(1),
      ]),
    bytes: () => Promise.resolve(new Blob(['x'])),
    upload: () => Promise.resolve(upload),
    add: () => Promise.reject(new Error('refused')),
    restore: () => Promise.reject(new Error('refused')),
    attachApart: () => Promise.resolve(),
    refusalOf: (): WriteRefusal => 'stale',
    userName: (userId) => (userId === 'u1' ? 'Ada' : undefined),
    learnUsers: () => Promise.resolve(),
    me: 'u1',
    lease: () => Promise.resolve({ editing: false, editorId: '' }),
    holdLease: () => Promise.resolve({ leaseId: 'lease', seconds: 90 }),
    releaseLease: () => Promise.resolve(),
    ...more,
  };
}

async function shown(source = sourceOf(), shownVersion = 3) {
  const list = new VersionsList(source);
  const calls = { onView: vi.fn(), onDownload: vi.fn(), onWritten: vi.fn(), onClose: vi.fn() };
  const { rerender } = render(FileVersions, {
    list,
    source,
    shown: shownVersion,
    accept: '.txt',
    ...calls,
  });
  await vi.waitFor(() => {
    expect(screen.getByTestId('file-versions-list')).toBeTruthy();
  });
  const rows = [...screen.getByTestId('file-versions-list').querySelectorAll('li')];
  return { list, calls, rows, rerender };
}

afterEach(cleanup);

/** The three rows of the list. */
function three(rows: HTMLElement[]): [HTMLElement, HTMLElement, HTMLElement] {
  const [newest, middle, first] = rows;
  if (!newest || !middle || !first) throw new Error('the list has fewer rows than three');
  return [newest, middle, first];
}

describe('the list of the versions of a file', () => {
  it('shows each version with when, how large and by whom, the newest first', async () => {
    const { rows } = await shown();
    expect(rows.map((row) => row.dataset.version)).toEqual(['3', '2', '1']);
    const [newest, middle, first] = three(rows);
    expect(within(newest).getByTestId('file-version-current').textContent).toContain('Current');
    expect(newest.dataset.shown).toBe('');
    expect(newest.textContent).toContain('Restored from version 1');
    expect(newest.textContent).toContain('3.0 KiB');
    // Nobody known, and somebody whose name is not: the same words as elsewhere.
    for (const row of [newest, middle]) {
      expect(within(row).getByTestId('file-version-by').textContent).not.toContain('Ada');
    }
    expect(within(first).getByTestId('file-version-by').textContent).toBe('By Ada');
    expect(within(first).getByTestId('file-version-number').textContent.trim()).toBe('Version 1');
    expect(first.querySelector('time')?.getAttribute('datetime')).toBe('2026-01-01T12:00:00.000Z');
  });

  it('lets a version be looked at and downloaded, but for the one on screen', async () => {
    const { rows, calls } = await shown();
    const [newest, , first] = three(rows);
    expect(within(newest).getByTestId('file-version-view').getAttribute('aria-disabled')).toBe(
      'true',
    );
    within(newest).getByTestId('file-version-view').click();
    expect(calls.onView).not.toHaveBeenCalled();
    within(first).getByTestId('file-version-view').click();
    within(first).getByTestId('file-version-download').click();
    expect(calls.onView.mock.calls[0]?.[0]).toMatchObject({ version: 1 });
    expect(calls.onDownload.mock.calls[0]?.[0]).toMatchObject({ version: 1 });
    // Each button says which version it is of.
    expect(within(first).getByTestId('file-version-view').getAttribute('aria-label')).toMatch(
      // Noon in Greenwich is the first or, far east, the second.
      /^View version 1, of January 0[12] at /,
    );
  });

  it('keeps the focus on the button of the version it showed, which is then unavailable', async () => {
    const { rows, calls, rerender } = await shown();
    const [, , first] = three(rows);
    const view = within(first).getByTestId<HTMLButtonElement>('file-version-view');
    view.focus();
    view.click();
    expect(calls.onView).toHaveBeenCalledTimes(1);
    await rerender({ shown: 1 });
    expect(view.getAttribute('aria-disabled')).toBe('true');
    expect(document.activeElement).toBe(view);
    view.click();
    expect(calls.onView).toHaveBeenCalledTimes(1);
  });

  it('offers going back and a new version to who may write, and to nobody else', async () => {
    const editor = await shown();
    const [newest, middle] = three(editor.rows);
    expect(within(newest).queryByTestId('file-version-restore')).toBeNull();
    expect(within(middle).getByTestId('file-version-restore')).toBeTruthy();
    expect(screen.getByTestId('file-version-add')).toBeTruthy();
    expect(screen.getByTestId<HTMLInputElement>('file-version-input').accept).toBe('.txt');
  });

  it('offers a reader neither', async () => {
    await shown(sourceOf({ canWrite: false }));
    expect(screen.queryByTestId('file-version-add')).toBeNull();
    expect(screen.queryAllByTestId('file-version-restore')).toEqual([]);
    expect(screen.getAllByTestId('file-version-download')).toHaveLength(3);
  });

  it('says that somebody wrote a version meanwhile, and offers the file under another name', async () => {
    const attachApart = vi.fn(() => Promise.resolve());
    const { list } = await shown(sourceOf({ attachApart }));
    expect(await list.add(new File(['mine'], 'mine.txt'))).toBeUndefined();
    const refusal = await screen.findByTestId('file-version-refusal');
    expect(refusal.getAttribute('role')).toBe('alert');
    expect(refusal.dataset.refusal).toBe('stale');
    expect(refusal.textContent).toContain('Save under another name');
    screen.getByTestId('file-version-save-apart').click();
    await vi.waitFor(() => {
      expect(screen.queryByTestId('file-version-refusal')).toBeNull();
    });
    expect(attachApart).toHaveBeenCalledWith(upload);
  });

  it('says that a version is kept no more, with nothing to save', async () => {
    const { list } = await shown(sourceOf({ refusalOf: () => 'gone' }));
    const [, , first] = list.versions as [FileVersion, FileVersion, FileVersion];
    expect(await list.restore(first)).toBeUndefined();
    const refusal = await screen.findByTestId('file-version-refusal');
    expect(refusal.textContent).toContain('not kept any more');
    expect(screen.queryByTestId('file-version-save-apart')).toBeNull();
    screen.getByTestId('file-version-dismiss').click();
    await vi.waitFor(() => {
      expect(screen.queryByTestId('file-version-refusal')).toBeNull();
    });
  });

  it('says so when the versions could not be read, and reads them again when asked', async () => {
    let fails = true;
    const source = sourceOf({
      list: () => (fails ? Promise.reject(new Error('down')) : Promise.resolve([version(1)])),
    });
    const list = new VersionsList(source);
    render(FileVersions, {
      list,
      source,
      shown: undefined,
      accept: '',
      onView: vi.fn(),
      onDownload: vi.fn(),
      onWritten: vi.fn(),
      onClose: vi.fn(),
    });
    const retry = await screen.findByRole('button', { name: 'Try again' });
    fails = false;
    retry.click();
    await vi.waitFor(() => {
      expect(screen.getByTestId('file-versions-list').querySelectorAll('li')).toHaveLength(1);
    });
  });

  it('reactively updates formatted timestamps when the regional prop changes', async () => {
    const source = sourceOf({
      list: () => Promise.resolve([version(1, { createdAt: Date.UTC(2026, 0, 1, 20, 15, 0) })]),
    });
    const list = new VersionsList(source);
    const { rerender } = render(FileVersions, {
      list,
      source,
      shown: 1,
      accept: '.txt',
      regional: { timeZone: 'UTC', weekStart: 1, hourCycle: 'h23' },
      onView: vi.fn(),
      onDownload: vi.fn(),
      onWritten: vi.fn(),
      onClose: vi.fn(),
    });
    await vi.waitFor(() => {
      expect(screen.getByTestId('file-versions-list')).toBeTruthy();
    });
    const row = screen.getByTestId('file-versions-list').querySelector('li');
    expect(row?.textContent).toContain('January 01 at 20:15');

    await rerender({
      regional: { timeZone: 'Asia/Seoul', weekStart: 7, hourCycle: 'h12' },
    });
    await vi.waitFor(() => {
      expect(row?.textContent).toContain('January 02 at 5:15 AM');
    });
  });
});
