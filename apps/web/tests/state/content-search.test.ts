/**
 * The search of what cards and pages say, at the data boundary: the backend
 * in memory finds as the server does (what the reader has a role on, no
 * templates, a snippet in stretches), the server's is asked as the contract
 * says, and a reader by a public link has none.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { SharedBackend } from '$lib/api/shared-backend.ts';
import { createBoard, createCard, type Block } from '$lib/logic/wire.ts';
import { BackendError, type SearchHit } from '$lib/state/backend.ts';
import { MemoryBackend, memberOf } from '$lib/state/memory/backend.ts';
import { markedParts, SNIPPET_CHARS, snippetOf } from '$lib/state/memory/content-search.ts';
import { World } from '$lib/state/memory/world.ts';

import { serverBackend, text } from './support.ts';

const NOW = Date.UTC(2026, 10, 15, 3);

function block(id: string, boardId: string, title: string, updatedAt = NOW): Block {
  return {
    id,
    type: 'card',
    containerId: boardId,
    parentId: '',
    createdBy: 'reader',
    modifiedBy: 'reader',
    title,
    fields: {},
    createdAt: NOW,
    updatedAt,
  };
}

/** A world of boards: the reader's, another's, an open one not joined, and a template. */
function world(): World {
  const made = new World(() => NOW);
  made.workspace = { id: 'w', title: 'Example', role: 'admin' };
  const board = (id: string, extra: object = {}): void => {
    made.containers.set(id, {
      ...createBoard(),
      id,
      workspaceId: 'w',
      title: `Board ${id}`,
      ...extra,
    });
  };
  board('mine');
  board('theirs');
  board('open', { visibility: 'open' });
  board('template', { isTemplate: true });
  made.members.set('mine', [memberOf('mine', 'reader', 'viewer')]);
  made.members.set('theirs', [memberOf('theirs', 'someone', 'admin')]);
  made.members.set('open', [memberOf('open', 'someone', 'admin')]);
  made.members.set('template', [memberOf('template', 'reader', 'admin')]);
  const card = (id: string, boardId: string, title: string, body: string, extra = {}): void => {
    made.blocks.set(id, {
      ...createCard(block(id, boardId, title)),
      body: text(body),
      ...extra,
    });
  };
  card('found', 'mine', 'Release plan', 'The kangaroo route is ready');
  card('titled', 'mine', 'Kangaroo', 'Nothing about it here');
  card('secret', 'theirs', 'Theirs', 'A kangaroo of theirs');
  card('unjoined', 'open', 'Open', 'A kangaroo nobody joined');
  card('in-template', 'template', 'Template', 'A kangaroo template');
  const template = createCard(block('card-template', 'mine', 'Card template'));
  made.blocks.set('card-template', {
    ...template,
    fields: { ...template.fields, isTemplate: true },
    body: text('A kangaroo to copy'),
  });
  return made;
}

const ids = (hits: SearchHit[]): string[] => hits.map((hit) => hit.id);

