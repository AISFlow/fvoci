/**
 * What the viewer shows in the document's place (ViewerNotice.svelte): that
 * the file opens; for a file not drawn in its time, the question whether to
 * wait on for it or download it instead, whose answer keeps the focus in the
 * place; why a file cannot be shown, with the download alone.
 */
import { cleanup, render, screen } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import ViewerNotice from '$lib/components/file-viewer/ViewerNotice.svelte';
import type { Status } from '$lib/file-viewer/session.svelte.ts';

afterEach(() => {
  cleanup();
});

function shown(status: Status) {
  const onDownload = vi.fn();
  const onAnswer = vi.fn<(wait: boolean) => void>();
  const view = render(ViewerNotice, { status, onDownload, onAnswer });
  return { ...view, onDownload, onAnswer };
}

describe("what the viewer shows in the document's place", () => {
  it('says the file opens, and asks nothing, until it is late', () => {
    shown({ name: 'opening', format: 'xlsx', late: false });
    expect(screen.getByRole('status').textContent.trim()).toBe('Opening the file…');
    expect(screen.queryAllByRole('button')).toEqual([]);
  });

  it('asks whether to wait on for a file not drawn in its time; waiting on keeps the focus in the place', async () => {
    const { onAnswer, onDownload, rerender } = shown({
      name: 'opening',
      format: 'xlsx',
      late: true,
    });
    const place = screen.getByRole('status');
    expect(place.textContent).toContain('This file is taking long to show.');
    expect(place.textContent).toContain('Keep waiting, or download it to open it in another app.');
    const wait = screen.getByRole('button', { name: 'Keep waiting' });
    wait.focus();
    wait.click();
    expect(onAnswer).toHaveBeenCalledWith(true);
    expect(onDownload).not.toHaveBeenCalled();
    await rerender({ status: { name: 'opening', format: 'xlsx', late: false } });
    // The same place, still focused, now saying the file opens.
    expect(screen.getByRole('status')).toBe(place);
    expect(document.activeElement).toBe(place);
    expect(place.textContent.trim()).toBe('Opening the file…');
  });

  it('downloads a file not drawn in its time as the answer not to wait on, the focus staying on Download', async () => {
    const { onAnswer, onDownload, rerender } = shown({
      name: 'opening',
      format: 'xlsx',
      late: true,
    });
    const download = screen.getByRole('button', { name: 'Download' });
    download.focus();
    download.click();
    expect(onAnswer).toHaveBeenCalledWith(false);
    expect(onDownload).toHaveBeenCalledTimes(1);
    // The file is given up as too slow (FileViewer): the question goes, Download stays.
    await rerender({ status: { name: 'failed', reason: 'slow' } });
    expect(screen.getByRole('status').textContent).toContain('The file took too long to show.');
    expect(screen.getByRole('button', { name: 'Download' })).toBe(download);
    expect(document.activeElement).toBe(download);
  });

  it('offers a file it cannot show as a download, and asks nothing', () => {
    const { onAnswer, onDownload } = shown({ name: 'failed', reason: 'slow' });
    expect(screen.getByRole('status').textContent).toContain('The file took too long to show.');
    expect(screen.queryByRole('button', { name: 'Keep waiting' })).toBeNull();
    screen.getByRole('button', { name: 'Download' }).click();
    expect(onDownload).toHaveBeenCalledTimes(1);
    expect(onAnswer).not.toHaveBeenCalled();
  });

  it('shows nothing once the file is shown', () => {
    shown({ name: 'shown', format: 'xlsx' });
    expect(screen.queryByRole('status')).toBeNull();
  });
});
