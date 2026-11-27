/**
 * The lease of a file open in the editor: the backend in memory keeps it as
 * the server does (one holder at a time, the same person's other window
 * included; held on to, it lasts its term from then; not held on to, it
 * ends; let go of, it is the next one's; read by the board's readers, held
 * by its editors), and the server backend asks for it, letting it go with a
 * request that outlives the page.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { attachmentVersions } from '$lib/features/card/file-versions.ts';
import { FILE_BEING_EDITED } from '$lib/state/file-versions.ts';
import { BackendError } from '$lib/state/index.ts';
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

/** A board with a file on it, whose two people both edit it, on a clock the test turns. */
async function boardWithFile() {
  const made = setup();
  // From the world's own time: it never goes back.
  let time = made.world.now() + 1000;
  made.world.useClock(() => time);
  const [workspace] = await made.backend.workspaces();
  const [board] = await made.backend.boards(workspace?.id ?? '');
  if (!board) throw new Error('no board');
  const file = await made.backend.upload(board.id, new File(['x'], 'paper.hwpx'));
  await made.backend.setMemberRole(board.id, made.other.userId, 'editor');
  const at = [board.id, file.fileId] as const;
  return {
    ...made,
    at,
    boardId: board.id,
    mine: made.backend.fileVersions,
    theirs: made.other.fileVersions,
    pass: (seconds: number) => {
      time += seconds * 1000;
    },
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('the lease of a file in memory', () => {
  it('is held by one editor at a time, and told to the others with who', async () => {
    const { backend, at, mine, theirs } = await boardWithFile();
    expect(await theirs.lease(...at)).toEqual({ editing: false, editorId: '' });
    const held = await mine.holdLease(...at, '');
    expect(held.seconds).toBe(LIMITS.fileLeaseSeconds);

    const refused = await rejection(theirs.holdLease(...at, ''));
    expect([refused.status, refused.reason]).toEqual([409, FILE_BEING_EDITED]);
    // The holder's other window, and somebody who names the holder's lease.
    expect((await rejection(mine.holdLease(...at, ''))).status).toBe(409);
    expect((await rejection(theirs.holdLease(...at, held.leaseId))).status).toBe(409);
    expect(await theirs.lease(...at)).toEqual({ editing: true, editorId: backend.userId });

    expect((await mine.holdLease(...at, held.leaseId)).leaseId).toBe(held.leaseId);
    // Letting another's lease go changes nothing.
    await theirs.releaseLease(...at, held.leaseId);
    expect((await theirs.lease(...at)).editing).toBe(true);
    await mine.releaseLease(...at, held.leaseId);
    expect((await theirs.lease(...at)).editing).toBe(false);
    expect((await theirs.holdLease(...at, '')).leaseId).not.toBe(held.leaseId);
  });

  it('lasts its term from when it was held on to last, and ends unheld', async () => {
    const { at, mine, theirs, pass } = await boardWithFile();
    const term = LIMITS.fileLeaseSeconds;
    const held = await mine.holdLease(...at, '');
    pass(term - 1);
    await mine.holdLease(...at, held.leaseId);
    pass(2);
    expect((await rejection(theirs.holdLease(...at, ''))).status).toBe(409);
    pass(term);
    expect(await theirs.lease(...at)).toEqual({ editing: false, editorId: '' });
    const taken = await theirs.holdLease(...at, '');
    expect(taken.leaseId).not.toBe(held.leaseId);
    // The lease that ended is no more its holder's to hold on to.
    expect((await rejection(mine.holdLease(...at, held.leaseId))).status).toBe(409);
  });

  it('is told to the board’s readers, who hold none', async () => {
    const { backend, other, boardId, at, mine, theirs } = await boardWithFile();
    const held = await mine.holdLease(...at, '');
    await backend.setMemberRole(boardId, other.userId, 'viewer');
    expect((await theirs.lease(...at)).editing).toBe(true);
    expect((await rejection(theirs.holdLease(...at, ''))).status).toBe(403);
    expect((await rejection(theirs.releaseLease(...at, held.leaseId))).status).toBe(403);
    await backend.removeMember(boardId, other.userId);
    expect((await rejection(theirs.lease(...at))).status).not.toBe(200);
  });
});

describe('the lease of a file on the server', () => {
  it('is read, held and let go of under the workspace, as the page goes too', async () => {
    const sent: { asked: string; body: unknown; keepalive: boolean | undefined }[] = [];
    const answers: unknown[] = [
      { editing: true, editorId: 'u1' },
      { leaseId: 'l1', seconds: 90 },
      undefined,
    ];
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
      const body: unknown = typeof init.body === 'string' ? JSON.parse(init.body) : undefined;
      sent.push({ asked: `${init.method ?? 'GET'} ${url}`, body, keepalive: init.keepalive });
      const answer = answers.shift();
      return Promise.resolve(
        answer === undefined
          ? new Response(null, { status: 204 })
          : new Response(JSON.stringify(answer), {
              status: 200,
              headers: { 'Content-Type': 'application/json' },
            }),
      );
    });
    const versions = serverBackend('').fileVersions;
    const at = '/api/v1/w/w1/containers/b/files/f.hwpx/lease';
    expect(await versions.lease('b', 'f.hwpx')).toEqual({ editing: true, editorId: 'u1' });
    expect(await versions.holdLease('b', 'f.hwpx', '')).toEqual({ leaseId: 'l1', seconds: 90 });
    await versions.releaseLease('b', 'f.hwpx', 'l1', { keepalive: true });
    expect(sent.map(({ asked, body }) => [asked, body])).toEqual([
      [`GET ${at}`, undefined],
      [`PUT ${at}`, { leaseId: '' }],
      [`DELETE ${at}/l1`, undefined],
    ]);
    expect(sent.at(-1)?.keepalive).toBe(true);
  });
});

describe('the lease of a file attached to an open card', () => {
  it('says a lease another holds is theirs, and names the reader', async () => {
    const made = await openFirstBoard();
    const shown = made.board.cards.find((one) => one.title === 'Alpha');
    if (!shown) throw new Error('no card');
    const card = await made.board.showCard(shown.id);
    await card.attach(new File(['x'], 'paper.hwpx'));
    const attachment = card.attachments.find((one) => one.name === 'paper.hwpx');
    if (!attachment) throw new Error('nothing was attached');
    const source = attachmentVersions(made.board, card, attachment);
    if (!source) throw new Error('the backend keeps no versions');
    expect(source.me).toBe(made.backend.userId);
    const held = await source.holdLease('');
    if (held === 'editing') throw new Error('nobody else holds it');
    expect(await source.holdLease('')).toBe('editing');
    expect(await source.lease()).toEqual({ editing: true, editorId: made.backend.userId });
    await source.releaseLease(held.leaseId, false);
    expect((await source.lease()).editing).toBe(false);
  });
});