describe('the search in memory', () => {
  it('finds the cards of the boards the reader has a role on, of no template', async () => {
    const backend = new MemoryBackend(world(), 'reader');
    const hits = await backend.search.find('w', { query: 'kangaroo' });
    // The title first.
    expect(ids(hits)).toEqual(['titled', 'found']);
    expect(hits[1]).toMatchObject({
      kind: 'card',
      containerId: 'mine',
      containerKind: 'board',
      title: 'Release plan',
    });
  });

  it('finds the pages of an open wiki the reader works beside, not joined, as the server does', async () => {
    const made = world();
    made.containers.set('notes', {
      ...createBoard(),
      id: 'notes',
      workspaceId: 'w',
      title: 'Notes',
      visibility: 'open',
      kind: 'wiki',
    });
    made.members.set('notes', [memberOf('notes', 'someone', 'admin')]);
    made.blocks.set('note', { ...block('note', 'notes', 'Kangaroo notes'), type: 'page' });
    const backend = new MemoryBackend(made, 'reader');
    expect(ids(await backend.search.find('w', { query: 'notes' }))).toEqual([]);
    made.workspaceMembers.add('reader');
    const hits = await backend.search.find('w', { query: 'notes' });
    expect(hits.map((hit) => [hit.id, hit.containerKind])).toEqual([['note', 'wiki']]);
  });

  it('finds units of an open course the reader works beside, never chapters', async () => {
    const made = world();
    made.containers.set('course-1', {
      ...createBoard(),
      id: 'course-1',
      workspaceId: 'w',
      title: 'Onboarding',
      visibility: 'open',
      kind: 'course',
    });
    made.members.set('course-1', [memberOf('course-1', 'someone', 'admin')]);
    made.blocks.set('ch-1', { ...block('ch-1', 'course-1', 'Kangaroo chapter'), type: 'chapter' });
    made.blocks.set('u-1', {
      ...block('u-1', 'course-1', 'First unit'),
      type: 'unit',
      parentId: 'ch-1',
      body: text('Learn about kangaroo routes'),
    });
    const backend = new MemoryBackend(made, 'reader');
    expect(ids(await backend.search.find('w', { query: 'kangaroo', kind: 'unit' }))).toEqual([]);
    made.workspaceMembers.add('reader');
    const hits = await backend.search.find('w', { query: 'kangaroo', kind: 'unit' });
    expect(hits.map((hit) => [hit.id, hit.kind, hit.containerKind])).toEqual([
      ['u-1', 'unit', 'course'],
    ]);
  });

  it('wants every word, in any case, and finds nothing for no words', async () => {
    const backend = new MemoryBackend(world(), 'reader');
    expect(ids(await backend.search.find('w', { query: 'ROUTE kangaroo' }))).toEqual(['found']);
    expect(ids(await backend.search.find('w', { query: 'route wallaby' }))).toEqual([]);
    expect(await backend.search.find('w', { query: '   ' })).toEqual([]);
  });

  it('finds one kind when asked, and as many as asked', async () => {
    const backend = new MemoryBackend(world(), 'reader');
    expect(await backend.search.find('w', { query: 'kangaroo', kind: 'page' })).toEqual([]);
    expect(await backend.search.find('w', { query: 'kangaroo', limit: 1 })).toHaveLength(1);
  });

  it('marks the words in a piece of the body, and gives the beginning of a body that has none', async () => {
    const backend = new MemoryBackend(world(), 'reader');
    const [titled, found] = await backend.search.find('w', { query: 'kangaroo' });
    expect(found?.snippet).toEqual([
      { text: 'The ', matched: false },
      { text: 'kangaroo', matched: true },
      { text: ' route is ready', matched: false },
    ]);
    expect(titled?.snippet).toEqual([{ text: 'Nothing about it here', matched: false }]);
  });

  it('refuses what the server refuses', async () => {
    const backend = new MemoryBackend(world(), 'reader');
    const refusal = (promise: Promise<unknown>): Promise<unknown> =>
      promise.catch((error: unknown) => error);
    const long = await refusal(backend.search.find('w', { query: 'x'.repeat(201) }));
    expect(long).toBeInstanceOf(BackendError);
    expect((long as BackendError).status).toBe(400);
    expect((long as BackendError).errors[0]?.parameter).toBe('q');
    // Characters, not code units: two hundred of a script beyond the first plane pass.
    await expect(backend.search.find('w', { query: '𝒳'.repeat(200) })).resolves.toEqual([]);
    const limit = await refusal(backend.search.find('w', { query: 'a', limit: 51 }));
    expect((limit as BackendError).status).toBe(400);
    const elsewhere = await refusal(backend.search.find('other', { query: 'a' }));
    expect((elsewhere as BackendError).status).toBe(404);
  });

  it('says whether bodies are searched as the world says', async () => {
    const off = world();
    off.searchEnabled = false;
    expect(await new MemoryBackend(off, 'reader').search.enabled()).toBe(false);
    expect(await new MemoryBackend(world(), 'reader').search.enabled()).toBe(true);
  });

  it('takes the failure a test asked for, as every call does', async () => {
    const backend = new MemoryBackend(world(), 'reader');
    backend.failNext('search', new BackendError(503, { reason: 'search_unavailable' }));
    await expect(backend.search.find('w', { query: 'kangaroo' })).rejects.toMatchObject({
      status: 503,
    });
    await expect(backend.search.find('w', { query: 'kangaroo' })).resolves.toHaveLength(2);
  });

  it('finds `#12` and `#0012` cards across readable boards with containerTitle, excluding templates and private boards', async () => {
    const made = world();
    made.containers.set('second', {
      ...createBoard(),
      id: 'second',
      workspaceId: 'w',
      title: 'Roadmap',
    });
    made.members.set('second', [memberOf('second', 'reader', 'editor')]);
    const c1 = made.blocks.get('found');
    const c2 = made.blocks.get('secret');
    const c3 = made.blocks.get('unjoined');
    const c4 = made.blocks.get('in-template');
    const c5 = made.blocks.get('card-template');
    if (!c1 || !c2 || !c3 || !c4 || !c5) throw new Error('missing seed cards');
    c1.number = 12;
    c2.number = 12;
    c3.number = 12;
    c4.number = 12;
    // Preserved number on a card template must stay excluded from `#12` search.
    c5.number = 12;
    made.blocks.set('second-12', {
      ...createCard(block('second-12', 'second', 'Ship milestone', NOW + 1000)),
      number: 12,
      body: text('Other board card'),
    });
    made.blocks.set('mixed', {
      ...createCard(block('mixed', 'mine', 'Check #12abc issue')),
      number: 99,
    });

    const backend = new MemoryBackend(made, 'reader');
    const hits = await backend.search.find('w', { query: '#12' });
    expect(hits).toEqual([
      {
        id: 'second-12',
        kind: 'card',
        containerId: 'second',
        containerKind: 'board',
        containerTitle: 'Roadmap',
        title: 'Ship milestone',
        number: 12,
        snippet: [],
      },
      {
        id: 'found',
        kind: 'card',
        containerId: 'mine',
        containerKind: 'board',
        containerTitle: 'Board mine',
        title: 'Release plan',
        number: 12,
        snippet: [],
      },
    ]);

    // Leading zeros normalize to `#12`.
    expect(ids(await backend.search.find('w', { query: '  #0012 ' }))).toEqual([
      'second-12',
      'found',
    ]);
    // Filtering by kind=page runs ordinary text search (never returning cards, still finding pages mentioning `#12`).
    made.containers.set('notes-wiki', {
      ...createBoard(),
      id: 'notes-wiki',
      workspaceId: 'w',
      title: 'Notes',
      visibility: 'open',
      kind: 'wiki',
    });
    made.members.set('notes-wiki', [memberOf('notes-wiki', 'reader', 'editor')]);
    made.blocks.set('page-12', {
      ...block('page-12', 'notes-wiki', 'Sprint notes'),
      type: 'page',
      body: text('See #12 details'),
    });
    expect(ids(await backend.search.find('w', { query: '#12', kind: 'page' }))).toEqual([
      'page-12',
    ]);
    // Mixed non-digits fall back to normal text search.
    expect(ids(await backend.search.find('w', { query: '#12abc' }))).toEqual(['mixed']);
  });
});

