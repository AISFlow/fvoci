/**
 * An open card: its body in its room, its comments and attachments, and who
 * may change and delete a comment.
 */
import { describe, expect, it, vi } from 'vitest';
import * as Y from 'yjs';

import { BackendError } from '$lib/state/index.ts';
import { bodyOf, FIELD, roomsOf } from '$lib/state/memory/rooms.ts';

import { openFirstBoard, settle, text } from './support.ts';

/** Alpha opened, once `before` has had the board (the room is asked for as the card is opened). */
async function openAlpha(before?: (made: Awaited<ReturnType<typeof openFirstBoard>>) => void) {
  const made = await openFirstBoard();
  const alpha = made.board.realCards.find((card) => card.title === 'Alpha');
  if (!alpha) throw new Error('no card');
  before?.(made);
  const card = await made.board.showCard(alpha.id);
  return { ...made, card, cardId: alpha.id };
}

/**
 * Alpha open with its body in its room, and a way to have the reader taken
 * off the board; each state of the room is put on `statuses` as it comes.
 */
async function inTheRoom(statuses: string[] = []) {
  const made = await openAlpha(({ backend }) => {
    const openDoc = backend.openDoc.bind(backend);
    vi.spyOn(backend, 'openDoc').mockImplementation((workspaceId, boardId, cardId, hears) =>
      openDoc(workspaceId, boardId, cardId, {
        ...hears,
        state: (state) => {
          statuses.push(state.status);
          hears.state(state);
        },
      }),
    );
  });
  const { card, board, backend, world } = made;
  card.openBody();
  await vi.waitFor(() => {
    expect(card.room?.status).toBe('joined');
  });
  const me = board.workspace.me?.id ?? '';
  const otherId = [...world.users.keys()].find((id) => id !== me) ?? '';
  // A member already keeps their role when added: it is set after.
  await backend.addMember(board.boardId, otherId, 'admin');
  await backend.setMemberRole(board.boardId, otherId, 'admin');
  const takenOff = () => made.other.removeMember(board.boardId, me);
  return { ...made, takenOff };
}

