/**
 * The stores: what a screen reads is sorted here, a change shows at once and
 * is taken back when it is refused, undo takes it back on purpose, and what
 * other people do arrives by itself.
 */
import { describe, expect, it, vi } from 'vitest';

import { BackendError, duplicateView } from '$lib/state/index.ts';
import { refusedMember, type ChangeListener } from '$lib/state/backend.ts';
import { droppedCardOrder, placeAmongAll } from '$lib/state/card-order.ts';
import { sidebarOf } from '$lib/state/order.ts';
import { reread } from '$lib/state/write.ts';

import { loadTemplates, openFirstBoard, settle, setup, STATUS } from './support.ts';

const refused = (): BackendError => new BackendError(403, { detail: 'no' });

describe('the workspace', () => {
  it('reads the reader, the people, the boards and the sidebar', async () => {
    const { workspace } = setup(await loadTemplates());
    await workspace.load();
    expect(workspace.status).toBe('ready');
    expect(workspace.me?.username).toBe('hana');
    expect(workspace.userName(workspace.me?.id ?? '')).toBe('김하나 (@hana)');
    expect(workspace.containers).toHaveLength(8);
    expect(workspace.sidebar.map((entry) => entry.category.name)).toEqual([
      'Projects',
      'Operations',
      'Personal',
      'Boards',
    ]);
    expect(workspace.sidebar[0]?.containers.map((board) => board.title)).toEqual([
      'Project schedule',
      'Issue tracker',
      'Meeting notes',
    ]);
  });

  it('orders the sidebar itself, whatever order the lists arrived in', () => {
    const category = (id: string, sortOrder: number, boardIds: string[], system = false) => ({
      id,
      name: id,
      userId: 'u',
      workspaceId: 't',
      type: system ? ('system' as const) : ('custom' as const),
      collapsed: false,
      sortOrder,
      containerIds: boardIds,
      hiddenContainerIds: [],
      createdAt: 0,
      updatedAt: 0,
    });
    const board = (id: string, title: string) =>
      ({ id, title, createdAt: 0 }) as Parameters<typeof sidebarOf>[1][number];
    const boards = [
      board('b3', 'Zebra'),
      board('b1', 'One'),
      board('b4', 'apple'),
      board('b2', 'Two'),
    ];
    const forwards = sidebarOf(
      [category('second', 10, ['b2', 'gone'], true), category('first', 0, ['b1'])],
      boards,
      'en',
    );
    const backwards = sidebarOf(
      [category('first', 0, ['b1']), category('second', 10, ['b2', 'gone'], true)],
      [...boards].reverse(),
      'en',
    );
    const shape = (sidebar: typeof forwards) =>
      sidebar.map((entry) => [entry.category.id, entry.containers.map((each) => each.id)]);
    expect(shape(forwards)).toEqual([
      ['first', ['b1']],
      // Boards no category names go to the default one, by title; a board that is gone is skipped.
      ['second', ['b2', 'b4', 'b3']],
    ]);
    expect(shape(backwards)).toEqual(shape(forwards));
  });

  it('applies a setting at once and takes it back when the server refuses it', async () => {
    const { workspace, backend } = setup();
    await workspace.load();
    const changing = workspace.changeSettings({ theme: 'dark' });
    expect(workspace.settings.theme).toBe('dark');
    await changing;
    expect(workspace.settings.theme).toBe('dark');

    backend.failNext(
      'patchSettings',
      new BackendError(400, { errors: [refusedMember('timeZone', 'unknown time zone')] }),
    );
    const failing = workspace.changeSettings({ timeZone: 'Asia/Seoul' });
    expect(workspace.settings.timeZone).toBe('Asia/Seoul');
    await failing;
    expect(workspace.settings.timeZone).toBe('auto');
    expect(workspace.problems.at(-1)).toMatchObject({
      action: 'save',
      error: { errors: [{ pointer: '/timeZone' }] },
    });
  });

  it('is signed out, with nothing to tell, when the backend says the session is over', async () => {
    const { workspace, backend } = setup();
    backend.failNext('me', new BackendError(401, { detail: 'no session' }));
    await workspace.load();
    expect(workspace.status).toBe('signedOut');
    expect(workspace.problems).toEqual([]);

    await workspace.load();
    expect(workspace.status).toBe('ready');
    backend.failNext('patchSettings', new BackendError(401, { detail: 'session over' }));
    await workspace.changeSettings({ theme: 'dark' });
    expect(workspace.status).toBe('signedOut');
    expect(workspace.problems).toEqual([]);
  });

  it('stops listening when the session is over, and listens again once read afresh', async () => {
    const { workspace, backend } = setup();
    const stopped = vi.fn();
    const listen = backend.watchSelf.bind(backend);
    const watchSelf = vi.spyOn(backend, 'watchSelf').mockImplementation((listener) => {
      const stop = listen(listener);
      return () => {
        stopped();
        stop();
      };
    });
    await workspace.load();
    const first = workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (!first) throw new Error('the seed has no board');
    const board = await workspace.openBoard(first.id);
    const stopBoard = vi.spyOn(board, 'close');

    workspace.report('save', new BackendError(401, { detail: 'session over' }));
    expect(workspace.status).toBe('signedOut');
    expect(stopped).toHaveBeenCalledOnce();
    expect(stopBoard).toHaveBeenCalled();
    expect(workspace.board).toBeUndefined();

    await workspace.load();
    expect(workspace.status).toBe('ready');
    expect(watchSelf).toHaveBeenCalledTimes(2);
  });

  it('reads the names of people the list of the workspace did not bring, once', async () => {
    const { workspace } = setup();
    await workspace.load();
    const everyone = workspace.users.map((user) => user.id);
    workspace.users = workspace.users.slice(0, 1);
    await workspace.learnUsers([...everyone, ...everyone]);
    expect(workspace.users.map((user) => user.id).sort()).toEqual([...everyone].sort());
  });

  it('learns the names it finds and leaves the others unknown', async () => {
    const { workspace } = setup();
    await workspace.load();
    const [known] = workspace.users.map((user) => user.id);
    if (!known) throw new Error('unexpected seed');
    workspace.users = [];
    await workspace.learnUsers([known, 'nobody-of-this-workspace']);
    expect(workspace.users.map((user) => user.id)).toEqual([known]);
    expect(workspace.userName('nobody-of-this-workspace')).toBeUndefined();
  });

  it('files a board under another category and keeps the order the reader gave', async () => {
    const { workspace } = setup(await loadTemplates());
    await workspace.load();
    const [first, operations] = workspace.sidebar;
    const moved = first?.containers[0];
    if (!moved || !operations) throw new Error('unexpected seed');
    await workspace.moveContainer(moved.id, operations.category.id, 1);
    expect(workspace.sidebar[1]?.containers.map((board) => board.title)[1]).toBe(
      'Project schedule',
    );
    expect(workspace.sidebar[0]?.containers).toHaveLength(2);
  });

  it('hides a board from the sidebar, and undo shows it again in its place', async () => {
    const { workspace, backend } = setup(await loadTemplates());
    await workspace.load();
    const [first] = workspace.sidebar;
    const hidden = first?.containers[1];
    if (!first || !hidden) throw new Error('unexpected seed');
    const before = first.containers.map((board) => board.id);
    await workspace.hideContainer(hidden.id);
    expect(workspace.sidebar[0]?.containers.map((board) => board.id)).toEqual(
      before.filter((id) => id !== hidden.id),
    );
    const stored = await backend.categories(workspace.info?.id ?? '');
    expect(stored.find((one) => one.id === first.category.id)?.hiddenContainerIds).toEqual([
      hidden.id,
    ]);
    await workspace.undoLast();
    expect(workspace.sidebar[0]?.containers.map((board) => board.id)).toEqual(before);
  });

  it('lists the hidden boards, and shows one in the sidebar again', async () => {
    const { workspace } = setup(await loadTemplates());
    await workspace.load();
    const hidden = workspace.sidebar[0]?.containers[0];
    if (!hidden) throw new Error('unexpected seed');
    expect(workspace.hiddenContainers).toEqual([]);
    await workspace.hideContainer(hidden.id);
    expect(workspace.hiddenContainers.map((board) => board.id)).toEqual([hidden.id]);
    await workspace.unhideContainer(hidden.id);
    expect(workspace.hiddenContainers).toEqual([]);
    expect(workspace.sidebar[0]?.containers[0]?.id).toBe(hidden.id);
  });

  it('keeps a board hidden when its category is deleted', async () => {
    const { workspace } = setup(await loadTemplates());
    await workspace.load();
    const [first] = workspace.sidebar;
    const hidden = first?.containers[0];
    if (!first || !hidden) throw new Error('unexpected seed');
    await workspace.hideContainer(hidden.id);
    await workspace.deleteCategory(first.category.id);
    const shown = workspace.sidebar.flatMap((entry) => entry.containers.map((board) => board.id));
    expect(shown).not.toContain(hidden.id);
    expect(workspace.hiddenContainers.map((board) => board.id)).toEqual([hidden.id]);
    await settle();
    expect(workspace.sidebar.flatMap((entry) => entry.containers.map((board) => board.id))).toEqual(
      shown,
    );
  });

  it('shows a new category first at once', async () => {
    const { workspace } = setup(await loadTemplates());
    await workspace.load();
    await workspace.createCategory('Zebra');
    expect(workspace.sidebar[0]?.category.name).toBe('Zebra');
  });

  it('reads the sidebar again when a write of it is refused', async () => {
    const { workspace, backend } = setup(await loadTemplates());
    await workspace.load();
    const reading = vi.spyOn(backend, 'categories');
    backend.failNext('patchCategory', refused());
    await workspace.renameCategory(workspace.sidebar[0]?.category.id ?? '', 'Renamed');
    await settle();
    expect(workspace.sidebar[0]?.category.name).toBe('Projects');
    expect(reading).toHaveBeenCalled();
  });

  it('shows a hidden board again when hiding it is refused', async () => {
    const { workspace, backend } = setup(await loadTemplates());
    await workspace.load();
    const shown = workspace.sidebar[0]?.containers.map((board) => board.id) ?? [];
    backend.failNext('setContainerHidden', refused());
    await workspace.hideContainer(shown[0] ?? '');
    expect(workspace.sidebar[0]?.containers.map((board) => board.id)).toEqual(shown);
    expect(workspace.problems).toHaveLength(1);
  });
});

