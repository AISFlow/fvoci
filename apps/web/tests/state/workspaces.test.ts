/**
 * The workspace shown is the one asked for (`Workspace.load`, `enter`): which
 * one is entered when none is named, what a workspace that is not the
 * reader's reads as, and that nothing of the one left stays.
 */
import { describe, expect, it, vi, type MockInstance } from 'vitest';

import { BackendError, whileHere, Workspace, type WorkspaceInfo } from '$lib/state/index.ts';

import { settle, setup, SMALL, type Setup } from './support.ts';

const OTHER: WorkspaceInfo = { id: 'other-workspace', title: 'Other' };

/** The world's workspace and a second one, empty, that the reader also works in. */
function twoWorkspaces(made: Setup = setup()): Setup & {
  first: WorkspaceInfo;
  /** The reading of a workspace's boards, watched. */
  readBoards: MockInstance<Setup['backend']['boards']>;
} {
  const { backend, world } = made;
  const first = world.workspace;
  vi.spyOn(backend, 'workspaces').mockResolvedValue([first, OTHER]);
  // The second workspace is empty; the world's is read as it is.
  const users = backend.users.bind(backend);
  const boards = backend.boards.bind(backend);
  const categories = backend.categories.bind(backend);
  vi.spyOn(backend, 'users').mockImplementation((id) =>
    id === OTHER.id ? Promise.resolve([]) : users(id),
  );
  const readBoards = vi
    .spyOn(backend, 'boards')
    .mockImplementation((id) => (id === OTHER.id ? Promise.resolve([]) : boards(id)));
  vi.spyOn(backend, 'categories').mockImplementation((id) =>
    id === OTHER.id ? Promise.resolve([]) : categories(id),
  );
  const wikis = backend.wiki.wikis.bind(backend.wiki);
  vi.spyOn(backend.wiki, 'wikis').mockImplementation((id) =>
    id === OTHER.id ? Promise.resolve([]) : wikis(id),
  );
  const courses = backend.course.courses.bind(backend.course);
  vi.spyOn(backend.course, 'courses').mockImplementation((id) =>
    id === OTHER.id ? Promise.resolve([]) : courses(id),
  );
  return { ...made, first, readBoards };
}

describe('the workspace entered', () => {
  it('is the first one when none is named and none was kept', async () => {
    const { workspace, first } = twoWorkspaces();
    await workspace.load();
    expect(workspace.status).toBe('ready');
    expect(workspace.entered).toBe('ready');
    expect(workspace.info).toEqual(first);
    expect(workspace.containers.length).toBeGreaterThan(0);
  });

  it('is the one this user was last in when it is still theirs, and the first when it is not', async () => {
    const made = twoWorkspaces();
    const asked: string[] = [];
    let kept = OTHER.id;
    const workspace = new Workspace({
      backend: made.backend,
      locale: () => 'en',
      lastWorkspace: (userId) => {
        asked.push(userId);
        return kept;
      },
    });
    await workspace.load();
    expect(workspace.info).toEqual(OTHER);
    expect(asked).toEqual([made.backend.userId]);

    kept = 'gone';
    expect(workspace.firstChoice()).toBe(made.first.id);
  });

  it('is the one named, and one the reader does not work in is missing, with nothing of it read', async () => {
    const { workspace, readBoards } = twoWorkspaces();
    await workspace.load('not-theirs');
    expect(workspace.status).toBe('ready');
    expect(workspace.entered).toBe('missing');
    expect(workspace.asked).toBe('not-theirs');
    expect(workspace.info).toBeUndefined();
    expect(workspace.containers).toEqual([]);
    expect(readBoards).not.toHaveBeenCalled();
  });

  it('is missing when the backend says it is not there (404), as one not theirs', async () => {
    const { workspace, backend, world } = setup();
    vi.spyOn(backend, 'workspaces').mockResolvedValue([world.workspace, OTHER]);
    await workspace.load(OTHER.id);
    expect(workspace.entered).toBe('missing');
    expect(workspace.info).toBeUndefined();
  });

  it('is none when the reader works in no workspace', async () => {
    const { workspace, backend } = setup();
    vi.spyOn(backend, 'workspaces').mockResolvedValue([]);
    await workspace.load();
    expect(workspace.status).toBe('ready');
    expect(workspace.entered).toBe('none');
    expect(workspace.firstChoice()).toBeUndefined();
  });
});

describe('a load the page leaves for', () => {
  it('reads nothing more once the settings say so, and stays loading', async () => {
    const made = twoWorkspaces();
    const watch = vi.spyOn(made.backend, 'watchSelf');
    const asked: string[] = [];
    await made.workspace.load(undefined, (settings) => {
      asked.push(settings.language);
      return true;
    });
    expect(asked).toEqual(['auto']);
    expect(made.workspace.status).toBe('loading');
    expect(made.workspace.entered).toBe('none');
    expect(made.readBoards).not.toHaveBeenCalled();
    expect(watch).not.toHaveBeenCalled();
    // Where it does not leave, it goes on as a load does.
    await made.workspace.load(undefined, () => false);
    expect(made.workspace.status).toBe('ready');
    expect(made.workspace.info?.id).toBe(made.first.id);
    expect(watch).toHaveBeenCalledTimes(1);
  });
});