describe('the snippet of a body', () => {
  it('is cut around the first word found, in characters', () => {
    const body = `${'가'.repeat(300)} target ${'나'.repeat(300)}`;
    const parts = snippetOf(body, ['target']);
    const joined = parts.map((part) => part.text).join('');
    expect(Array.from(joined)).toHaveLength(SNIPPET_CHARS);
    expect(parts.filter((part) => part.matched).map((part) => part.text)).toEqual(['target']);
  });

  it('marks the longest of two words where one holds the other, and keeps the case of the text', () => {
    expect(markedParts('Foobar and foo', ['foo', 'FOOBAR'])).toEqual([
      { text: 'Foobar', matched: true },
      { text: ' and ', matched: false },
      { text: 'foo', matched: true },
    ]);
  });

  it('is not moved by letters whose small form is longer', () => {
    const body = `${'İ'.repeat(100)} middle target ${'x'.repeat(300)}`;
    const joined = snippetOf(body, ['target'])
      .map((part) => part.text)
      .join('');
    // Forty characters before the word, as for any other letters.
    expect(joined.startsWith(`${'İ'.repeat(32)} middle target`)).toBe(true);
  });

  it('is empty for an empty body', () => {
    expect(snippetOf('', ['a'])).toEqual([]);
  });
});

describe('the search on the server', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function serve(answers: Record<string, unknown>): string[] {
    const asked: string[] = [];
    vi.stubGlobal('fetch', (url: string) => {
      asked.push(url);
      const path = new URL(url, 'http://here').pathname;
      return Promise.resolve(
        new Response(JSON.stringify(answers[path] ?? {}), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    });
    return asked;
  }

  it('asks the search of the workspace, and gives its hits as they are', async () => {
    const hit = {
      id: 'c',
      kind: 'card',
      containerId: 'b',
      containerKind: 'board',
      containerTitle: 'Board B',
      title: 'Title',
      number: 12,
      snippet: [{ text: '<b>', matched: true }],
    };
    const asked = serve({ '/api/v1/w/w1/search': [hit] });
    const backend = serverBackend('');
    const hits = await backend.search.find('w1', { query: '환불 규정', kind: 'card', limit: 20 });
    expect(hits).toEqual([hit]);
    const url = new URL(asked[0] ?? '', 'http://here');
    expect(url.searchParams.get('q')).toBe('환불 규정');
    expect(url.searchParams.get('kind')).toBe('card');
    expect(url.searchParams.get('limit')).toBe('20');
    // Without a kind the server finds both.
    await backend.search.find('w1', { query: 'a' });
    expect(new URL(asked[1] ?? '', 'http://here').searchParams.has('kind')).toBe(false);
  });

  it('is on as the settings for clients say, which are read once', async () => {
    const config = {
      allowPublicLinks: true,
      featureFlags: {},
      maxFileBytes: 1,
      maxImportBytes: 1,
      searchEnabled: true,
      nameDisplay: 'username',
    };
    const asked = serve({ '/api/v1/client-config': config });
    const backend = serverBackend('');
    expect(await backend.search.enabled()).toBe(true);
    expect(await backend.search.enabled()).toBe(true);
    expect(asked.filter((url) => url.endsWith('/client-config'))).toHaveLength(1);
  });
});

describe('the search of a reader by a public link', () => {
  it('is off, and finds nothing without asking', async () => {
    const fetched = vi.fn();
    vi.stubGlobal('fetch', fetched);
    const backend = new SharedBackend('', 'board', 'token');
    expect(await backend.search.enabled()).toBe(false);
    expect(await backend.search.find('w', { query: 'a' })).toEqual([]);
    expect(fetched).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });
});