describe('making and deleting boards', () => {
  it('makes a board with a first view, a kanban grouped by its first select property', async () => {
    const { workspace, backend } = setup();
    await workspace.load();
    const made = await workspace.createBoard({ title: 'Fresh' }, 'Board view');
    expect(workspace.containers.map((board) => board.title)).toContain('Fresh');
    // A board nobody filed yet shows in the default category.
    expect(workspace.sidebar.at(-1)?.containers.map((board) => board.title)).toContain('Fresh');
    const views = await backend.views(made?.id ?? '');
    expect(views).toHaveLength(1);
    expect(views[0]).toMatchObject({ title: 'Board view', fields: { viewType: 'board' } });
    const status = made?.cardProperties.find((template) => template.type === 'select');
    expect(views[0]?.fields.groupBy).toBe(status?.id);
    expect(views[0]?.fields.visibleGroups.at(-1)).toBe('');
    const board = await workspace.openBoard(made?.id ?? '');
    expect(board.role).toBe('admin');
  });

  it('deletes a board, and undo brings it back with its cards', async () => {
    const { workspace, backend } = setup();
    await workspace.load();
    const [first] = workspace.containers;
    await workspace.deleteContainer(first?.id ?? '');
    expect(workspace.containers).toHaveLength(0);
    await workspace.undoLast();
    expect(workspace.containers).toHaveLength(1);
    expect(await backend.cards(first?.id ?? '')).toHaveLength(3);
  });

  it('brings a deleted board back to its place in its category, and the open board with it', async () => {
    const { workspace } = setup(await loadTemplates());
    await workspace.load();
    const [first] = workspace.sidebar;
    const gone = first?.containers[1];
    if (!first || !gone) throw new Error('unexpected seed');
    const place = first.containers.map((board) => board.id);
    const open = await workspace.openBoard(gone.id);
    await workspace.deleteContainer(gone.id);
    await settle();
    expect(open.missing).toBe(true);
    expect(workspace.categories.some((category) => category.containerIds.includes(gone.id))).toBe(
      false,
    );
    await workspace.undoLast();
    await settle();
    expect(workspace.sidebar[0]?.containers.map((board) => board.id)).toEqual(place);
    await vi.waitFor(() => {
      expect(open.status).toBe('ready');
    });
    expect(open.missing).toBe(false);
    expect(await workspace.openBoard(gone.id)).toBe(open);
  });

  it('names the property of a new board in the words it is given', async () => {
    const { workspace } = setup();
    await workspace.load();
    const made = await workspace.createBoard({ title: 'Neu' }, 'Board view', 'Zustand');
    expect(made?.cardProperties.map((template) => template.name)).toEqual(['Zustand']);
  });
});