describe('entering another workspace', () => {
  it('reads it anew, and lets go of the open board, the sidebar and undo of the one left', async () => {
    const { workspace, backend, first } = twoWorkspaces();
    const entered = vi.fn();
    Object.assign(backend, { enter: entered });
    await workspace.load();
    expect(entered).toHaveBeenLastCalledWith(first.id);
    const board = workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (!board) throw new Error('no board');
    const store = await workspace.openBoard(board.id);
    await workspace.hideContainer(board.id);
    expect(workspace.canUndo).toBe(true);

    await workspace.enter(OTHER.id);
    expect(entered).toHaveBeenLastCalledWith(OTHER.id);
    expect(workspace.info).toEqual(OTHER);
    expect(workspace.entered).toBe('ready');
    expect(workspace.board).toBeUndefined();
    expect(workspace.containers).toEqual([]);
    expect(workspace.categories).toEqual([]);
    expect(workspace.users).toEqual([]);
    expect(workspace.canUndo).toBe(false);
    // The board left hears nothing any more.
    const before = store.board?.title;
    await backend.patchContainer(board.id, { title: 'Changed elsewhere' });
    await settle();
    expect(store.board?.title).toBe(before);

    await workspace.enter(first.id);
    expect(workspace.containers.map((one) => one.id)).toContain(board.id);
  });

  it('does not remember a change of the workspace left that was still under way', async () => {
    const { workspace, backend } = twoWorkspaces();
    await workspace.load();
    const board = workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (!board) throw new Error('no board');
    const store = await workspace.openBoard(board.id);
    const card = store.cards[0];
    if (!card) throw new Error('no card');
    backend.latency = 20;
    const renaming = store.setCardTitle(card.id, 'Renamed before leaving');
    await workspace.enter(OTHER.id);
    await renaming;
    expect(workspace.canUndo).toBe(false);
  });

  it('does not put a category made in the workspace left into the one entered', async () => {
    const { workspace, backend } = twoWorkspaces();
    await workspace.load();
    backend.latency = 20;
    const making = workspace.createCategory('Made before leaving');
    await workspace.enter(OTHER.id);
    await making;
    expect(workspace.categories).toEqual([]);
  });

  it('does not take a refused sidebar write of the workspace left back into the one entered', async () => {
    const { workspace, backend } = twoWorkspaces();
    await workspace.load();
    const category = workspace.categories[0];
    if (!category) throw new Error('no category');
    backend.latency = 20;
    backend.failNext('patchCategory', new BackendError(500));
    const renaming = workspace.renameCategory(category.id, 'Refused');
    await workspace.enter(OTHER.id);
    await renaming;
    expect(workspace.categories).toEqual([]);
  });

  it('says an entry failed, and enters on being asked again', async () => {
    const { workspace, readBoards } = twoWorkspaces();
    await workspace.load();
    readBoards.mockRejectedValueOnce(new BackendError(500));
    await workspace.enter(OTHER.id);
    expect(workspace.entered).toBe('failed');
    expect(workspace.problems).toHaveLength(1);
    await workspace.enter(OTHER.id);
    expect(workspace.entered).toBe('ready');
    expect(workspace.info).toEqual(OTHER);
  });

  it('does not let a late failure of an entry undo a later one of the same workspace', async () => {
    const { workspace, first, readBoards } = twoWorkspaces();
    await workspace.load();
    let fail: (error: unknown) => void = () => undefined;
    readBoards.mockImplementationOnce(
      () =>
        new Promise((_, reject) => {
          fail = reject;
        }),
    );
    const slow = workspace.enter(OTHER.id);
    await workspace.enter(first.id);
    await workspace.enter(OTHER.id);
    fail(new BackendError(500));
    await slow;
    expect(workspace.entered).toBe('ready');
    expect(workspace.info).toEqual(OTHER);
    expect(workspace.problems).toEqual([]);
  });

  it('counts the latest of two entries that overlap', async () => {
    const { workspace, first, readBoards } = twoWorkspaces();
    await workspace.load();
    let answer: (boards: never[]) => void = () => undefined;
    readBoards.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          answer = resolve;
        }),
    );
    const slow = workspace.enter(OTHER.id);
    await workspace.enter(first.id);
    answer([]);
    await slow;
    expect(workspace.info).toEqual(first);
    expect(workspace.containers.length).toBeGreaterThan(0);
  });

  it('does nothing for the workspace it is in already', async () => {
    const { workspace, readBoards } = twoWorkspaces();
    await workspace.load();
    readBoards.mockClear();
    await workspace.enter(workspace.info?.id ?? '');
    expect(readBoards).not.toHaveBeenCalled();
  });
});

