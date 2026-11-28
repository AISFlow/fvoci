/**
 * What bodies name, by each backend: the one in memory answers as the server
 * does (a card is told to who is on its board, a page to who is on its wiki,
 * and nobody else, whatever is asked), the server backend asks the three operations under the
 * workspace, and a reader by a public link is told of the link's board alone
 * and asks the server nothing more for it.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { SharedBackend } from '$lib/api/shared-backend.ts';
import type { DocNode } from '$lib/editor/schema.ts';
import { isBackendError } from '$lib/state/index.ts';
import { REFERENCES_AT_ONCE } from '$lib/state/references.ts';

import { serverBackend, setup } from './support.ts';

const UNKNOWN = '01930000-0000-7000-8000-00000000dead';

const naming = (...ids: string[]): DocNode => ({
  type: 'doc',
  content: [
    {
      type: 'paragraph',
      content: ids.map((id) => ({ type: 'reference', attrs: { id } })),
    },
  ],
});

/** What a body names, in the order it stands. */
function namedBy(node: DocNode | null | undefined): string[] {
  if (!node) return [];
  const own = node.type === 'reference' ? [String(node.attrs?.id)] : [];
  return [...own, ...(node.content ?? []).flatMap(namedBy)];
}

async function rejection(promise: Promise<unknown>): Promise<{ status: number }> {
  try {
    await promise;
  } catch (error) {
    if (isBackendError(error)) return error;
    throw error;
  }
  throw new Error('nothing was refused');
}