describe('a board', () => {
  it('reads its parts and says what the reader may do', async () => {
    const { board } = await openFirstBoard();
    expect(board.status).toBe('ready');
    expect(board.board?.title).toBe('Small');
    expect(board.orderedViews.map((view) => view.title)).toEqual(['Board']);
    expect(board.realCards).toHaveLength(3);
    expect(board.role).toBe('admin');
    expect(board.canEdit).toBe(true);
  });

  it('marks a board that is not there as missing, not as a failure to tell about', async () => {
    const { workspace } = setup();
    await workspace.load();
    const board = await workspace.openBoard('nope');
    expect(board.missing).toBe(true);
    expect(workspace.problems).toHaveLength(0);
  });

  it('knows a board the reader may only look at', async () => {
    const made = setup(await loadTemplates());
    await made.workspace.load();
    const readOnly = made.workspace.containers.find((board) => board.title === 'Study notes');
    const board = await made.workspace.openBoard(readOnly?.id ?? '');
    expect(board.role).toBe('viewer');
    expect(board.canEdit).toBe(false);
    expect(board.canComment).toBe(false);
  });

  it("shows the cards of a view in the view's order, however the list arrived", async () => {
    const { board } = await openFirstBoard();
    const view = board.orderedViews[0];
    if (!view) throw new Error('no view');
    expect(board.cardsOf(view).map((card) => card.title)).toEqual(['Alpha', 'Beta', 'Gamma']);
    expect(board.cardsOf(view, 'gam').map((card) => card.title)).toEqual(['Gamma']);
    const groups = board.groupsOf(view, board.cardsOf(view), (name) => `No ${name}`);
    expect(groups.visible.map((group) => [group.option.label, group.cards.length])).toEqual([
      ['To do', 2],
      ['Doing', 1],
      ['Done', 0],
      ['No Status', 0],
    ]);
  });
});

describe('changing a card', () => {
  it('shows the change before the backend has answered, then keeps the answer', async () => {
    const { board, backend } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no card');
    backend.latency = 5;
    const changing = board.setCardProperty(alpha.id, STATUS, 'done');
    expect(board.card(alpha.id)?.fields.properties[STATUS]).toBe('done');
    await changing;
    expect(board.card(alpha.id)?.fields.properties[STATUS]).toBe('done');
    expect(board.card(alpha.id)?.updatedAt).toBeGreaterThan(alpha.updatedAt);
    expect((await backend.card(board.boardId, alpha.id)).fields.properties[STATUS]).toBe('done');
  });

  it('takes the change back and tells the reader when the backend refuses it', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no card');
    backend.failNext('patchCard', refused());
    const changing = board.setCardTitle(alpha.id, 'Renamed');
    expect(board.card(alpha.id)?.title).toBe('Renamed');
    await changing;
    expect(board.card(alpha.id)?.title).toBe('Alpha');
    expect(workspace.problems.at(-1)).toMatchObject({ action: 'save', error: { status: 403 } });
    expect(workspace.canUndo).toBe(false);
  });

  it('keeps the later of two quick changes on screen while the first is answered', async () => {
    const { board, backend } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no card');
    backend.latency = 5;
    const first = board.setCardTitle(alpha.id, 'One');
    const second = board.setCardProperty(alpha.id, STATUS, 'doing');
    const seen: string[] = [];
    const look = (): void => {
      const card = board.card(alpha.id);
      seen.push(`${card?.title}/${JSON.stringify(card?.fields.properties[STATUS])}`);
    };
    look();
    await first;
    look();
    await second;
    look();
    expect(seen).toEqual(['One/"doing"', 'One/"doing"', 'One/"doing"']);
  });

  it('sends nothing for a change that changes nothing', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    backend.failNext('patchCard', refused());
    await board.setCardTitle(alpha?.id ?? '', 'Alpha');
    expect(workspace.problems).toHaveLength(0);
  });

  it('undoes and redoes', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    if (!alpha) throw new Error('no card');
    await board.setCardTitle(alpha.id, 'Renamed');
    expect(workspace.canUndo).toBe(true);
    await workspace.undoLast();
    expect(board.card(alpha.id)?.title).toBe('Alpha');
    expect((await backend.card(board.boardId, alpha.id)).title).toBe('Alpha');
    expect(workspace.canRedo).toBe(true);
    await workspace.redoLast();
    expect((await backend.card(board.boardId, alpha.id)).title).toBe('Renamed');
  });

  it('creates a card, and undo deletes exactly that one', async () => {
    const { board, workspace } = await openFirstBoard();
    const made = await board.createCard({ title: 'New', properties: { [STATUS]: 'done' } });
    expect(board.realCards.map((card) => card.title)).toContain('New');
    await workspace.undoLast();
    expect(board.card(made?.id)).toBeUndefined();
    await workspace.redoLast();
    expect(board.realCards.filter((card) => card.title === 'New')).toHaveLength(1);
  });

  it('deletes cards, and one undo brings all of them back with their comments', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const ids = board.realCards.filter((card) => card.title !== 'Gamma').map((card) => card.id);
    const alpha = board.realCards.find((card) => card.title === 'Alpha');
    await board.deleteCards(ids);
    expect(board.realCards.map((card) => card.title)).toEqual(['Gamma']);
    await workspace.undoLast();
    expect(board.realCards).toHaveLength(3);
    expect(await backend.comments(board.boardId, alpha?.id ?? '')).toHaveLength(2);
  });
});