describe('an open card', () => {
  it('reads the body, the comments and the attachments, oldest first', async () => {
    const { card } = await openAlpha();
    expect(card.status).toBe('ready');
    expect(card.body).toEqual(text('first body'));
    expect(card.comments).toHaveLength(2);
    expect(card.comments[0]?.createdAt).toBeLessThan(card.comments[1]?.createdAt ?? 0);
    expect(card.attachments.map((attachment) => attachment.name)).toEqual(['kick-off-notes.txt']);
    expect(card.summary?.title).toBe('Alpha');
  });

  it('is missing when the card is not there', async () => {
    const { board } = await openFirstBoard();
    const card = await board.showCard('nope');
    expect(card.missing).toBe(true);
  });

  it("takes somebody else's body while it is not in the body's room", async () => {
    const { card, other, board, cardId } = await openAlpha();
    const read = await other.card(board.boardId, cardId);
    await other.writeCardBody(board.boardId, cardId, {
      body: text('news'),
      readAt: read.updatedAt,
    });
    await settle();
    await settle();
    expect(card.body).toEqual(text('news'));
  });

  it("opens the body in its room, where the room's document wins over the card's body, and leaves it when closed", async () => {
    const { card, other, board, cardId, world } = await openAlpha();
    card.openBody();
    await vi.waitFor(() => {
      expect(card.room?.status).toBe('joined');
    });
    expect(card.room?.writable).toBe(true);
    expect(roomsOf(world).members(cardId)).toBe(1);
    // A change of the card heard meanwhile does not read the body: the room's is newer.
    const read = vi.spyOn(board.workspace.backend, 'card');
    await other.patchCard(board.boardId, cardId, { title: 'Renamed' });
    await settle();
    expect(read).not.toHaveBeenCalled();
    card.close();
    expect(card.room).toBeUndefined();
    await vi.waitFor(() => {
      expect(roomsOf(world).members(cardId)).toBe(0);
    });
  });

  it("puts nothing of the body on the app's undo line: the body's undo is the editor's", async () => {
    const { card, board, cardId, world } = await openAlpha();
    card.openBody();
    await vi.waitFor(() => {
      expect(card.room?.status).toBe('joined');
    });
    const doc = card.room?.doc;
    if (!doc) throw new Error('no document');
    const paragraph = new Y.XmlElement('paragraph');
    paragraph.insert(0, [new Y.XmlText('typed')]);
    doc.getXmlFragment(FIELD).insert(0, [paragraph]);
    await vi.waitFor(() => {
      expect(card.bodyPending).toBe(false);
    });
    roomsOf(world).flush();
    await settle();
    expect(board.workspace.canUndo).toBe(false);
    // Written into the card, and still nothing to undo.
    expect(JSON.stringify(world.blocks.get(cardId)?.body)).toContain('typed');
    card.close();
  });

  it('says when what the reader wrote was dropped with a document made anew', async () => {
    const { card, other, board, cardId, backend } = await openAlpha();
    card.openBody();
    await vi.waitFor(() => {
      expect(card.room?.status).toBe('joined');
    });
    const doc = card.room?.doc;
    if (!doc) throw new Error('no document');
    // The reader's change is on its way when the body is written whole elsewhere.
    backend.latency = 50;
    const paragraph = new Y.XmlElement('paragraph');
    paragraph.insert(0, [new Y.XmlText('mine')]);
    doc.getXmlFragment(FIELD).insert(0, [paragraph]);
    expect(card.bodyPending).toBe(true);
    const read = await other.card(board.boardId, cardId);
    await other.writeCardBody(board.boardId, cardId, {
      body: text('theirs'),
      readAt: read.updatedAt,
    });
    await vi.waitFor(() => {
      expect(card.room?.doc).not.toBe(doc);
      expect(card.room?.status).toBe('joined');
    });
    expect(card.lost).toEqual(['document_reset']);
    const currentRoom = card.room as { doc?: Y.Doc } | undefined;
    expect(bodyOf(currentRoom?.doc ?? doc)).toEqual(text('theirs'));
    card.dismissLost();
    expect(card.lost).toEqual([]);
    card.close();
  });

  it('is missing when its room lets the reader go and the card is no longer theirs to read', async () => {
    const { card, backend, takenOff } = await inTheRoom();
    // Read again, the card is not there for the reader any more, as the server answers.
    const read = vi.spyOn(backend, 'card');
    // The board is read again: its views are read with it, and by nothing else here.
    const boardRead = vi.spyOn(backend, 'views');
    await takenOff();
    await vi.waitFor(() => {
      expect(card.missing).toBe(true);
    });
    expect(card.room?.reason).toBe('not_found');
    await settle();
    // Once for the room put out, however often it says so; and the board is read again.
    expect(read).toHaveBeenCalledTimes(1);
    expect(boardRead).toHaveBeenCalledTimes(1);
    card.close();
  });

  it('asks its room once more when the room lets the reader go but the card can still be read', async () => {
    const statuses: string[] = [];
    const { card, board, cardId, backend, takenOff } = await inTheRoom(statuses);
    // A card that is still read is not called gone; its room, which refuses them now, is not asked again.
    const detail = await backend.card(board.boardId, cardId);
    const read = vi.spyOn(backend, 'card').mockResolvedValue(detail);
    await takenOff();
    const sincePutOut = () => statuses.slice(statuses.indexOf('out'));
    await vi.waitFor(() => {
      expect(sincePutOut()).toEqual(['out', 'joining', 'out']);
    });
    await settle();
    expect(sincePutOut()).toEqual(['out', 'joining', 'out']);
    expect(read).toHaveBeenCalledOnce();
    expect(card.missing).toBe(false);
    expect(card.room?.reason).toBe('not_found');
    card.close();
  });

  it("joins its room again when the card is the reader's again before the room told them", async () => {
    const { card, board, cardId, backend, other, takenOff } = await inTheRoom();
    const me = board.workspace.me?.id ?? '';
    const detail = await backend.card(board.boardId, cardId);
    // Restored (here: let back onto the board) between the room letting them go and the card read.
    const read = vi.spyOn(backend, 'card').mockImplementationOnce(async () => {
      await other.addMember(board.boardId, me, 'editor');
      return detail;
    });
    await takenOff();
    await vi.waitFor(() => {
      expect(read).toHaveBeenCalledOnce();
      expect(card.room?.status).toBe('joined');
    });
    expect(card.missing).toBe(false);
    expect(card.room?.writable).toBe(true);
    card.close();
  });

  it('tells a session that ended when its room lets the reader go', async () => {
    const { card, board, backend, takenOff } = await inTheRoom();
    const ended = new BackendError(401);
    vi.spyOn(backend, 'card').mockRejectedValue(ended);
    const report = vi.spyOn(board.workspace, 'report');
    await takenOff();
    await vi.waitFor(() => {
      expect(report).toHaveBeenCalledWith('load', ended);
    });
    expect(card.missing).toBe(false);
    card.close();
  });

  it('takes no answer about a room it left meanwhile', async () => {
    const { card, backend, takenOff } = await inTheRoom();
    let answer: (error: unknown) => void = () => undefined;
    const read = vi.spyOn(backend, 'card').mockReturnValue(
      new Promise((_, reject) => {
        answer = reject;
      }),
    );
    await takenOff();
    await vi.waitFor(() => {
      expect(read).toHaveBeenCalled();
    });
    // The body is left (the card was deleted and its window made anew) before the answer.
    card.closeBody();
    answer(new BackendError(404));
    await settle();
    expect(card.missing).toBe(false);
    card.close();
  });
});

