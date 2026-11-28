/**
 * What the references on screen name, as the workspace holds it: asked once
 * and together, kept current by the changes of the boards, and never a
 * title for what the reader may not see.
 */
import { describe, expect, it, vi } from 'vitest';

import { BackendError, referenceTitles } from '$lib/state/index.ts';

import { openFirstBoard, settle } from './support.ts';

const UNKNOWN = '01930000-0000-7000-8000-00000000dead';

async function opened() {
  const made = await openFirstBoard();
  const [alpha, beta] = ['Alpha', 'Beta'].map((title) =>
    made.board.realCards.find((card) => card.title === title),
  );
  if (!alpha || !beta) throw new Error('no cards');
  return { ...made, alpha, beta, titles: referenceTitles(made.workspace) };
}

describe('the titles of what references name', () => {
  it('are asked once for all that is drawn in a turn, and held', async () => {
    const { backend, workspace, alpha, beta, titles } = await opened();
    const asked = vi.spyOn(backend.references, 'resolve');
    expect(referenceTitles(workspace)).toBe(titles);

    expect(titles.of(alpha.id)).toBeUndefined();
    expect(titles.of(beta.id)).toBeUndefined();
    expect(titles.of(UNKNOWN)).toBeUndefined();
    expect(titles.of(alpha.id)).toBeUndefined();
    await settle();
    expect(asked).toHaveBeenCalledOnce();
    expect(asked.mock.calls[0]?.[1]).toEqual([alpha.id, beta.id, UNKNOWN]);
    expect(titles.of(alpha.id)).toMatchObject({ id: alpha.id, kind: 'card', title: 'Alpha' });
    expect(titles.of(beta.id)?.title).toBe('Beta');
    // What is not there and what the reader may not see are the same nothing.
    expect(titles.of(UNKNOWN)).toBeNull();
    await settle();
    expect(asked).toHaveBeenCalledOnce();
  });

  it('follow a card that is renamed, and one that is deleted', async () => {
    const { other, board, alpha, beta, titles } = await opened();
    titles.of(alpha.id);
    titles.of(beta.id);
    await settle();
    // Drawn again with the answer: what is drawn follows its board from then on.
    titles.of(alpha.id);
    titles.of(beta.id);
    await settle();

    await other.patchCard(board.boardId, alpha.id, { title: 'Renamed', icon: '🚀' });
    await settle();
    expect(titles.of(alpha.id)).toMatchObject({ title: 'Renamed', icon: '🚀' });

    await other.batchDeleteCards(board.boardId, [beta.id]);
    await settle();
    expect(titles.of(beta.id)).toBeNull();
  });

  it('follow the reader taken off the board of what is named', async () => {
    const { backend, world, board, alpha, titles } = await opened();
    titles.of(alpha.id);
    await settle();
    titles.of(alpha.id);
    await settle();
    // What the reader is told when another takes them off the board (removeMember).
    world.tellUsers([backend.userId], { kind: 'containerDeleted', containerId: board.boardId });
    await settle();
    expect(titles.of(alpha.id)).toBeNull();
  });

  it('never put back an answer older than a change heard while it was asked', async () => {
    const { backend, other, board, alpha, beta, titles } = await opened();
    titles.of(alpha.id);
    await settle();
    titles.of(alpha.id);
    await settle();
    // What names beta is asked; the answer, Alpha as it was, comes after the rename.
    const held = titles.of(alpha.id);
    if (!held) throw new Error('Alpha is not held');
    const before = { ...held };
    let answer: () => void = () => undefined;
    vi.spyOn(backend.references, 'to').mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          answer = () => {
            resolve([before]);
          };
        }),
    );
    const naming = titles.to(beta.id);
    await other.patchCard(board.boardId, alpha.id, { title: 'Renamed' });
    await settle();
    answer();
    await naming;
    expect(titles.of(alpha.id)?.title).toBe('Renamed');
  });

  it('show what could not be asked as what the reader may not see, and ask again later', async () => {
    const { backend, alpha, titles } = await opened();
    const asked = vi
      .spyOn(backend.references, 'resolve')
      .mockRejectedValueOnce(new BackendError(503));
    vi.useFakeTimers();
    try {
      expect(titles.of(alpha.id)).toBeUndefined();
      await vi.advanceTimersByTimeAsync(0);
      expect(titles.of(alpha.id)).toBeNull();
      // Drawn again meanwhile, nothing more is asked.
      expect(asked).toHaveBeenCalledOnce();
      await vi.advanceTimersByTimeAsync(5_000);
      expect(asked).toHaveBeenCalledTimes(2);
      expect(titles.of(alpha.id)?.title).toBe('Alpha');
    } finally {
      vi.useRealTimers();
    }
  });

  it('ask again together all that failed while a try was waited for', async () => {
    const { backend, alpha, beta, titles } = await opened();
    const asked = vi
      .spyOn(backend.references, 'resolve')
      .mockRejectedValueOnce(new BackendError(503))
      .mockRejectedValueOnce(new TypeError('Failed to fetch'));
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    vi.useFakeTimers();
    try {
      titles.of(alpha.id);
      await vi.advanceTimersByTimeAsync(0);
      titles.of(beta.id);
      await vi.advanceTimersByTimeAsync(0);
      expect([titles.of(alpha.id), titles.of(beta.id)]).toEqual([null, null]);
      await vi.advanceTimersByTimeAsync(5_000);
      expect(asked).toHaveBeenCalledTimes(3);
      expect(asked.mock.calls[2]?.[1]).toEqual([alpha.id, beta.id]);
      expect([titles.of(alpha.id)?.title, titles.of(beta.id)?.title]).toEqual(['Alpha', 'Beta']);
    } finally {
      vi.useRealTimers();
    }
  });

  it('do not ask again what the server refused', async () => {
    const { backend, alpha, titles } = await opened();
    const asked = vi
      .spyOn(backend.references, 'resolve')
      .mockRejectedValueOnce(new BackendError(404));
    vi.useFakeTimers();
    try {
      titles.of(alpha.id);
      await vi.advanceTimersByTimeAsync(0);
      expect(titles.of(alpha.id)).toBeNull();
      await vi.advanceTimersByTimeAsync(120_000);
      expect(asked).toHaveBeenCalledOnce();
    } finally {
      vi.useRealTimers();
    }
  });

  it('put a change heard while an identifier is asked before the answer', async () => {
    const { backend, other, board, alpha, beta, titles } = await opened();
    // The board is followed already, through what else is drawn.
    titles.of(beta.id);
    await settle();
    titles.of(beta.id);
    await settle();
    const stale = (await backend.references.resolve(board.workspace.info?.id ?? '', [alpha.id]))[0];
    let answer: () => void = () => undefined;
    vi.spyOn(backend.references, 'resolve').mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          answer = () => {
            resolve(stale ? [stale] : []);
          };
        }),
    );
    titles.of(alpha.id);
    await settle();
    await other.patchCard(board.boardId, alpha.id, { title: 'Renamed' });
    await settle();
    answer();
    await settle();
    expect(titles.of(alpha.id)?.title).toBe('Renamed');
  });

  it('ask again what is held of a workspace that is entered again, and follow none of the one left', async () => {
    const { backend, workspace, alpha, titles } = await opened();
    const watch = backend.watchContainer.bind(backend);
    const stops: ReturnType<typeof vi.fn>[] = [];
    vi.spyOn(backend, 'watchContainer').mockImplementation((containerId, listener) => {
      const stop = vi.fn(watch(containerId, listener));
      stops.push(stop);
      return stop;
    });
    titles.of(alpha.id);
    await settle();
    titles.of(alpha.id);
    await settle();
    expect(stops).toHaveLength(1);
    const info = workspace.info;
    if (!info) throw new Error('no workspace');
    workspace.info = { ...info, id: 'elsewhere' };
    titles.of(alpha.id);
    await settle();
    expect(stops[0]).toHaveBeenCalledOnce();
    const asked = vi.spyOn(backend.references, 'resolve');
    workspace.info = info;
    titles.of(alpha.id);
    await settle();
    expect(asked.mock.calls).toContainEqual([info.id, [alpha.id]]);
  });

  it('follow a page that is renamed, and pages that go with the one deleted', async () => {
    const { backend, workspace, titles } = await opened();
    const workspaceId = workspace.info?.id ?? '';
    const wiki = await backend.wiki.createWiki(workspaceId, {
      title: 'Handbook',
      visibility: 'private',
    });
    const top = await backend.wiki.createPage(wiki.id, { title: 'Top' });
    const below = await backend.wiki.createPage(wiki.id, { title: 'Below', parentId: top.id });
    titles.of(top.id);
    titles.of(below.id);
    await settle();
    // Drawn with the answer: the wiki is followed from then on.
    expect(titles.of(top.id)).toMatchObject({ kind: 'page', title: 'Top', containerKind: 'wiki' });
    titles.of(below.id);
    await settle();

    await backend.wiki.patchPage(wiki.id, top.id, { title: 'Renamed' });
    await settle();
    expect(titles.of(top.id)?.title).toBe('Renamed');

    await backend.wiki.deletePage(wiki.id, top.id);
    await settle();
    expect([titles.of(top.id), titles.of(below.id)]).toEqual([null, null]);
  });

  it('offer what the server changed last before anything is typed where there is no board', async () => {
    const { backend, titles } = await opened();
    const asked = vi.spyOn(backend.references, 'search');
    vi.useFakeTimers();
    try {
      const found = titles.search('');
      await vi.runAllTimersAsync();
      expect((await found).length).toBeGreaterThan(0);
      expect(asked).toHaveBeenCalledOnce();
      expect(asked.mock.calls[0]?.[1]).toBe('');
    } finally {
      vi.useRealTimers();
    }
  });

  it('say whether the reader may see what an identifier names', async () => {
    const { alpha, titles } = await opened();
    expect(await titles.seen(alpha.id)).toBe(true);
    expect(await titles.seen(UNKNOWN)).toBe(false);
  });

  it('offer the cards of the board changed last before anything is typed, and ask once the typing rests', async () => {
    const { backend, board, titles } = await opened();
    const asked = vi.spyOn(backend.references, 'search');
    const unasked = await titles.search('', board);
    expect(unasked.map((linked) => linked.title).sort()).toEqual(['Alpha', 'Beta', 'Gamma']);
    expect(asked).not.toHaveBeenCalled();

    let found: string | undefined;
    vi.useFakeTimers();
    try {
      const first = titles.search('a', board);
      const second = titles.search('al', board);
      await vi.runAllTimersAsync();
      expect((await second).map((linked) => linked.title)).toEqual(['Alpha']);
      found = (await second)[0]?.id;
      // The earlier term is answered as the last one typed.
      expect(await first).toEqual(await second);
      expect(asked).toHaveBeenCalledOnce();
      expect(asked.mock.calls[0]?.[1]).toBe('al');
    } finally {
      vi.useRealTimers();
    }
    // What was offered is known without being asked again, typed for or not.
    const resolved = vi.spyOn(backend.references, 'resolve');
    expect(titles.of(found ?? '')?.title).toBe('Alpha');
    for (const linked of unasked) expect(titles.of(linked.id)?.title).toBe(linked.title);
    expect(resolved).not.toHaveBeenCalled();
  });

  it('tell what names a card, and hold what was told', async () => {
    const { backend, board, alpha, beta, titles } = await opened();
    const now = await backend.card(board.boardId, beta.id);
    await backend.writeCardBody(board.boardId, beta.id, {
      body: {
        type: 'doc',
        content: [{ type: 'paragraph', content: [{ type: 'reference', attrs: { id: alpha.id } }] }],
      },
      readAt: now.updatedAt,
    });
    const naming = await titles.to(alpha.id);
    expect(naming.map((linked) => linked.title)).toEqual(['Beta']);
    const resolved = vi.spyOn(backend.references, 'resolve');
    expect(titles.of(beta.id)?.title).toBe('Beta');
    expect(resolved).not.toHaveBeenCalled();
  });
});