describe('dropping cards', () => {
  it('places dropped cards before the card at the place, or after the last of the column', () => {
    const shown = ['a', 'b', 'c', 'd', 'e'];
    expect(droppedCardOrder(shown, ['a', 'b'], ['e'], 0)).toEqual(['e', 'a', 'b', 'c', 'd']);
    expect(droppedCardOrder(shown, ['a', 'b'], ['e'], 1)).toEqual(['a', 'e', 'b', 'c', 'd']);
    expect(droppedCardOrder(shown, ['a', 'b'], ['e'], 2)).toEqual(['a', 'b', 'e', 'c', 'd']);
    expect(droppedCardOrder(shown, ['a', 'b'], ['e'], 99)).toEqual(['a', 'b', 'e', 'c', 'd']);
    expect(droppedCardOrder(shown, [], ['b', 'd'], 0)).toEqual(['a', 'c', 'e', 'b', 'd']);
    expect(droppedCardOrder(shown, ['a', 'c'], ['b'], 1)).toEqual(shown);
  });

  describe('a place among the shown cards, as a place among all of the column', () => {
    it('is the same place when every card of the column is shown', () => {
      const column = ['a', 'b', 'c'];
      for (let index = 0; index <= 3; index += 1) {
        expect(placeAmongAll(column, column, index)).toBe(index);
      }
    });

    it('is right after the shown card above, past the cards a filter hides before it', () => {
      // `b` is hidden: the cards shown are a and c.
      expect(placeAmongAll(['a', 'b', 'c'], ['a', 'c'], 1)).toBe(1);
      // After c, which is the third of the column.
      expect(placeAmongAll(['a', 'b', 'c'], ['a', 'c'], 2)).toBe(3);
      // `a` is hidden: after b, the second.
      expect(placeAmongAll(['a', 'b', 'c'], ['b', 'c'], 1)).toBe(2);
    });

    it('is right before the first shown card when the drop is at the top', () => {
      // `a` is hidden and stays first.
      expect(placeAmongAll(['a', 'b', 'c'], ['b', 'c'], 0)).toBe(1);
    });

    it('skips a shown card the column does not have', () => {
      // `x` is shown in the group (its value is one no option has) but is not counted there.
      expect(placeAmongAll(['a', 'b'], ['a', 'x', 'b'], 2)).toBe(1);
      expect(placeAmongAll(['a', 'b'], ['x', 'a', 'b'], 1)).toBe(0);
      expect(placeAmongAll(['x'], ['stale'], 0)).toBe(1);
    });

    it('is the end of the column when none of it is shown: a folded or filtered-out group', () => {
      expect(placeAmongAll(['a', 'b'], [], 0)).toBe(2);
      expect(placeAmongAll([], [], 0)).toBe(0);
    });

    it('takes a place beyond the shown cards for the end', () => {
      expect(placeAmongAll(['a', 'b'], ['a', 'b'], 99)).toBe(2);
      expect(placeAmongAll(['x', 'a', 'y'], ['a'], Number.POSITIVE_INFINITY)).toBe(2);
    });
  });

  it('counts the cards the page hides when it is told which ones the page shows', async () => {
    const { board } = await openFirstBoard();
    const view = board.orderedViews[0];
    const ids = Object.fromEntries(board.realCards.map((card) => [card.title, card.id]));
    if (!view) throw new Error('no view');
    // The page shows Beta only (a search hides Alpha, which stands before it):
    // Gamma dropped under Beta lands right after Beta, not after the first card.
    await board.dropCards({
      viewId: view.id,
      cardIds: [ids.Gamma ?? ''],
      optionId: 'todo',
      index: 1,
      shown: [ids.Beta ?? ''],
    });
    const current = board.view(view.id);
    if (!current) throw new Error('no view');
    const groups = board.groupsOf(current, board.cardsOf(current), () => 'none');
    expect(groups.visible[0]?.cards.map((card) => card.title)).toEqual(['Alpha', 'Beta', 'Gamma']);
  });

  it('puts a card dropped above the only shown one right before it', async () => {
    const { board } = await openFirstBoard();
    const view = board.orderedViews[0];
    const ids = Object.fromEntries(board.realCards.map((card) => [card.title, card.id]));
    if (!view) throw new Error('no view');
    await board.dropCards({
      viewId: view.id,
      cardIds: [ids.Gamma ?? ''],
      optionId: 'todo',
      index: 0,
      shown: [ids.Beta ?? ''],
    });
    const current = board.view(view.id);
    if (!current) throw new Error('no view');
    const groups = board.groupsOf(current, board.cardsOf(current), () => 'none');
    expect(groups.visible[0]?.cards.map((card) => card.title)).toEqual(['Alpha', 'Gamma', 'Beta']);
  });

  it('moves a card to another column at a place, and one undo takes both changes back', async () => {
    const { board, workspace } = await openFirstBoard();
    const view = board.orderedViews[0];
    const gamma = board.realCards.find((card) => card.title === 'Gamma');
    if (!view || !gamma) throw new Error('unexpected seed');
    await board.dropCards({ viewId: view.id, cardIds: [gamma.id], optionId: 'todo', index: 1 });
    const current = board.view(view.id);
    if (!current) throw new Error('no view');
    const groups = board.groupsOf(current, board.cardsOf(current), () => 'none');
    expect(groups.visible[0]?.cards.map((card) => card.title)).toEqual(['Alpha', 'Gamma', 'Beta']);
    await workspace.undoLast();
    const back = board.view(view.id);
    if (!back) throw new Error('no view');
    const undone = board.groupsOf(back, board.cardsOf(back), () => 'none');
    expect(undone.visible.map((group) => group.cards.map((card) => card.title))).toEqual([
      ['Alpha', 'Beta'],
      ['Gamma'],
      [],
      [],
    ]);
  });

  it('clears the value of a card dropped in the column of cards without one', async () => {
    const { board } = await openFirstBoard();
    const view = board.orderedViews[0];
    const beta = board.realCards.find((card) => card.title === 'Beta');
    if (!view || !beta) throw new Error('unexpected seed');
    await board.dropCards({ viewId: view.id, cardIds: [beta.id], optionId: '' });
    expect(board.card(beta.id)?.fields.properties).not.toHaveProperty(STATUS);
  });
});

