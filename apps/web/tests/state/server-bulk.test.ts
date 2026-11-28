/**
 * `batchPatchCards` of the backend that calls the server (api/bulk-requests.ts),
 * against a stand-in for `fetch`: what it sends, and that it takes its turn
 * on every card it names.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { inTurns } from '$lib/api/bulk-requests.ts';
import { BlockTurns } from '$lib/state/write.ts';

import { serverBackend } from './support.ts';

const CARD = {
  containerId: 'b',
  parentId: '',
  createdAt: 1,
  updatedAt: 2,
  createdBy: 'u',
  modifiedBy: 'u',
  title: 'Card',
  icon: '',
  isTemplate: false,
  body: null,
  hasBody: false,
  tasksTotal: 0,
  tasksDone: 0,
};

interface Sent {
  method: string;
  url: string;
  body: unknown;
}

/** A server whose writes wait until `release`; every request is kept, in the order it arrived. */
function server() {
  const sent: Sent[] = [];
  let release = (): void => undefined;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
    const body: unknown = typeof init.body === 'string' ? JSON.parse(init.body) : undefined;
    sent.push({ method: init.method ?? 'GET', url, body });
    if (url.includes('/cards/c1') && init.method === 'PATCH') await gate;
    const ids = (body as { cardIds?: string[] } | undefined)?.cardIds ?? ['c1'];
    const cards = ids.map((id) => ({ ...CARD, id, properties: { s: 'done' } }));
    const answer = url.includes('batch-patch') ? cards : cards[0];
    return new Response(JSON.stringify(answer), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  });
  return { sent, release };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('a change of many cards sent to the server', () => {
  it('posts the cards and the values, and keeps a repeated change from telling anybody', async () => {
    const { sent } = server();
    const backend = serverBackend('');
    const cards = await backend.batchPatchCards('b', ['c2', 'c3'], {
      updatedProperties: { s: 'done' },
      deletedProperties: [],
    });
    await backend.batchPatchCards('b', ['c2'], { deletedProperties: ['s'] }, { quiet: true });
    expect(sent).toEqual([
      {
        method: 'POST',
        url: '/api/v1/w/w1/boards/b/cards/batch-patch',
        body: { cardIds: ['c2', 'c3'], updatedProperties: { s: 'done' } },
      },
      {
        method: 'POST',
        url: '/api/v1/w/w1/boards/b/cards/batch-patch?disableNotify=true',
        body: { cardIds: ['c2'], deletedProperties: ['s'] },
      },
    ]);
    expect(cards.map((card) => [card.id, card.fields.properties])).toEqual([
      ['c2', { s: 'done' }],
      ['c3', { s: 'done' }],
    ]);
  });

  it('waits for a write of one of its cards sent before it', async () => {
    const { sent, release } = server();
    const backend = serverBackend('');
    const single = backend.patchCard('b', 'c1', { updatedProperties: { s: 'doing' } });
    const batch = backend.batchPatchCards('b', ['c2', 'c1'], { updatedProperties: { s: 'done' } });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(sent.map((one) => one.method)).toEqual(['PATCH']);
    release();
    await Promise.all([single, batch]);
    expect(sent.map((one) => one.method)).toEqual(['PATCH', 'POST']);
  });
});

describe('turns on many blocks', () => {
  it('runs once the earlier writes of each have ended, and holds the later ones', async () => {
    const turns = new BlockTurns();
    const order: string[] = [];
    let endFirst = (): void => undefined;
    const first = turns.run('a', async () => {
      await new Promise<void>((resolve) => {
        endFirst = resolve;
      });
      order.push('a');
    });
    const many = inTurns(turns, ['a', 'b', 'a'], () => {
      order.push('many');
      return Promise.resolve();
    });
    const later = turns.run('b', () => {
      order.push('b');
      return Promise.resolve();
    });
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(order).toEqual([]);
    endFirst();
    await Promise.all([first, many, later]);
    expect(order).toEqual(['a', 'many', 'b']);
  });

  it('lets the later writes go when it fails', async () => {
    const turns = new BlockTurns();
    await expect(inTurns(turns, ['a'], () => Promise.reject(new Error('no')))).rejects.toThrow(
      'no',
    );
    await expect(turns.run('a', () => Promise.resolve('next'))).resolves.toBe('next');
  });
});