describe('comments and attachments', () => {
  it("adds a comment, sees somebody else's, and brings a deleted one back with undo", async () => {
    const { card, other, board, workspace, cardId } = await openAlpha();
    expect(await card.addComment(text('mine'))).toBe(true);
    await other.createComment(board.boardId, cardId, text('theirs'));
    await settle();
    expect(card.comments).toHaveLength(4);
    const mine = card.comments.find((comment) => JSON.stringify(comment.body).includes('mine'));
    await card.deleteComment(mine?.id ?? '');
    expect(card.comments).toHaveLength(3);
    await workspace.undoLast();
    expect(card.comments).toHaveLength(4);
  });

  it('changes the text of a comment from the version it was begun on, and refuses to write over a newer one', async () => {
    const { card, other, board, world } = await openAlpha();
    // Somebody else changes the reader's comment: only an admin may.
    world.members.set(
      board.boardId,
      (world.members.get(board.boardId) ?? []).map((member) =>
        member.userId === other.userId ? { ...member, role: 'admin' } : member,
      ),
    );
    expect(await card.addComment(text('mine'))).toBe(true);
    const mine = card.comments.at(-1);
    if (!mine) throw new Error('no comment');
    expect(await card.editComment(mine.id, text('mine, changed'), mine.updatedAt)).toBe('saved');
    expect(card.comment(mine.id)?.body).toEqual(text('mine, changed'));

    // The reader begins on this version; somebody else changes it, and that is heard.
    const begun = card.comment(mine.id)?.updatedAt ?? 0;
    await other.patchComment(board.boardId, mine.id, { body: text('theirs'), readAt: begun });
    await settle();
    expect(card.comment(mine.id)?.body).toEqual(text('theirs'));
    // Saved on the version begun from: refused, theirs stays.
    const refresh = vi.spyOn(card, 'refresh');
    expect(await card.editComment(mine.id, text('again'), begun)).toBe('conflict');
    expect(refresh).toHaveBeenCalledOnce();
    expect(card.comment(mine.id)?.body).toEqual(text('theirs'));
    // On the version shown now it goes.
    const now = card.comment(mine.id)?.updatedAt ?? 0;
    expect(await card.editComment(mine.id, text('again'), now)).toBe('saved');
    expect(card.comment(mine.id)?.body).toEqual(text('again'));
  });

  it('tells a comment change that failed for another reason', async () => {
    const { card, backend, workspace } = await openAlpha();
    const first = card.comments[0];
    if (!first) throw new Error('no comment');
    backend.failNext('patchComment', new BackendError(500));
    expect(await card.editComment(first.id, text('x'), first.updatedAt)).toBe('failed');
    expect(workspace.problems.at(-1)).toMatchObject({ action: 'save', error: { status: 500 } });
  });

  it('uploads a file, shows its progress in the stack and in place, and attaches it', async () => {
    const { card, backend, workspace } = await openAlpha();
    backend.latency = 2;
    const attaching = card.attach(new File(['hello'], 'hello.txt', { type: 'text/plain' }));
    expect(card.uploads.map((upload) => upload.name)).toEqual(['hello.txt']);
    expect(workspace.transfers.list).toMatchObject([
      { kind: 'upload', name: 'hello.txt', total: 5, finished: false, card: card.cardId },
    ]);
    await attaching;
    // In: an attachment, no more an upload in place; the stack keeps it a moment, done.
    expect(card.uploads).toHaveLength(0);
    expect(workspace.transfers.list).toMatchObject([
      { name: 'hello.txt', done: 5, finished: true },
    ]);
    expect(card.attachments.map((attachment) => attachment.name)).toEqual([
      'kick-off-notes.txt',
      'hello.txt',
    ]);
    expect(card.fileUrl(card.attachments[1]?.fileId ?? '')).toMatch(/^blob:/);
  });

  it('uploads an image for the body out of the list, shown in the stack', async () => {
    const { card, backend, workspace } = await openAlpha();
    backend.latency = 5;
    const uploading = card.uploadFile(new File(['png'], 'a.png', { type: 'image/png' }));
    expect(card.uploads).toHaveLength(0);
    expect(card.filesGoingUp).toBe(true);
    expect(workspace.transfers.list).toMatchObject([{ kind: 'upload', name: 'a.png' }]);
    expect(await uploading).toMatch(/./);
    expect(card.filesGoingUp).toBe(false);
    expect(workspace.transfers.list).toMatchObject([{ name: 'a.png', finished: true }]);
  });

  it('gives up the uploads for the body when the card is closed: each stays in the stack, left', async () => {
    const { card, backend, workspace } = await openAlpha();
    backend.latency = 5;
    const told = workspace.problems.length;
    const uploading = card.uploadFile(new File(['png'], 'a.png', { type: 'image/png' }));
    const other = card.uploadFile(new File(['%PDF'], 'b.pdf'));
    card.close();
    await expect(uploading).rejects.toThrow('a.png did not go up');
    await expect(other).rejects.toThrow('b.pdf did not go up');
    expect(workspace.transfers.list.map(({ name, failure }) => [name, failure])).toEqual([
      ['a.png', 'left'],
      ['b.pdf', 'left'],
    ]);
    expect(workspace.transfers.list.every((one) => one.retry === undefined)).toBe(true);
    // Told in the stack, and not again where problems are.
    expect(workspace.problems).toHaveLength(told);
  });

  it('goes on with an attachment when its card is closed, and the card open then takes it', async () => {
    const { card, backend, workspace, board, cardId } = await openAlpha();
    backend.latency = 20;
    const attaching = card.attach(new File(['hello'], 'later.txt'));
    board.closeCard();
    const again = await board.showCard(cardId);
    // The card shown again shows the same upload in its place.
    expect(again.uploads.map((upload) => upload.name)).toEqual(['later.txt']);
    await attaching;
    expect(workspace.transfers.list).toMatchObject([{ name: 'later.txt', finished: true }]);
    expect(again.attachments.map((attachment) => attachment.name)).toContain('later.txt');
    expect((await backend.attachments(board.boardId, cardId)).map((one) => one.name)).toContain(
      'later.txt',
    );
  });

  it('tells nothing of files when a card with none going up is closed', async () => {
    const { card, backend, workspace } = await openAlpha();
    backend.latency = 5;
    await card.uploadFile(new File(['png'], 'a.png', { type: 'image/png' }));
    const told = workspace.problems.length;
    card.close();
    expect(workspace.problems).toHaveLength(told);
    expect(workspace.transfers.list.some((one) => one.failure)).toBe(false);
  });

  it('keeps a failed upload, with why, until it is dismissed; one sent again is attached', async () => {
    const { card, backend, workspace } = await openAlpha();
    const told = workspace.problems.length;
    backend.failNext('upload', new BackendError(413, { limit: 10 }));
    await card.attach(new File(['hello'], 'big.bin'));
    expect(card.uploads[0]).toMatchObject({ name: 'big.bin', failure: 'too-large', limit: 10 });
    expect(workspace.problems).toHaveLength(told);
    workspace.transfers.dismiss(card.uploads[0]?.id ?? 0);
    expect(card.uploads).toHaveLength(0);
    expect(workspace.transfers.list).toEqual([]);

    backend.failNext('upload', new BackendError(0, { detail: 'no answer' }));
    await card.attach(new File(['hello'], 'again.txt'));
    const failed = card.uploads[0];
    expect(failed).toMatchObject({ name: 'again.txt', failure: 'network' });
    failed?.retry?.();
    await vi.waitFor(() => {
      expect(card.attachments.map((attachment) => attachment.name)).toContain('again.txt');
    });
    expect(card.uploads).toHaveLength(0);
  });

  it('attaches nothing of an upload given up', async () => {
    const { card, backend, workspace } = await openAlpha();
    backend.latency = 5;
    const attaching = card.attach(new File(['hello'], 'stopped.bin'));
    workspace.transfers.dismiss(card.uploads[0]?.id ?? 0);
    await attaching;
    expect(card.uploads).toHaveLength(0);
    expect(workspace.transfers.list).toEqual([]);
    expect(card.attachments).toHaveLength(1);
  });

  it('keeps an attachment given up as it was made, when taking it out fails, and says so in the stack', async () => {
    const { card, backend, workspace, board } = await openAlpha();
    const create = backend.createAttachment.bind(backend);
    // Given up in the gap between the attachment made and its answer.
    vi.spyOn(backend, 'createAttachment').mockImplementationOnce(async (...args) => {
      const made = await create(...args);
      workspace.transfers.dismiss(card.uploads[0]?.id ?? 0);
      return made;
    });
    backend.failNext('deleteBlock', new BackendError(500, { detail: 'down' }));
    const told = workspace.problems.length;
    await card.attach(new File(['hello'], 'late.txt'));
    expect(workspace.transfers.list).toMatchObject([{ name: 'late.txt', failure: 'kept' }]);
    expect(workspace.transfers.said.at(-1)?.transfer).toMatchObject({ failure: 'kept' });
    // It is attached, on the server and on the card, and no more an upload in place.
    expect(card.attachments.map((attachment) => attachment.name)).toContain('late.txt');
    expect(card.uploads).toHaveLength(0);
    expect(
      (await backend.attachments(board.boardId, card.cardId)).map((one) => one.name),
    ).toContain('late.txt');
    expect(workspace.problems).toHaveLength(told);
  });

  it('tells nothing of an attachment given up as it was made that is gone already', async () => {
    const { card, backend, workspace } = await openAlpha();
    const create = backend.createAttachment.bind(backend);
    vi.spyOn(backend, 'createAttachment').mockImplementationOnce(async (...args) => {
      const made = await create(...args);
      workspace.transfers.dismiss(card.uploads[0]?.id ?? 0);
      return made;
    });
    backend.failNext('deleteBlock', new BackendError(404, { detail: 'gone' }));
    const said = workspace.transfers.said.length;
    await card.attach(new File(['hello'], 'gone.txt'));
    expect(workspace.transfers.list).toEqual([]);
    // Its start was said, and nothing after.
    expect(workspace.transfers.said).toHaveLength(said + 1);
  });

  it('closes when its card is deleted by somebody else', async () => {
    const { card, other, board, cardId } = await openAlpha();
    await other.deleteBlock(board.boardId, cardId);
    await settle();
    expect(card.missing).toBe(true);
  });
});