describe('changing views and the board', () => {
  it('changes a view at once, reverts a refused change, and undoes', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    const view = board.orderedViews[0];
    if (!view) throw new Error('no view');
    await board.setViewFields(view.id, {
      hiddenGroups: ['done'],
      visibleGroups: ['todo', 'doing', ''],
    });
    expect(board.view(view.id)?.fields.hiddenGroups).toEqual(['done']);
    backend.failNext('patchView', refused());
    await board.changeView(view.id, (current) => ({ ...current, title: 'Renamed' }));
    expect(board.view(view.id)?.title).toBe('Board');
    await workspace.undoLast();
    expect(board.view(view.id)?.fields.hiddenGroups).toEqual([]);
  });

  it('creates, copies and deletes views', async () => {
    const { board, workspace } = await openFirstBoard();
    const view = board.orderedViews[0];
    if (!view) throw new Error('no view');
    await duplicateView(board, view.id, 'A copy');
    // Right after its view, not by its title.
    expect(board.orderedViews.map((each) => each.title)).toEqual(['Board', 'A copy']);
    await board.deleteView(view.id);
    expect(board.orderedViews.map((each) => each.title)).toEqual(['A copy']);
    await workspace.undoLast();
    expect(board.orderedViews.map((each) => each.title)).toEqual(['Board', 'A copy']);
  });

  it('changes the definition of a property', async () => {
    const { board, backend, workspace } = await openFirstBoard();
    await board.changeBoard((current) => ({
      ...current,
      cardProperties: current.cardProperties.map((template) =>
        template.id === STATUS
          ? {
              ...template,
              options: [...template.options, { id: 'later', label: 'Later', color: 'gray' }],
            }
          : template,
      ),
    }));
    const stored = await backend.container(board.boardId);
    expect(stored.cardProperties[0]?.options.map((option) => option.id)).toContain('later');
    await workspace.undoLast();
    expect(board.board?.cardProperties[0]?.options).toHaveLength(3);
  });
});

describe('what other people do', () => {
  it('shows up on the board by itself', async () => {
    const { board, other } = await openFirstBoard();
    const beta = board.realCards.find((card) => card.title === 'Beta');
    if (!beta) throw new Error('no card');
    await other.patchCard(board.boardId, beta.id, { title: 'Beta, by someone else' });
    await other.createCard(board.boardId, { title: 'Theirs' });
    await settle();
    expect(board.card(beta.id)?.title).toBe('Beta, by someone else');
    expect(board.realCards.map((card) => card.title)).toContain('Theirs');
    await other.batchDeleteCards(board.boardId, [beta.id]);
    await settle();
    expect(board.card(beta.id)).toBeUndefined();
  });

  it('shows a board renamed by someone else in the sidebar too', async () => {
    const { board, other, workspace } = await openFirstBoard();
    await other.patchContainer(board.boardId, { title: 'Renamed elsewhere' });
    await settle();
    expect(board.board?.title).toBe('Renamed elsewhere');
    expect(workspace.sidebar.flatMap((entry) => entry.containers)[0]?.title).toBe(
      'Renamed elsewhere',
    );
  });

  it('stops following a board that was closed', async () => {
    const { board, other, workspace } = await openFirstBoard();
    const beta = board.realCards.find((card) => card.title === 'Beta');
    workspace.closeBoard();
    await other.patchCard(board.boardId, beta?.id ?? '', { title: 'unseen' });
    await settle();
    expect(board.card(beta?.id)?.title).toBe('Beta');
  });
});