describe('a board of another workspace', () => {
  it('is missing when opened from this one, as one that is not there', async () => {
    const { workspace, world } = setup();
    await workspace.load();
    const board = workspace.containers[0];
    if (!board) throw new Error('no board');
    const stored = world.containers.get(board.id);
    if (!stored) throw new Error('not in the world');
    world.containers.set(board.id, { ...stored, workspaceId: OTHER.id });
    const store = await workspace.openBoard(board.id);
    expect(store.missing).toBe(true);
    expect(store.board).toBeUndefined();
    expect(store.cards).toEqual([]);
  });

  it('is not put into the sidebar when its change is heard', async () => {
    const { workspace, backend, world } = setup();
    await workspace.load();
    const board = workspace.containers[0];
    if (!board) throw new Error('no board');
    const stored = world.containers.get(board.id);
    if (!stored) throw new Error('not in the world');
    world.containers.set(board.id, { ...stored, workspaceId: OTHER.id });
    await backend.patchContainer(board.id, { title: 'Moved' });
    await settle();
    expect(workspace.containers.find((one) => one.id === board.id)?.title).not.toBe('Moved');
  });
});

describe('entering another workspace while an undo or redo of the one left is written', () => {
  // Found by a review of web-o3 (its tests/state/workspace-review.test.ts).
  for (const action of ['undo', 'redo'] as const) {
    it(`keeps the new workspace's history empty when the ${action} is answered late`, async () => {
      const { workspace, backend } = twoWorkspaces();
      await workspace.load();
      const board = workspace.sidebar.flatMap((entry) => entry.containers)[0];
      if (!board) throw new Error('no board');
      await workspace.hideContainer(board.id);
      if (action === 'redo') await workspace.undoLast();

      let answer = (): void => undefined;
      const pending = new Promise<void>((resolve) => {
        answer = resolve;
      });
      const writing = vi.spyOn(backend, 'setContainerHidden').mockReturnValueOnce(pending);
      const applying = action === 'undo' ? workspace.undoLast() : workspace.redoLast();
      // Entered once the write has begun: the undo or redo is being carried out, not waiting.
      await vi.waitFor(() => {
        expect(writing).toHaveBeenCalled();
      });
      await workspace.enter(OTHER.id);
      expect(workspace.info).toEqual(OTHER);
      expect(workspace.canUndo).toBe(false);
      expect(workspace.canRedo).toBe(false);

      answer();
      await applying;
      expect(workspace.canUndo).toBe(false);
      expect(workspace.canRedo).toBe(false);
      workspace.close();
    });
  }
});

describe('an answer that would take the reader somewhere', () => {
  it('is handed on while the reader is still in the workspace it was asked in', async () => {
    const { workspace } = twoWorkspaces();
    await workspace.load();
    const made = await whileHere(workspace, () => workspace.createBoard({ title: 'Here' }));
    expect(made?.title).toBe('Here');
  });

  it('is not handed on once the reader entered another workspace; what it made is there', async () => {
    const { workspace, backend, first } = twoWorkspaces();
    await workspace.load();
    backend.latency = 20;
    const making = whileHere(workspace, () => workspace.createBoard({ title: 'Left behind' }));
    await workspace.enter(OTHER.id);
    expect(await making).toBeUndefined();
    backend.latency = 0;
    await workspace.enter(first.id);
    expect(workspace.containers.map((board) => board.title)).toContain('Left behind');
  });

  it('is not handed on to one who went to another workspace and came back', async () => {
    const { workspace, backend, first } = twoWorkspaces();
    await workspace.load();
    backend.latency = 20;
    const making = whileHere(workspace, () => workspace.createBoard({ title: 'Asked before' }));
    await workspace.enter(OTHER.id);
    await workspace.enter(first.id);
    expect(await making).toBeUndefined();
  });

  it('of a board, is not handed on once another board is open', async () => {
    const { workspace, backend } = twoWorkspaces(setup([SMALL, { ...SMALL, key: 'small-2' }]));
    await workspace.load();
    const [one, two] = workspace.sidebar.flatMap((entry) => entry.containers);
    if (!one || !two) throw new Error('the seed has fewer than two boards');
    const store = await workspace.openBoard(one.id);
    backend.latency = 20;
    const making = whileHere(store, () => store.createCard({}));
    await workspace.openBoard(two.id);
    expect(await making).toBeUndefined();
  });

  it('of a board, is handed on while that board is open', async () => {
    const { workspace } = twoWorkspaces();
    await workspace.load();
    const [one] = workspace.sidebar.flatMap((entry) => entry.containers);
    if (!one) throw new Error('the seed has no board');
    const store = await workspace.openBoard(one.id);
    const card = await whileHere(store, () => store.createCard({ title: 'Kept' }));
    expect(card?.title).toBe('Kept');
  });
});