describe('deleted and brought back', () => {
  it('an open card deleted by somebody else is missing, and is there again once restored', async () => {
    const { card, other, board, cardId } = await openAlpha();
    await other.deleteBlock(board.boardId, cardId);
    await settle();
    expect(card.missing).toBe(true);
    await other.restoreBlock(board.boardId, cardId);
    await settle();
    await vi.waitFor(() => {
      expect(card.missing).toBe(false);
    });
    expect(card.summary?.title).toBe('Alpha');
  });

  it('restores deleted cards atomically without rereading all board cards, and keeps failed undo retryable', async () => {
    const { board, workspace, backend, cardId } = await openAlpha();
    const cardsSpy = vi.spyOn(backend, 'cards');
    await board.deleteCards([cardId]);
    vi.spyOn(backend, 'batchRestoreCards').mockRejectedValueOnce(
      new BackendError(500, { detail: 'down' }),
    );
    await workspace.undoLast();
    expect(workspace.problems).toHaveLength(1);
    expect(workspace.undo.canUndo).toBe(true);
    await workspace.undoLast();
    expect(workspace.undo.canUndo).toBe(false);
    expect(cardsSpy).not.toHaveBeenCalled();
    expect(board.card(cardId)?.title).toBe('Alpha');
  });

  it('undo of a card made goes on when somebody else deleted it first', async () => {
    const { board, workspace, other } = await openAlpha();
    const made = await board.createCard({ title: 'Brief', fields: {} } as never);
    if (!made) throw new Error('not made');
    await other.deleteBlock(board.boardId, made.id);
    await settle();
    await workspace.undoLast();
    expect(workspace.problems).toHaveLength(0);
    expect(board.card(made.id)).toBeUndefined();
  });

  it('shows a deleted comment again on card.refresh() when it was restored while offline', async () => {
    const { card, backend, board, cardId } = await openAlpha();
    const body = {
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Restored note' }] }],
    };
    const created = await backend.createComment(board.boardId, cardId, body);
    card.receive(created);
    expect(card.comments.map((c) => c.id)).toContain(created.id);

    // Simulate comment deletion recorded locally, then restored on the backend without live echo.
    card.forget([created.id]);
    expect(card.comments.map((c) => c.id)).not.toContain(created.id);

    await card.refresh();
    expect(card.comments.map((c) => c.id)).toContain(created.id);
  });
});

