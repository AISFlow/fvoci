/**
 * Bodies in Markdown and containers as ZIPs of them (`Backend.markdown`): on
 * the server, by `format=markdown` and `exportContainerMarkdown`; in memory,
 * by its own writer; none through a public link.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { DocNode } from '$lib/editor/schema.ts';
import { markdownOf } from '$lib/state/memory/markdown-writer.ts';

import { openFirstBoard, serverBackend } from './support.ts';

const doc = (content: DocNode[]): DocNode => ({ type: 'doc', content });
const text = (value: string, marks?: DocNode['marks']): DocNode => ({
  type: 'text',
  text: value,
  ...(marks ? { marks } : {}),
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('the in-memory writer of Markdown', () => {
  it('writes text and its marks, headings, lists, quotes and code', () => {
    const body = doc([
      { type: 'heading', attrs: { level: 2 }, content: [text('Plan')] },
      {
        type: 'paragraph',
        content: [
          text('a '),
          text('bold', [{ type: 'bold' }]),
          text(' *star* and '),
          text('x()', [{ type: 'code' }]),
          text(' '),
          text('site', [{ type: 'link', attrs: { href: 'https://example.com' } }]),
        ],
      },
      {
        type: 'bulletList',
        content: [
          { type: 'listItem', content: [{ type: 'paragraph', content: [text('one')] }] },
          { type: 'listItem', content: [{ type: 'paragraph', content: [text('two')] }] },
        ],
      },
      {
        type: 'taskList',
        content: [
          {
            type: 'taskItem',
            attrs: { checked: true },
            content: [{ type: 'paragraph', content: [text('done')] }],
          },
        ],
      },
      { type: 'blockquote', content: [{ type: 'paragraph', content: [text('said')] }] },
      { type: 'codeBlock', attrs: { language: 'ts' }, content: [text('let a = 1;')] },
      { type: 'paragraph', content: [{ type: 'reference', attrs: { id: 'c1' } }] },
    ]);
    expect(markdownOf(body)).toBe(
      [
        '## Plan',
        'a **bold** \\*star\\* and `x()` [site](<https://example.com>)',
        '- one\n- two',
        '- [x] done',
        '> said',
        '```ts\nlet a = 1;\n```',
        '[[c1]]',
      ].join('\n\n'),
    );
    expect(markdownOf(null)).toBe('');
  });
});

describe('Markdown in the in-memory backend', () => {
  it('reads a card as its own read does, and exports the board for its members', async () => {
    const { world, backend, other, board } = await openFirstBoard();
    const markdown = backend.markdown;
    const [card] = board.cards;
    if (!card) throw new Error('the seed has a card');
    const read = await markdown.body({ kind: 'card', boardId: board.boardId, cardId: card.id });
    expect(read).toBe('first body');
    await expect(
      markdown.body({ kind: 'card', boardId: board.boardId, cardId: 'nothing' }),
    ).rejects.toMatchObject({ status: 404 });

    const zip = await markdown.exportContainer(board.boardId);
    expect(zip.name).toMatch(/^markdown-\d{4}-\d{2}-\d{2}\.zip$/);
    const bytes = new Uint8Array(await zip.bytes.arrayBuffer());
    const names = new TextDecoder().decode(bytes);
    expect(names).toContain(`/${card.title}.md`);

    // Once the other person is off the board, its Markdown is not theirs to take.
    const members = world.members.get(board.boardId) ?? [];
    world.members.set(
      board.boardId,
      members.filter((member) => member.userId !== other.userId),
    );
    await expect(other.markdown.exportContainer(board.boardId)).rejects.toMatchObject({
      status: 403,
    });
  });
});

describe('Markdown on the server', () => {
  it('reads a body with format=markdown under the workspace of its container', async () => {
    const sent: string[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit = {}) => {
      sent.push(`${init.method ?? 'GET'} ${url}`);
      return Promise.resolve(
        new Response(JSON.stringify({ bodyMarkdown: 'tick **tock**' }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    });
    const { markdown } = serverBackend('/base');
    expect(await markdown.body({ kind: 'page', wikiId: 'k', pageId: 'p' })).toBe('tick **tock**');
    await markdown.body({ kind: 'unit', courseId: 'u', unitId: 'n' });
    await markdown.body({ kind: 'card', boardId: 'b', cardId: 'c' });
    expect(sent).toEqual([
      'GET /base/api/v1/w/w1/wikis/k/pages/p?format=markdown',
      'GET /base/api/v1/w/w1/courses/u/units/n?format=markdown',
      'GET /base/api/v1/w/w1/cards/c?format=markdown',
    ]);
  });
});