/** The small world, its board and three of its cards by title. */
function world() {
  const made = setup();
  const cards = [...made.world.blocks.values()].filter((block) => block.type === 'card');
  const [alpha, beta, gamma] = ['Alpha', 'Beta', 'Gamma'].map((title) =>
    cards.find((card) => card.title === title),
  );
  if (!alpha || !beta || !gamma) throw new Error('the seed has no such cards');
  const workspaceId = made.world.workspace.id;
  const write = async (cardId: string, body: DocNode): Promise<void> => {
    const now = await made.backend.card(alpha.containerId, cardId);
    await made.backend.writeCardBody(alpha.containerId, cardId, { body, readAt: now.updatedAt });
  };
  return { ...made, boardId: alpha.containerId, workspaceId, alpha, beta, gamma, write };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('references, in memory', () => {
  it('resolves the cards the reader is on the board of, and tells nothing of the rest', async () => {
    const { backend, other, workspaceId, boardId, alpha, beta } = world();
    const ids = [alpha.id, UNKNOWN, beta.id, alpha.id];
    const found = await backend.references.resolve(workspaceId, ids);
    expect(found).toEqual([
      {
        id: alpha.id,
        kind: 'card',
        title: 'Alpha',
        icon: '',
        containerId: boardId,
        containerKind: 'board',
        number: 1,
      },
      {
        id: beta.id,
        kind: 'card',
        title: 'Beta',
        icon: '',
        containerId: boardId,
        containerKind: 'board',
        number: 2,
      },
    ]);

    // Off the board, a card that is there is told as one that is not: the same answer.
    await backend.removeMember(boardId, other.userId);
    expect(await other.references.resolve(workspaceId, ids)).toEqual([]);
    expect(await other.references.resolve(workspaceId, [UNKNOWN])).toEqual([]);
    expect(await other.references.search(workspaceId, 'alpha')).toEqual([]);
    expect((await rejection(other.references.to(workspaceId, alpha.id))).status).toBe(404);
    expect((await rejection(other.references.to(workspaceId, UNKNOWN))).status).toBe(404);
  });

  it('resolves and finds the pages of a wiki the reader is on, and tells nothing of them to the rest', async () => {
    const { backend, other, workspaceId } = world();
    const wiki = await backend.wiki.createWiki(workspaceId, {
      title: 'Handbook',
      visibility: 'private',
    });
    const page = await backend.wiki.createPage(wiki.id, { title: 'Road plan' });
    const told = { id: page.id, kind: 'page', title: 'Road plan', icon: '', containerId: wiki.id };
    expect(await backend.references.resolve(workspaceId, [page.id])).toEqual([
      { ...told, containerKind: 'wiki' },
    ]);
    expect(await backend.references.search(workspaceId, 'road')).toEqual([
      { ...told, containerKind: 'wiki' },
    ]);
    expect(await backend.references.to(workspaceId, page.id)).toEqual([]);
    // Not on the private wiki: the page is as nothing is.
    expect(await other.references.resolve(workspaceId, [page.id])).toEqual([]);
    expect(await other.references.search(workspaceId, 'road')).toEqual([]);
    expect((await rejection(other.references.to(workspaceId, page.id))).status).toBe(404);
  });

  it('takes no more identifiers at once than the server, and no other workspace', async () => {
    const { backend, workspaceId, alpha } = world();
    const many = Array.from({ length: REFERENCES_AT_ONCE + 1 }, () => alpha.id);
    expect((await rejection(backend.references.resolve(workspaceId, many))).status).toBe(400);
    expect((await rejection(backend.references.resolve('elsewhere', [alpha.id]))).status).toBe(404);
  });

  it('tells what names a card by its body or a comment, the oldest first, a page at a time', async () => {
    const { backend, workspaceId, boardId, alpha, beta, gamma, write } = world();
    expect(await backend.references.to(workspaceId, alpha.id)).toEqual([]);
    await write(gamma.id, naming(alpha.id, alpha.id));
    await backend.createComment(boardId, beta.id, naming(alpha.id));
    // A card that names itself is not what names it.
    await write(alpha.id, naming(alpha.id));
    const titles = async (paging?: { page: number; perPage: number }): Promise<string[]> =>
      (await backend.references.to(workspaceId, alpha.id, paging)).map((linked) => linked.title);
    expect((await titles()).sort()).toEqual(['Beta', 'Gamma']);
    const [first] = await titles({ page: 0, perPage: 1 });
    const [second] = await titles({ page: 1, perPage: 1 });
    expect([first, second]).toEqual(await titles());
    expect(await titles({ page: 2, perPage: 1 })).toEqual([]);
    // 0, below 0 and above 1000 are refused.
    await expect(titles({ page: -1, perPage: 1 })).rejects.toMatchObject({ status: 400 });
    await expect(titles({ page: 0, perPage: 0 })).rejects.toMatchObject({ status: 400 });
    await expect(titles({ page: 0, perPage: 1001 })).rejects.toMatchObject({ status: 400 });

    // Only the body as it is now counts.
    await write(gamma.id, naming(beta.id));
    expect(await titles()).toEqual(['Beta']);
    await backend.batchDeleteCards(boardId, [beta.id]);
    expect(await titles()).toEqual([]);
  });

  it('finds cards by every word of the title, whatever the case, and no template', async () => {
    const { backend, workspaceId, boardId } = world();
    await backend.createCard(boardId, { title: 'Road map of the year' });
    await backend.createCard(boardId, { title: 'Road works', isTemplate: true });
    const found = async (term: string): Promise<string[]> =>
      (await backend.references.search(workspaceId, term)).map((linked) => linked.title);
    expect(await found('ROAD')).toEqual(['Road map of the year']);
    expect(await found('year road')).toEqual(['Road map of the year']);
    expect(await found('road nowhere')).toEqual([]);
    expect((await found('')).length).toBeGreaterThan(2);
  });

  it('points the references of a copied board at the copies, and leaves what was not copied', async () => {
    const { backend, world: stored, workspaceId, boardId, alpha, beta, write } = world();
    await write(beta.id, naming(alpha.id, UNKNOWN));
    const copy = await backend.duplicateContainer(boardId, false, workspaceId);
    const copies = [...stored.blocks.values()].filter((block) => block.containerId === copy.id);
    const copied = (title: string) =>
      copies.find((block) => block.type === 'card' && block.title === title);
    expect(copied('Alpha')?.id).not.toBe(alpha.id);
    expect(namedBy(copied('Beta')?.body)).toEqual([copied('Alpha')?.id, UNKNOWN]);
    // The original still names the original.
    expect(namedBy(stored.blocks.get(beta.id)?.body)).toEqual([alpha.id, UNKNOWN]);
  });
});

describe('references, from the server', () => {
  interface Sent {
    request: string;
    body: unknown;
  }

  /** Answers every request with `answer`, and keeps what was asked. */
  function serve(answer: unknown): Sent[] {
    const sent: Sent[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit = {}) => {
      const body: unknown = typeof init.body === 'string' ? JSON.parse(init.body) : undefined;
      sent.push({ request: `${init.method ?? 'GET'} ${url}`, body });
      return Promise.resolve(
        new Response(JSON.stringify(answer), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    });
    return sent;
  }

  const linked = {
    id: 'c',
    kind: 'card',
    title: 'Plan',
    icon: '',
    containerId: 'b',
    containerKind: 'board',
  };

  it('asks the three operations under the workspace', async () => {
    const { references } = serverBackend('/base');
    let sent = serve([linked]);
    expect(await references.resolve('w1', ['c', 'x', 'c'])).toEqual([linked]);
    expect(sent).toEqual([
      { request: 'POST /base/api/v1/w/w1/references/resolve', body: ['c', 'x'] },
    ]);

    sent = serve([linked]);
    expect(await references.to('w1', 'c 1', { page: 2, perPage: 50 })).toEqual([linked]);
    expect(await references.to('w1', 'c')).toEqual([linked]);
    expect(sent.map((one) => one.request)).toEqual([
      'GET /base/api/v1/w/w1/references/to/c%201?page=2&perPage=50',
      'GET /base/api/v1/w/w1/references/to/c',
    ]);

    sent = serve([linked]);
    expect(await references.search('w1', 'road map')).toEqual([linked]);
    expect(sent.map((one) => one.request)).toEqual([
      'GET /base/api/v1/w/w1/references/search?q=road+map',
    ]);
  });

  it('resolves more than the server takes at once in parts', async () => {
    const sent = serve([]);
    const ids = Array.from({ length: REFERENCES_AT_ONCE * 2 + 1 }, (_, at) => `id-${String(at)}`);
    await serverBackend('').references.resolve('w1', ids);
    expect(sent.map((one) => (one.body as string[]).length)).toEqual([
      REFERENCES_AT_ONCE,
      REFERENCES_AT_ONCE,
      1,
    ]);
    expect(sent.flatMap((one) => one.body as string[])).toEqual(ids);
  });
});

describe('references, for a reader by a public link', () => {
  const card = (id: string, title: string, icon?: string) => ({
    id,
    boardId: 'b1',
    title,
    ...(icon === undefined ? {} : { icon }),
    properties: {},
    isTemplate: false,
    createdAt: 1,
    updatedAt: 1,
  });

  it('are resolved among the cards of the link’s board, and nothing else is asked', async () => {
    const sent: string[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit = {}) => {
      sent.push(`${init.method ?? 'GET'} ${url}`);
      const cards = [card('c1', 'Plan', '🚀'), card('c2', 'Other')];
      return Promise.resolve(
        new Response(JSON.stringify(cards), { headers: { 'Content-Type': 'application/json' } }),
      );
    });
    const { references } = new SharedBackend('', 'b1', 't');
    expect(await references.resolve('', ['c1', UNKNOWN])).toEqual([
      {
        id: 'c1',
        kind: 'card',
        title: 'Plan',
        icon: '🚀',
        containerId: 'b1',
        containerKind: 'board',
      },
    ]);
    expect(sent).toEqual(['GET /api/v1/shared/b1/cards?page=0&perPage=1000&readToken=t']);

    // What names a card and what there is to name are not a link's reader's to ask.
    sent.length = 0;
    expect(await references.to('', 'c1')).toEqual([]);
    expect(await references.search('', 'plan')).toEqual([]);
    expect(sent).toEqual([]);
  });
});