describe('who deletes a comment', () => {
  it('an admin anyone’s, an editor or a commenter their own, a viewer none', async () => {
    const { board, workspace } = await openAlpha();
    const me = workspace.me?.id ?? '';
    const as = (role: 'admin' | 'editor' | 'commenter' | 'viewer') => {
      board.members = board.members.map((member) =>
        member.userId === me ? { ...member, role } : member,
      );
    };
    expect(board.role).toBe('admin');
    expect(board.canDeleteComment('someone')).toBe(true);
    as('editor');
    expect([board.canDeleteComment(me), board.canDeleteComment('someone')]).toEqual([true, false]);
    as('commenter');
    expect([board.canDeleteComment(me), board.canDeleteComment('someone')]).toEqual([true, false]);
    // A viewer, and who is no member, none.
    as('viewer');
    expect(board.role).toBe('viewer');
    expect(board.canDeleteComment(me)).toBe(false);
    board.members = board.members.filter((member) => member.userId !== me);
    expect(board.canDeleteComment(me)).toBe(false);
  });
});

describe('who changes a comment', () => {
  it('an admin anyone’s, an editor or a commenter their own, a viewer none', async () => {
    const { board, workspace } = await openAlpha();
    const me = workspace.me?.id ?? '';
    const as = (role: 'admin' | 'editor' | 'commenter' | 'viewer') => {
      board.members = board.members.map((member) =>
        member.userId === me ? { ...member, role } : member,
      );
      return [board.canEditComment(me), board.canEditComment('someone')];
    };
    expect(as('admin')).toEqual([true, true]);
    expect(as('editor')).toEqual([true, false]);
    expect(as('commenter')).toEqual([true, false]);
    expect(as('viewer')).toEqual([false, false]);
    // Any member reads what comments said before; whoever is none (a reader by the link) does not.
    expect(board.canReadCommentHistory).toBe(true);
    board.members = board.members.filter((member) => member.userId !== me);
    expect(board.canReadCommentHistory).toBe(false);
  });
});
