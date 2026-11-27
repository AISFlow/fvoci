/** A refused board write takes back only what is still its own on the screen. */
import { expect, it, vi } from 'vitest';

import { BackendError } from '$lib/state/index.ts';

import { openFirstBoard } from './support.ts';

function held() {
  let release = (): void => undefined;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

it.each(['title', 'description'] as const)(
  'keeps the later %s while its write still waits',
  async (field) => {
    const { board, backend, workspace } = await openFirstBoard();
    const before = board.board?.title;
    const failed = held();
    const later = held();
    const patch = backend.patchContainer.bind(backend);
    vi.spyOn(backend, 'patchContainer')
      .mockImplementationOnce(async () => {
        await failed.promise;
        throw new BackendError(403);
      })
      .mockImplementationOnce(async (...args) => {
        await later.promise;
        return patch(...args);
      });
    try {
      const first = board.changeBoard((current) => ({ ...current, title: 'refused' }));
      const second = board.changeBoard((current) => ({ ...current, [field]: 'later' }));
      failed.release();
      await first;
      expect(board.board?.[field]).toBe('later');
      if (field === 'description') expect(board.board?.title).toBe(before);
      later.release();
      await second;
      await workspace.undoLast();
      expect(board.board?.title).toBe(before);
      expect((await backend.container(board.boardId)).title).toBe(before);
    } finally {
      failed.release();
      later.release();
      workspace.close();
    }
  },
);

it.each([false, true])(
  'rejects its own fields even when a direct board version is newer (title changed: %s)',
  async (titleChanged) => {
    const { board, backend, other, workspace } = await openFirstBoard();
    const before = board.board?.title;
    const failed = held();
    vi.spyOn(backend, 'patchContainer').mockImplementationOnce(async () => {
      await failed.promise;
      throw new BackendError(403);
    });
    try {
      const writing = board.changeBoard((current) => ({ ...current, title: 'refused' }));
      // The change message is held while the first write is pending.
      const version = await other.patchContainer(board.boardId, { icon: 'remote' });
      const shown = board.board;
      if (!shown) throw new Error('missing board');
      // A separate operation put a newer board version on screen directly.
      board.board = {
        ...shown,
        updatedAt: version.updatedAt + 1,
        description: 'direct',
        title: titleChanged ? 'direct title' : shown.title,
      };
      failed.release();
      await writing;
      expect(board.board.title).toBe(titleChanged ? 'direct title' : before);
      expect(board.board.description).toBe('direct');
      expect(board.board.updatedAt).toBe(version.updatedAt + 1);
    } finally {
      failed.release();
      workspace.close();
    }
  },
);

it('restores array properties when both writes are refused in reverse order', async () => {
  const { board, backend, workspace } = await openFirstBoard();
  await board.changeBoard((current) => ({
    ...current,
    properties: { ...current.properties, p: ['base'] },
  }));
  const first = held();
  const second = held();
  vi.spyOn(backend, 'patchContainer')
    .mockImplementationOnce(async () => {
      await first.promise;
      throw new BackendError(403);
    })
    .mockImplementationOnce(async () => {
      await second.promise;
      throw new BackendError(403);
    });
  try {
    const one = board.changeBoard((current) => ({
      ...current,
      properties: { ...current.properties, p: ['one'] },
    }));
    const two = board.changeBoard((current) => ({
      ...current,
      properties: { ...current.properties, p: ['two'] },
    }));
    second.release();
    await two;
    expect(board.board?.properties.p).toEqual(['one']);
    first.release();
    await one;
    expect(board.board?.properties.p).toEqual(['base']);
    expect((await backend.container(board.boardId)).properties.p).toEqual(['base']);
  } finally {
    first.release();
    second.release();
    workspace.close();
  }
});