describe('reading again after changes may have been missed', () => {
  it('takes what was missed into an open board, and keeps it on screen meanwhile', async () => {
    const made = setup();
    const listeners: ChangeListener[] = [];
    const watch = made.backend.watchContainer.bind(made.backend);
    vi.spyOn(made.backend, 'watchContainer').mockImplementation((containerId, listener) => {
      listeners.push(listener);
      return watch(containerId, listener);
    });
    const { world, board } = await openFirstBoard(made);
    const card = board.cards.find((each) => each.title === 'Alpha');
    const gone = board.cards.find((each) => each.title === 'Gamma');
    if (!card || !gone) throw new Error('the seed changed');

    // Changed and deleted behind the store's back: no message came.
    world.blocks.set(card.id, {
      ...world.blocks.get(card.id),
      title: 'Changed while away',
      updatedAt: card.updatedAt + 1000,
    } as never);
    world.blocks.delete(gone.id);
    for (const listener of listeners) listener({ kind: 'resync' });
    expect(board.status).toBe('ready');
    await settle();
    await vi.waitFor(() => {
      expect(board.card(card.id)?.title).toBe('Changed while away');
    });
    expect(board.card(gone.id)).toBeUndefined();
  });
});

describe('a list read again', () => {
  const item = (id: string, updatedAt: number) => ({ id, updatedAt });

  it('takes what was read, keeps what is newer on screen or held by a write, and what came meanwhile', () => {
    // 'e' was shown, then deleted while the list was read: the older reading still has it.
    const shown = [item('a', 5), item('b', 1), item('c', 1), item('d', 1), item('new', 1)];
    const before = ['a', 'b', 'c', 'd', 'e'];
    const read = [item('a', 3), item('b', 2), item('c', 2), item('e', 1)];
    const next = reread(shown, read, before, (one) => one.id !== 'c');
    expect(next).toEqual([item('a', 5), item('b', 2), item('c', 1), item('new', 1)]);
  });
});

describe('an open board the reader is no member of', () => {
  it('opens as its members have it, and the first write joins it before it goes', async () => {
    const made = setup();
    await made.workspace.load();
    const first = made.workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (!first) throw new Error('the seed has no board');
    const stored = made.world.containers.get(first.id);
    if (!stored) throw new Error('no stored board');
    made.world.containers.set(first.id, { ...stored, visibility: 'open' });
    const readerId = (await made.backend.me()).id;
    made.world.members.set(
      first.id,
      (made.world.members.get(first.id) ?? []).filter((member) => member.userId !== readerId),
    );

    const board = await made.workspace.openBoard(first.id);
    expect(board.status).toBe('ready');
    expect(board.joinable).toBe(true);
    expect(board.joinsOnWrite).toBe(true);
    expect(board.canEdit).toBe(true);
    expect(board.missing).toBe(false);
    expect(board.board?.title).toBe(stored.title);
    expect(board.realCards.length).toBeGreaterThan(0);
    expect(made.workspace.problems).toHaveLength(0);

    // Two writes at once join it once, and both go.
    const [one, two] = await Promise.all([
      board.createCard({ title: 'Mine' }),
      board.createCard({ title: 'Mine too' }),
    ]);
    expect([one?.title, two?.title]).toEqual(['Mine', 'Mine too']);
    expect(made.workspace.problems).toHaveLength(0);
    expect(board.joinable).toBe(false);
    expect(board.canEdit).toBe(true);
    expect(board.status).toBe('ready');
    const joined = (made.world.members.get(first.id) ?? []).filter(
      (member) => member.userId === readerId,
    );
    expect(joined.map((member) => member.role)).toEqual(['editor']);
  });

  it('whose least role only views is read only, and joined by its banner', async () => {
    const made = setup();
    await made.workspace.load();
    const first = made.workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (!first) throw new Error('the seed has no board');
    const stored = made.world.containers.get(first.id);
    if (!stored) throw new Error('no stored board');
    made.world.containers.set(first.id, { ...stored, visibility: 'open', minimumRole: 'viewer' });
    const readerId = (await made.backend.me()).id;
    made.world.members.set(
      first.id,
      (made.world.members.get(first.id) ?? []).filter((member) => member.userId !== readerId),
    );
    const board = await made.workspace.openBoard(first.id);
    expect([board.joinable, board.joinsOnWrite, board.canEdit]).toEqual([true, false, false]);
    expect(await board.join()).toBe(true);
    expect([board.joinable, board.role]).toEqual([false, 'viewer']);
  });

  it('becomes joinable again when the reader is taken off it while it is open', async () => {
    const made = setup();
    const listeners: ChangeListener[] = [];
    const watch = made.backend.watchContainer.bind(made.backend);
    vi.spyOn(made.backend, 'watchContainer').mockImplementation((containerId, listener) => {
      listeners.push(listener);
      return watch(containerId, listener);
    });
    await made.workspace.load();
    const first = made.workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (!first) throw new Error('the seed has no board');
    const stored = made.world.containers.get(first.id);
    if (!stored) throw new Error('no stored board');
    made.world.containers.set(first.id, { ...stored, visibility: 'open' });
    const board = await made.workspace.openBoard(first.id);
    expect(board.status).toBe('ready');
    expect(board.joinable).toBe(false);

    const readerId = (await made.backend.me()).id;
    made.world.members.set(
      first.id,
      (made.world.members.get(first.id) ?? []).filter((member) => member.userId !== readerId),
    );
    for (const listener of [...listeners]) {
      listener({
        kind: 'members',
        containerId: first.id,
        members: made.world.members.get(first.id) ?? [],
      });
    }
    await vi.waitFor(() => {
      expect(board.joinable).toBe(true);
      // Its next write joins it again.
      expect(board.joinsOnWrite).toBe(true);
    });
    expect(board.missing).toBe(false);
  });

  it('is gone when joining finds it no longer open, and is not read once left', async () => {
    const made = setup();
    await made.workspace.load();
    const first = made.workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (!first) throw new Error('the seed has no board');
    const stored = made.world.containers.get(first.id);
    if (!stored) throw new Error('no stored board');
    made.world.containers.set(first.id, { ...stored, visibility: 'open' });
    const readerId = (await made.backend.me()).id;
    made.world.members.set(
      first.id,
      (made.world.members.get(first.id) ?? []).filter((member) => member.userId !== readerId),
    );
    const board = await made.workspace.openBoard(first.id);
    expect(board.joinable).toBe(true);
    made.backend.failNext('joinContainer', new BackendError(404));
    expect(await board.join()).toBe(false);
    expect(board).toMatchObject({ missing: true, joining: false });

    const again = await made.workspace.openBoard(first.id);
    const load = vi.spyOn(again, 'load');
    made.backend.latency = 5;
    const joining = again.join();
    again.close();
    expect(await joining).toBe(false);
    expect(load).not.toHaveBeenCalled();
  });

  it('is not offered to join when the board refused is a private one', async () => {
    const made = setup();
    await made.workspace.load();
    const first = made.workspace.sidebar.flatMap((entry) => entry.containers)[0];
    if (!first) throw new Error('the seed has no board');
    made.backend.failNext('views', new BackendError(403));
    const board = await made.workspace.openBoard(first.id);
    expect(board.joinable).toBe(false);
    expect(board.status).toBe('failed');
    expect(made.workspace.problems.at(-1)).toMatchObject({ action: 'load' });
  });
});

