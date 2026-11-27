/**
 * The versions of a file: the backend in memory keeps them as the server
 * does (the upload is the first, a write is made from the newest and is a
 * conflict otherwise, going back writes a new one, the newest fifty stay);
 * the server backend asks for them; a reader by a public link has none; and
 * the list the viewer shows them from, with the file that did not become a
 * version kept to be attached on its own.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { SharedBackend } from '$lib/api/shared-backend.ts';
import { attachmentVersions, versionRefusal } from '$lib/features/card/file-versions.ts';
import { VersionsList, type VersionsSource } from '$lib/file-viewer/versions.svelte.ts';
import { FILE_VERSION_STALE } from '$lib/state/file-versions.ts';
import { BackendError, type Backend } from '$lib/state/index.ts';
import { LIMITS } from '$lib/state/memory/shapes.ts';

import { openFirstBoard, serverBackend, setup } from './support.ts';

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('the call did not reject');
};

const named = (name: string, text: string): File => new File([text], name, { type: 'text/plain' });

/** The small board with a file of it that the reader uploaded. */
async function boardWithFile() {
  const made = setup();
  const [workspace] = await made.backend.workspaces();
  const [board] = await made.backend.boards(workspace?.id ?? '');
  if (!board) throw new Error('no board');
  const file = await made.backend.upload(board.id, named('paper.txt', 'first'));
  const versions = made.backend.fileVersions;
  const add = async (text: string, base: number, by = made.backend) =>
    by.fileVersions.add(
      board.id,
      file.fileId,
      base,
      await by.upload(board.id, named('p.txt', text)),
    );
  const said = async () =>
    (await versions.list(board.id, file.fileId)).map((one) => [
      one.version,
      one.isCurrent,
      one.restoredFrom,
    ]);
  const textOf = async (version: number) =>
    (await versions.bytes(board.id, file.fileId, version)).text();
  return { ...made, boardId: board.id, file, versions, add, said, textOf };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('the versions of a file in memory', () => {
  it('has the upload as the only version of a file never written anew', async () => {
    const { backend, boardId, file, versions, world } = await boardWithFile();
    expect(await versions.list(boardId, file.fileId)).toEqual([
      {
        version: 1,
        createdAt: world.files.get(file.fileId)?.createdAt,
        createdBy: backend.userId,
        size: 5,
        isCurrent: true,
        restoredFrom: null,
      },
    ]);
  });

  it('makes an uploaded file the new version, which is what the file is read as', async () => {
    const { backend, other, boardId, file, add, said, textOf, world } = await boardWithFile();
    const before = world.files.size;
    const written = await add('the second', 1, other);
    expect(written).toMatchObject({ version: 2, createdBy: other.userId, size: 10 });
    expect(await said()).toEqual([
      [2, true, null],
      [1, false, null],
    ]);
    expect(await (await backend.fileBytes(boardId, file.fileId)).text()).toBe('the second');
    expect(await textOf(1)).toBe('first');
    expect(await backend.fileDetails(boardId, file.fileId)).toEqual({
      name: 'paper.txt',
      size: 10,
    });
    // The upload is no file of its own any more.
    expect(world.files.size).toBe(before);
  });

  it('refuses a version written from one that is the newest no more, and keeps the upload', async () => {
    const { backend, other, boardId, file, add, said, versions } = await boardWithFile();
    await add('theirs', 1, other);
    const mine = await backend.upload(boardId, named('p.txt', 'mine'));
    const refused = await rejection(versions.add(boardId, file.fileId, 1, mine));
    expect([refused.status, refused.reason]).toEqual([409, FILE_VERSION_STALE]);
    expect(await said()).toEqual([
      [2, true, null],
      [1, false, null],
    ]);
    expect(await (await backend.fileBytes(boardId, mine.fileId)).text()).toBe('mine');
    expect((await rejection(versions.restore(boardId, file.fileId, 1, 1))).status).toBe(409);
  });

  it('goes back to a version by writing a new one, and writes nothing for the newest', async () => {
    const { boardId, file, add, said, textOf, versions } = await boardWithFile();
    await add('the second', 1);
    const back = await versions.restore(boardId, file.fileId, 1, 2);
    expect(back).toMatchObject({ version: 3, restoredFrom: 1, isCurrent: true, size: 5 });
    expect(await textOf(3)).toBe('first');
    expect(await versions.restore(boardId, file.fileId, 3, 3)).toMatchObject({ version: 3 });
    expect(await said()).toEqual([
      [3, true, 1],
      [2, false, null],
      [1, false, null],
    ]);
    expect((await rejection(versions.restore(boardId, file.fileId, 9, 3))).status).toBe(404);
  });

  it('takes no file that cannot be a version: itself, another kind, one shown, one of another board', async () => {
    const { backend, boardId, file, versions, world } = await boardWithFile();
    const refusedFor = async (fileId: string) =>
      (await rejection(versions.add(boardId, file.fileId, 1, { ...file, fileId }))).status;
    expect(await refusedFor(file.fileId)).toBe(400);
    expect(await refusedFor('no-such-file')).toBe(400);
    const picture = await backend.upload(boardId, new File(['x'], 'p.png'));
    expect(await refusedFor(picture.fileId)).toBe(400);
    const card = [...world.blocks.values()].find((block) => block.type === 'card');
    const shown = await backend.upload(boardId, named('shown.txt', 'shown'));
    await backend.createAttachment(boardId, card?.id ?? '', shown);
    expect(await refusedFor(shown.fileId)).toBe(400);
    const pictured = await backend.upload(boardId, named('pictured.txt', 'in a body'));
    if (!card) throw new Error('no card');
    card.body = { type: 'doc', content: [{ type: 'image', attrs: { fileId: pictured.fileId } }] };
    expect(await refusedFor(pictured.fileId)).toBe(400);
    const written = await backend.upload(boardId, named('w.txt', 'w'));
    await versions.add(
      boardId,
      written.fileId,
      1,
      await backend.upload(boardId, named('w.txt', 'x')),
    );
    expect(await refusedFor(written.fileId)).toBe(400);
    expect((await versions.list(boardId, file.fileId)).map((one) => one.version)).toEqual([1]);
  });

  it('writes the size of the newest version on the attachment that gives the file', async () => {
    const { backend, boardId, file, add, versions, world } = await boardWithFile();
    const cardId = [...world.blocks.values()].find((block) => block.type === 'card')?.id ?? '';
    const attached = await backend.createAttachment(boardId, cardId, file);
    const size = async () =>
      (await backend.attachments(boardId, cardId)).find((one) => one.id === attached.id)?.size;
    expect(await size()).toBe(5);
    await add('the second', 1);
    expect(await size()).toBe(10);
    await versions.restore(boardId, file.fileId, 1, 2);
    expect(await size()).toBe(5);
  });

  it('copies a file as its newest version alone, by nobody', async () => {
    const { backend, boardId, file, add, versions, world } = await boardWithFile();
    const cardId = [...world.blocks.values()].find((block) => block.type === 'card')?.id ?? '';
    await backend.createAttachment(boardId, cardId, file);
    await add('the second', 1);
    const [workspace] = await backend.workspaces();
    const copy = await backend.duplicateContainer(boardId, false, workspace?.id ?? '');
    const copied = [...world.files.values()].find(
      (one) => one.containerId === copy.id && one.name === 'paper.txt',
    );
    if (!copied) throw new Error('the file was not copied');
    expect(await versions.list(copy.id, copied.fileId)).toMatchObject([
      { version: 1, createdBy: '', size: 10, isCurrent: true },
    ]);
    expect(await (await versions.bytes(copy.id, copied.fileId, 1)).text()).toBe('the second');
  });

  it('keeps the newest versions and lets the oldest go', async () => {
    const { boardId, file, add, textOf, versions } = await boardWithFile();
    for (let base = 1; base <= LIMITS.fileVersions; base += 1)
      await add(`v${String(base + 1)}`, base);
    const kept = (await versions.list(boardId, file.fileId)).map((one) => one.version);
    expect(kept).toHaveLength(LIMITS.fileVersions);
    expect([kept[0], kept.at(-1)]).toEqual([LIMITS.fileVersions + 1, 2]);
    expect((await rejection(textOf(1))).status).toBe(404);
    expect(await textOf(2)).toBe('v2');
  });

  it('is read by who reads the board and written by who edits it', async () => {
    const { backend, other, boardId, file, add } = await boardWithFile();
    await add('the second', 1);
    await backend.setMemberRole(boardId, other.userId, 'viewer');
    const theirs = other.fileVersions;
    expect((await theirs.list(boardId, file.fileId)).map((one) => one.version)).toEqual([2, 1]);
    expect(await (await theirs.bytes(boardId, file.fileId, 1)).text()).toBe('first');
    expect((await rejection(theirs.restore(boardId, file.fileId, 1, 2))).status).toBe(403);
    expect((await rejection(theirs.add(boardId, file.fileId, 2, file))).status).toBe(403);
    await backend.removeMember(boardId, other.userId);
    expect((await rejection(theirs.list(boardId, file.fileId))).status).not.toBe(200);
  });
});

describe('the versions of a file on the server', () => {
  /** Answers every request with `answer`, and keeps what was asked. */
  function serve(answer: unknown): { asked: string; body: unknown }[] {
    const sent: { asked: string; body: unknown }[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
      const body: unknown = typeof init.body === 'string' ? JSON.parse(init.body) : undefined;
      sent.push({ asked: `${init.method ?? 'GET'} ${url}`, body });
      return Promise.resolve(
        new Response(JSON.stringify(answer), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    });
    return sent;
  }

  const version = {
    version: 2,
    createdAt: 5,
    createdBy: 'u1',
    size: 3,
    isCurrent: true,
    restoredFrom: null,
  };
  const upload = { fileId: 'up.txt', name: 'a.txt', size: 3, contentType: 'text/plain' };

  it('reads the list and a version, writes one and goes back to one, under the workspace', async () => {
    const versions = serverBackend('').fileVersions;
    const at = '/api/v1/w/w1/containers/b/files/f.txt/versions';

    let sent = serve([version]);
    expect(await versions.list('b', 'f.txt')).toEqual([version]);
    expect(sent).toEqual([{ asked: `GET ${at}`, body: undefined }]);

    sent = serve('bytes');
    expect(await (await versions.bytes('b', 'f.txt', 2)).text()).toContain('bytes');
    expect(sent.map((one) => one.asked)).toEqual([`GET ${at}/2`]);

    sent = serve(version);
    expect(await versions.add('b', 'f.txt', 1, upload)).toEqual(version);
    expect(sent).toEqual([{ asked: `POST ${at}`, body: { baseVersion: 1, fileId: 'up.txt' } }]);

    sent = serve(version);
    expect(await versions.restore('b', 'f.txt', 1, 2)).toEqual(version);
    expect(sent).toEqual([{ asked: `POST ${at}/1/restore`, body: { baseVersion: 2 } }]);
  });

  it('has none for a reader by a public link', () => {
    const shared: Backend = new SharedBackend('', 'b1', 't');
    expect(shared.fileVersions).toBeUndefined();
  });
});

describe('why a version was refused', () => {
  it('tells a newer version, a version gone and a file that cannot be one apart', () => {
    expect(versionRefusal(new BackendError(409, { reason: FILE_VERSION_STALE }))).toBe('stale');
    expect(versionRefusal(new BackendError(409, {}))).toBe('failed');
    expect(versionRefusal(new BackendError(404, {}))).toBe('gone');
    expect(versionRefusal(new BackendError(400, {}))).toBe('unusable');
    expect(versionRefusal(new BackendError(500, {}))).toBe('failed');
    expect(versionRefusal(new Error('no connection'))).toBe('failed');
  });
});

describe('the list of the versions of a file attached to an open card', () => {
  async function listed() {
    const made = await openFirstBoard();
    const shown = made.board.cards.find((one) => one.title === 'Alpha');
    if (!shown) throw new Error('no card');
    const card = await made.board.showCard(shown.id);
    await card.attach(named('paper.txt', 'first'));
    const attachment = card.attachments.find((one) => one.name === 'paper.txt');
    if (!attachment) throw new Error('nothing was attached');
    const source = attachmentVersions(made.board, card, attachment);
    if (!source) throw new Error('the backend keeps no versions');
    const list = new VersionsList(source);
    await vi.waitFor(() => {
      expect(list.status).toBe('ready');
    });
    return { ...made, card, attachment, source, list };
  }

  it('reads the versions with the names of who wrote them', async () => {
    const { list, source, backend } = await listed();
    expect(list.versions.map((one) => one.version)).toEqual([1]);
    expect(list.current?.version).toBe(1);
    expect(source.canWrite).toBe(true);
    expect(source.userName(backend.userId)).toBeTruthy();
  });

  it('writes a new version from a file, and goes back to the one before', async () => {
    const { list, card, attachment } = await listed();
    const sizeOnCard = () => card.attachments.find((one) => one.id === attachment.id)?.size;
    const written = await list.add(named('paper.txt', 'the second'));
    expect(written?.version).toBe(2);
    // The card's row of the file says the newest version's size.
    expect(sizeOnCard()).toBe(10);
    expect(list.versions.map((one) => [one.version, one.isCurrent])).toEqual([
      [2, true],
      [1, false],
    ]);
    const first = list.versions.at(-1);
    if (!first) throw new Error('no first version');
    expect((await list.restore(first))?.restoredFrom).toBe(1);
    expect(list.versions.map((one) => one.version)).toEqual([3, 2, 1]);
    expect([list.refusal, list.writing, sizeOnCard()]).toEqual([undefined, undefined, 5]);
  });

  it('keeps a file that did not become a version, to be attached on its own or let go', async () => {
    const { list, other, attachment, card, board } = await listed();
    const theirs = await other.upload(board.boardId, named('paper.txt', 'theirs'));
    await other.fileVersions.add(board.boardId, attachment.fileId, 1, theirs);

    expect(await list.add(named('mine.txt', 'mine'))).toBeUndefined();
    expect(list.refusal).toBe('stale');
    expect(list.apart?.name).toBe('mine.txt');
    // The list was read again: what was written meanwhile is there.
    expect(list.versions.map((one) => one.version)).toEqual([2, 1]);

    expect(await list.attachApart()).toBe(true);
    expect([list.refusal, list.apart]).toEqual([undefined, undefined]);
    expect(card.attachments.map((one) => one.name)).toContain('mine.txt');

    // Going back from the list as it is now is no conflict.
    const first = list.versions.at(-1);
    if (!first) throw new Error('no first version');
    expect((await list.restore(first))?.version).toBe(3);
  });

  it('says so when a file cannot be a version, and when the upload did not go up', async () => {
    const { list, backend, workspace } = await listed();
    expect(await list.add(new File(['x'], 'picture.png'))).toBeUndefined();
    expect([list.refusal, list.apart]).toEqual(['unusable', undefined]);
    list.dismiss();

    backend.failNext('upload', new BackendError(413, { limit: 1 }));
    expect(await list.add(named('paper.txt', 'large'))).toBeUndefined();
    // The stack of uploads tells it, as for any other: not the list, nor where problems are.
    expect(list.refusal).toBeUndefined();
    expect(workspace.transfers.list.at(-1)).toMatchObject({
      name: 'paper.txt',
      failure: 'too-large',
      limit: 1,
    });
    expect(workspace.problems).toEqual([]);
    expect(list.writing).toBeUndefined();
  });

  it('says an upload given up in the stack is stopped, and one that failed is told there', async () => {
    const { source, backend, workspace } = await listed();
    backend.latency = 5;
    const stopping = source.upload(named('paper.txt', 'stopped'));
    workspace.transfers.dismiss(workspace.transfers.list.at(-1)?.id ?? 0);
    expect(await stopping).toBe('stopped');
    backend.failNext('upload', new BackendError(0, { detail: 'no answer' }));
    expect(await source.upload(named('paper.txt', 'lost'))).toBeUndefined();
    expect(workspace.transfers.list.at(-1)).toMatchObject({ failure: 'network' });
  });

  it('has no versions to show where the backend keeps none', async () => {
    const { board, card, attachment } = await listed();
    // A board whose backend is that of a reader by a public link: it has no versions.
    const without = {
      boardId: board.boardId,
      workspace: { backend: {} },
    } as unknown as typeof board;
    const source: VersionsSource | undefined = attachmentVersions(without, card, attachment);
    expect(source).toBeUndefined();
  });
});
