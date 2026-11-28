/** The entries of a menu that copy a body as Markdown and save it as a `.md` file. */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { markdownActions, withHeading } from '$lib/components/markdown/markdown-actions.ts';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('the Markdown of a menu', () => {
  it('heads the body with the title, and ends in a new line', () => {
    expect(withHeading('Plan  of\nthe week', 'body')).toBe('# Plan of the week\n\nbody\n');
    expect(withHeading('', 'body\n')).toBe('body\n');
    expect(withHeading('Empty', '')).toBe('# Empty\n');
  });

  it('is not offered where nothing reads Markdown, as through a public link', () => {
    const none = markdownActions({
      read: undefined,
      title: 'x',
      onCopied: () => undefined,
      onRefused: () => undefined,
    });
    expect(none).toEqual([]);
  });

  it('copies what is read, and says why where it is not read', async () => {
    const written: string[] = [];
    vi.stubGlobal('ClipboardItem', undefined);
    vi.stubGlobal('navigator', {
      clipboard: {
        writeText: (copied: string) => {
          written.push(copied);
          return Promise.resolve();
        },
      },
    });
    const copied = vi.fn();
    const refused = vi.fn();
    const [copy] = markdownActions({
      read: () => Promise.resolve('tick'),
      title: 'Plan',
      onCopied: copied,
      onRefused: refused,
    });
    copy?.run?.();
    await vi.waitFor(() => {
      expect(copied).toHaveBeenCalled();
    });
    expect(written).toEqual(['# Plan\n\ntick\n']);

    const failure = new Error('not there');
    const [failing] = markdownActions({
      read: () => Promise.reject(failure),
      title: 'Plan',
      onCopied: copied,
      onRefused: refused,
    });
    failing?.run?.();
    await vi.waitFor(() => {
      expect(refused).toHaveBeenCalledWith(failure);
    });
    expect(copied).toHaveBeenCalledTimes(1);
  });
});