describe('sharing a board', () => {
  it('adds a member, changes their role and takes them off, as the backend keeps it', async () => {
    const { board, workspace, backend } = await openFirstBoard();
    const outsider = [...(await backend.users(workspace.info?.id ?? ''))].find(
      (user) => !board.members.some((member) => member.userId === user.id),
    );
    if (!outsider) throw new Error('unexpected seed');
    expect(board.canShare).toBe(true);
    await board.addMember(outsider.id, 'commenter');
    expect(board.members.find((member) => member.userId === outsider.id)?.role).toBe('commenter');
    await board.setMemberRole(outsider.id, 'viewer');
    const stored = await backend.members(board.boardId);
    expect(stored.find((member) => member.userId === outsider.id)?.role).toBe('viewer');
    await board.removeMember(outsider.id);
    expect((await backend.members(board.boardId)).map((member) => member.userId)).not.toContain(
      outsider.id,
    );
    expect(board.members.map((member) => member.userId)).not.toContain(outsider.id);
  });

  it('puts the last admin back when the backend refuses to demote them, and reads the members again', async () => {
    const { board, workspace } = await openFirstBoard();
    const me = workspace.me?.id ?? '';
    await board.setMemberRole(me, 'editor');
    expect(board.role).toBe('admin');
    expect(workspace.problems).toHaveLength(1);
    expect(workspace.problems[0]?.error).toMatchObject({
      reason: 'last_admin',
      type: '/problems/last-admin',
    });
  });

  it('opens the board to the workspace with a minimum role', async () => {
    const { board, backend } = await openFirstBoard();
    await board.setOpen(true);
    await board.setMinimumRole('commenter');
    expect(await backend.container(board.boardId)).toMatchObject({
      visibility: 'open',
      minimumRole: 'commenter',
    });
  });

  it('finds people of the workspace by name and learns them', async () => {
    const { workspace } = setup(await loadTemplates());
    await workspace.load();
    const someone = workspace.users.find((user) => user.id !== workspace.me?.id);
    if (!someone) throw new Error('unexpected seed');
    const found = await workspace.searchUsers(someone.username.slice(1, 4).toUpperCase());
    expect(found.map((user) => user.id)).toContain(someone.id);
    // The workspace's list without a search stays what it was.
    expect(workspace.workspaceUsers).toEqual(workspace.users);
  });
});

describe('templates', () => {
  it('makes a template from a board, offers it, and makes a board from it', async () => {
    const { workspace, backend } = setup(await loadTemplates());
    await workspace.load();
    const source = workspace.sidebar[0]?.containers[0];
    if (!source) throw new Error('unexpected seed');
    const template = await workspace.createTemplateFromBoard(source.id);
    expect(template).toMatchObject({ isTemplate: true, title: 'New board template' });
    // No board of the sidebar.
    expect(workspace.containers.map((board) => board.id)).not.toContain(template?.id);
    expect((await workspace.templates()).map((one) => one.id)).toContain(template?.id);

    const made = await workspace.duplicateContainer(template?.id ?? '');
    expect(made).toMatchObject({ isTemplate: false, title: 'New board template' });
    expect(workspace.containers.map((board) => board.id)).toContain(made?.id);
    const [sourceCards, madeCards] = await Promise.all([
      backend.cards(source.id),
      backend.cards(made?.id ?? ''),
    ]);
    expect(madeCards.map((card) => card.title).sort()).toEqual(
      sourceCards.map((card) => card.title).sort(),
    );
    expect(madeCards.some((card) => sourceCards.some((other) => other.id === card.id))).toBe(false);
    const [sourceViews, madeViews] = await Promise.all([
      backend.views(source.id),
      backend.views(made?.id ?? ''),
    ]);
    expect(madeViews.length).toBe(sourceViews.length);
    // A view's order of cards names the copies, as the server renames it.
    const ordered = madeViews.flatMap((view) => view.fields.cardOrder);
    expect(ordered.every((id) => madeCards.some((card) => card.id === id))).toBe(true);
  });
});

