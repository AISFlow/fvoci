/**
 * The content of a board for the invited only, in the memory backend as on
 * the server: a reader who is not among its members is told it is not there
 * (404), while the board and its members stay readable for those who manage
 * it (`src/lib/state/managed-container.ts`). The e2e tests and the Storybook's
 * managed board run on this backend, with the same refusal as the server.
 */
import { describe, expect, it } from 'vitest';

import { BackendError } from '$lib/state/index.ts';
import { MemoryBackend, memberOf } from '$lib/state/memory/backend.ts';
import { World } from '$lib/state/memory/world.ts';
import { createBoard, createBoardView, createCard, type Block } from '$lib/logic/wire.ts';

const NOW = Date.UTC(2026, 10, 13, 3);

function inviteOnlyBoard(): MemoryBackend {
  const world = new World(() => NOW);
  world.workspace = { id: 'w', title: 'Example', role: 'admin' };
  world.users.set('reader', {
    id: 'reader',
    username: 'example-reader',
    displayName: 'Example reader',
    email: 'example-reader@example.test',
    createdAt: NOW,
    updatedAt: NOW,
  });
  // Both work in the workspace: an open board of it asks the reader to join.
  world.workspaceMembers.add('reader').add('someone-else');
  const board = {
    ...createBoard(),
    id: 'b',
    workspaceId: 'w',
    visibility: 'private' as const,
    title: 'Hidden',
  };
  world.containers.set(board.id, board);
  world.members.set(board.id, [memberOf(board.id, 'someone-else', 'admin')]);
  const block = (id: string, type: Block['type'], title: string): Block => ({
    id,
    type,
    containerId: 'b',
    parentId: '',
    createdBy: 'someone-else',
    modifiedBy: 'someone-else',
    title,
    fields: {},
    createdAt: NOW,
    updatedAt: NOW,
  });
  const view = createBoardView(block('v', 'view', 'View'));
  const card = createCard(block('c', 'card', 'Secret title'));
  world.blocks.set(view.id, view);
  world.blocks.set(card.id, card);
  world.blocks.set('comment', {
    ...block('comment', 'comment', ''),
    parentId: 'c',
    body: {
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Example comment' }] }],
    },
  });
  const file = { fileId: 'file', name: 'example.txt', size: 12, contentType: 'text/plain' };
  world.blocks.set('attachment', {
    ...block('attachment', 'attachment', file.name),
    parentId: 'c',
    file,
  });
  world.files.set(file.fileId, {
    ...file,
    containerId: 'b',
    blob: new Blob(['Example file']),
    url: 'blob:example',
  });
  return new MemoryBackend(world, 'reader');
}

const statusOf = async (promise: Promise<unknown>): Promise<number | 'ok'> => {
  try {
    await promise;
    return 'ok';
  } catch (error) {
    if (error instanceof BackendError) return error.status;
    throw error;
  }
};

describe('a board the reader is not a member of', () => {
  it('does not give its views, cards or a card to the reader', async () => {
    const backend = inviteOnlyBoard();
    expect({
      views: await statusOf(backend.views('b')),
      cards: await statusOf(backend.cards('b')),
      card: await statusOf(backend.card('b', 'c')),
      previews: await statusOf(backend.cardPreviews('b')),
      comments: await statusOf(backend.comments('b', 'c')),
      attachments: await statusOf(backend.attachments('b', 'c')),
      bytes: await statusOf(backend.fileBytes('b', 'file')),
    }).toEqual({
      views: 404,
      cards: 404,
      card: 404,
      previews: 404,
      comments: 404,
      attachments: 404,
      bytes: 404,
    });
    expect(backend.fileUrl('b', 'file')).toBeUndefined();
  });

  it('still lets the reader see the board and its members, to manage them', async () => {
    const backend = inviteOnlyBoard();
    expect(await statusOf(backend.container('b'))).toBe('ok');
    expect(await statusOf(backend.members('b'))).toBe('ok');
  });
});

describe('membership changes', () => {
  it('allows content after joining and refuses it again after leaving', async () => {
    const backend = inviteOnlyBoard();
    await backend.addMember('b', 'reader', 'viewer');
    expect(await backend.views('b')).toHaveLength(1);
    expect(await backend.cards('b')).toHaveLength(1);
    expect((await backend.card('b', 'c')).id).toBe('c');
    expect((await backend.cardPreviews('b')).size).toBe(1);
    expect(await backend.comments('b', 'c')).toHaveLength(1);
    expect(await backend.attachments('b', 'c')).toHaveLength(1);
    expect((await backend.fileBytes('b', 'file')).size).toBe(12);
    expect(backend.fileUrl('b', 'file')).toBe('blob:example');
    await backend.removeMember('b', 'reader');
    expect(await statusOf(backend.card('b', 'c'))).toBe(404);
    expect(backend.fileUrl('b', 'file')).toBeUndefined();
  });

  it('lets nonmembers read an open board while refusing writes until they join', async () => {
    const backend = inviteOnlyBoard();
    const board = backend.world.containers.get('b');
    if (!board) throw new Error('missing test board');
    board.visibility = 'open';
    expect(await backend.views('b')).toHaveLength(1);
    expect(await backend.cards('b')).toHaveLength(1);
    expect((await backend.card('b', 'c')).id).toBe('c');
    expect((await backend.cardPreviews('b')).size).toBe(1);
    expect(await backend.comments('b', 'c')).toHaveLength(1);
    expect(await backend.attachments('b', 'c')).toHaveLength(1);
    expect((await backend.fileBytes('b', 'file')).size).toBe(12);
    expect(backend.fileUrl('b', 'file')).toBe('blob:example');
    expect(await statusOf(backend.createCard('b', { title: 'Nope' }))).toBe(403);
    expect(await statusOf(backend.duplicateContainer('b', false, board.workspaceId))).toBe(403);
  });
});