describe('archives', () => {
  it('exports a board with its progress, and its import makes another board like it', async () => {
    const { workspace, backend } = setup(await loadTemplates());
    await workspace.load();
    const source = workspace.sidebar[0]?.containers[0];
    if (!source) throw new Error('unexpected seed');
    const archive = await workspace.exportContainer(source.id);
    expect(archive?.name).toMatch(/^archive-\d{4}-\d{2}-\d{2}\.fvoci$/);
    expect(workspace.transfers.list).toMatchObject([{ kind: 'export', finished: true }]);
    const before = workspace.containers.length;
    const imported = await workspace.importArchive(
      new File([archive?.bytes ?? ''], archive?.name ?? 'a'),
    );
    expect(imported).toMatchObject({
      format: 'fvoci',
      membersLeftOut: 0,
      imported: { boards: 1, wikis: 0 },
    });
    expect(imported?.containerIds).toHaveLength(1);
    await settle();
    expect(workspace.containers).toHaveLength(before + 1);
    const copy = workspace.containers.find(
      (board) => board.title === source.title && board.id !== source.id,
    );
    expect((await backend.cards(copy?.id ?? '')).length).toBe(
      (await backend.cards(source.id)).length,
    );
  });

  it('names the container of a block in its lines `containerId`, matching the contract', async () => {
    const { workspace } = setup(await loadTemplates());
    await workspace.load();
    const source = workspace.sidebar[0]?.containers[0];
    if (!source) throw new Error('unexpected seed');
    const archive = await workspace.exportContainer(source.id);
    const lines = (await (archive?.bytes ?? new Blob()).text())
      .split('\n')
      .filter((line) => line !== '')
      .map((line) => JSON.parse(line) as { type?: string; data?: Record<string, unknown> });
    const blocks = lines.filter((line) => line.type === 'block');
    expect(blocks.length).toBeGreaterThan(0);
    for (const { data } of blocks) {
      expect(data).toMatchObject({ containerId: source.id });
      expect(data).not.toHaveProperty('boardId');
    }
  });

  it('moves one thing once at a time, and gives up a transfer that is dismissed, telling nothing', async () => {
    const { workspace, backend } = setup(await loadTemplates());
    await workspace.load();
    const board = workspace.sidebar[0]?.containers[0];
    if (!board) throw new Error('unexpected seed');
    backend.latency = 20;
    const first = workspace.exportWorkspace();
    const again = workspace.exportWorkspace();
    expect(workspace.transfers.list).toHaveLength(1);
    expect(await again).toBeUndefined();
    workspace.transfers.dismiss(workspace.transfers.list[0]?.id ?? 0);
    expect(await first).toBeUndefined();
    expect(workspace.transfers.list).toEqual([]);
    expect(workspace.problems).toEqual([]);
    backend.latency = 0;
    expect(await workspace.exportContainer(board.id)).toBeDefined();
  });

  it('tells a file that is no archive, and makes nothing', async () => {
    const { workspace } = setup(await loadTemplates());
    await workspace.load();
    const before = workspace.containers.length;
    expect(await workspace.importArchive(new File(['hello'], 'notes.txt'))).toBeUndefined();
    expect(workspace.problems).toMatchObject([{ action: 'import', error: { status: 400 } }]);
    expect(workspace.transfers.list).toEqual([]);
    await settle();
    expect(workspace.containers).toHaveLength(before);
  });
});

describe('searching boards', () => {
  it('finds the boards with every word in the title, whatever the order and case, hidden ones too', async () => {
    const { workspace } = setup(await loadTemplates());
    await workspace.load();
    const target = workspace.containers.find((board) => board.title.includes(' '));
    if (!target) throw new Error('unexpected seed');
    const words = target.title.toUpperCase().split(' ').reverse().join(' ');
    await workspace.hideContainer(target.id);
    const found = await workspace.searchBoards(words);
    expect(found.map((board) => board.id)).toContain(target.id);
    expect(found.every((board) => !board.isTemplate)).toBe(true);
    expect(await workspace.searchBoards('   ')).toEqual([]);
  });
});

describe('the sidebar across the steps', () => {
  it('hides a board brought back from the trash in the category it came back to, and it stays hidden', async () => {
    const { workspace, backend } = setup(await loadTemplates());
    await workspace.load();
    const gone = workspace.sidebar[0]?.containers[0];
    const categoryId = workspace.sidebar[0]?.category.id;
    if (!gone) throw new Error('unexpected seed');
    await workspace.deleteContainer(gone.id);
    await workspace.undoLast();
    await settle();
    await workspace.hideContainer(gone.id);
    await settle();
    const shown = workspace.sidebar.flatMap((entry) => entry.containers.map((board) => board.id));
    expect(shown).not.toContain(gone.id);
    const stored = await backend.categories(workspace.info?.id ?? '');
    const home = stored.find((category) => category.id === categoryId);
    expect(home?.containerIds).toContain(gone.id);
    expect(home?.hiddenContainerIds).toContain(gone.id);
  });

  it('does not show a reading of the sidebar over a write under way, and reads again after it', async () => {
    const { workspace, backend } = setup(await loadTemplates());
    await workspace.load();
    const [first, research] = workspace.sidebar;
    const moved = first?.containers[0];
    if (!moved || !research) throw new Error('unexpected seed');
    backend.latency = 30;
    const moving = workspace.moveContainer(moved.id, research.category.id, 0);
    // A message has the sidebar read while the move is on its way.
    backend.world.tellUsers([workspace.me?.id ?? ''], { kind: 'resync' });
    await new Promise((resolve) => setTimeout(resolve, 70));
    expect(workspace.sidebar[1]?.containers[0]?.id).toBe(moved.id);
    await moving;
    backend.latency = 0;
    await vi.waitFor(() => {
      expect(workspace.sidebar[1]?.containers[0]?.id).toBe(moved.id);
    });
  });

  it('files a new board under the default category, in memory as on the server', async () => {
    const { workspace, backend } = setup(await loadTemplates());
    await workspace.load();
    const made = await workspace.createBoard({ title: 'Fresh' });
    const stored = await backend.categories(workspace.info?.id ?? '');
    expect(stored.find((category) => category.type === 'system')?.containerIds).toContain(made?.id);
  });
});
