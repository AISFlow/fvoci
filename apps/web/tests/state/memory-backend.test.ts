/**
 * The in-memory backend behaves the way the contract says the server does,
 * as far as a screen can tell.
 */
import { describe, expect, it, vi } from 'vitest';

import type { DocNode } from '$lib/editor/schema.ts';
import { BackendError, isConflict, type Change } from '$lib/state/index.ts';
import { LIMITS } from '$lib/state/memory/backend.ts';

import { loadTemplates, settle, setup, STATUS, text } from './support.ts';

async function firstBoard(made = setup()) {
  const [workspace] = await made.backend.workspaces();
  const [board] = await made.backend.boards(workspace?.id ?? '');
  if (!board) throw new Error('no board');
  const cards = await made.backend.cards(board.id);
  const byTitle = (title: string) => {
    const card = cards.find((each) => each.title === title);
    if (!card) throw new Error(`no card ${title}`);
    return card;
  };
  return { ...made, boardId: board.id, cards, byTitle };
}

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('the call did not reject');
};

describe('reading', () => {
  it('makes identifiers of its own, shaped like a UUID of version 7', async () => {
    const { cards, boardId } = await firstBoard();
    const shape = /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
    expect(boardId).toMatch(shape);
    for (const card of cards) expect(card.id).toMatch(shape);
  });

  it('lists cards without their bodies, with what a list shows of them', async () => {
    const { byTitle, backend, boardId } = await firstBoard();
    const alpha = byTitle('Alpha');
    expect(alpha).not.toHaveProperty('body');
    expect(alpha.hasBody).toBe(true);
    expect(byTitle('Beta').hasBody).toBe(false);
    expect((await backend.card(boardId, alpha.id)).body).toEqual(text('first body'));
  });

  it('answers 404 for what is not there', async () => {
    const { backend, boardId } = await firstBoard();
    expect((await rejection(backend.container('nope'))).status).toBe(404);
    expect((await rejection(backend.card(boardId, 'nope'))).status).toBe(404);
  });

  it('hands out copies: changing an answer changes nothing stored', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const card = await backend.card(boardId, byTitle('Alpha').id);
    card.title = 'scribbled';
    card.fields.properties[STATUS] = 'done';
    const again = await backend.card(boardId, card.id);
    expect(again.title).toBe('Alpha');
    expect(again.fields.properties[STATUS]).toBe('todo');
  });

  it('lists the shared organizations the user is a member of and hands out copies', async () => {
    const { world, backend } = setup();
    expect(await backend.organizations()).toEqual([]);
    world.organizations.set(backend.userId, [{ id: 'org-1', name: 'Acme', role: 'admin' }]);
    const listed = await backend.organizations();
    expect(listed).toEqual([{ id: 'org-1', name: 'Acme', role: 'admin' }]);
    if (listed[0]) listed[0].name = 'Changed';
    expect(await backend.organizations()).toEqual([{ id: 'org-1', name: 'Acme', role: 'admin' }]);
  });
});

describe('writing a body', () => {
  it('stores it when the card is as the writer read it', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const card = await backend.card(boardId, byTitle('Beta').id);
    const stored = await backend.writeCardBody(boardId, card.id, {
      body: text('hello'),
      readAt: card.updatedAt,
    });
    expect(stored.body).toEqual(text('hello'));
    expect(stored.updatedAt).toBeGreaterThan(card.updatedAt);
    expect(stored.hasBody).toBe(true);
  });

  it('refuses a write based on an older version with 409', async () => {
    const { backend, other, boardId, byTitle } = await firstBoard();
    const card = await backend.card(boardId, byTitle('Alpha').id);
    await other.writeCardBody(boardId, card.id, { body: text('theirs'), readAt: card.updatedAt });
    const error = await rejection(
      backend.writeCardBody(boardId, card.id, { body: text('mine'), readAt: card.updatedAt }),
    );
    expect(isConflict(error)).toBe(true);
    expect(error.status).toBe(409);
    // The server's stale write has no reason either: the status says it.
    expect(error).toMatchObject({ type: '/problems/conflict', reason: undefined });
    expect((await backend.card(boardId, card.id)).body).toEqual(text('theirs'));
  });

  it('refuses a document that is not on the allow-list with 400 and the field', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const card = await backend.card(boardId, byTitle('Alpha').id);
    const error = await rejection(
      backend.writeCardBody(boardId, card.id, {
        body: { type: 'doc', content: [{ type: 'script' }] },
        readAt: card.updatedAt,
      }),
    );
    expect(error.status).toBe(400);
    expect(error.errors).toEqual([{ pointer: '/body', detail: error.message, reason: 'invalid' }]);
    expect(error.refusalAt('/body')?.reason).toBe('invalid');
  });

  it('refuses an image that names a file of no board, or of another one', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const card = await backend.card(boardId, byTitle('Alpha').id);
    const image = { type: 'doc', content: [{ type: 'image', attrs: { fileId: 'nowhere' } }] };
    const error = await rejection(
      backend.writeCardBody(boardId, card.id, { body: image, readAt: card.updatedAt }),
    );
    expect(error.status).toBe(400);
  });

  it('refuses a body over the limit with 400 naming the body, as the server does', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const card = await backend.card(boardId, byTitle('Alpha').id);
    const error = await rejection(
      backend.writeCardBody(boardId, card.id, {
        // Three bytes a letter: a letter past the limit.
        body: text('가'.repeat(Math.floor(LIMITS.bodyBytes / 3) + 1)),
        readAt: card.updatedAt,
      }),
    );
    expect(error).toMatchObject({ status: 400, errors: [{ pointer: '/body' }] });
    expect(error.limit).toBe(LIMITS.bodyBytes);
  });

  it('stores an empty paragraph as no body', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const card = await backend.card(boardId, byTitle('Alpha').id);
    const stored = await backend.writeCardBody(boardId, card.id, {
      body: { type: 'doc', content: [{ type: 'paragraph' }] },
      readAt: card.updatedAt,
    });
    expect(stored.body).toBeNull();
    expect(stored.hasBody).toBe(false);
  });
});

describe('deleting', () => {
  it('moves a card and what is under it to a trash they come back from', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const alpha = byTitle('Alpha');
    const before = await backend.comments(boardId, alpha.id);
    expect(before.length).toBeGreaterThan(0);
    await backend.batchDeleteCards(boardId, [alpha.id]);
    expect((await rejection(backend.card(boardId, alpha.id))).status).toBe(404);
    await backend.batchRestoreCards(boardId, [alpha.id]);
    expect((await backend.card(boardId, alpha.id)).title).toBe('Alpha');
    expect(await backend.comments(boardId, alpha.id)).toHaveLength(before.length);
  });

  it('answers 404 when asked to restore what is not in the trash', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    expect(
      (await rejection(backend.batchRestoreCards(boardId, [byTitle('Alpha').id]))).status,
    ).toBe(404);
  });

  it('takes a board out of every sidebar and puts it back with its blocks', async () => {
    const { backend, boardId } = await firstBoard();
    const [workspace] = await backend.workspaces();
    await backend.deleteContainer(boardId);
    expect(await backend.boards(workspace?.id ?? '')).toHaveLength(0);
    await backend.restoreContainer(boardId);
    expect(await backend.cards(boardId)).toHaveLength(3);
  });
});

describe('settings', () => {
  it('always answers with every member, and resets one that is set to null', async () => {
    const { backend } = setup();
    const start = await backend.settings();
    expect(Object.keys(start).sort()).toEqual(
      [
        'theme',
        'cardOpen',
        'density',
        'hourCycle',
        'timeZone',
        'haptics',
        'weekStart',
        'language',
        'notifyMentions',
        'notifyAssignments',
        'notifyComments',
        'notifyDue',
      ].sort(),
    );
    const changed = await backend.patchSettings({ theme: 'dark', timeZone: 'Asia/Seoul' });
    expect(changed).toMatchObject({ theme: 'dark', timeZone: 'Asia/Seoul', density: 'auto' });
    expect((await backend.patchSettings({ theme: null })).theme).toBe('auto');
  });

  it('refuses a time zone it does not know, and a value outside the set', async () => {
    const { backend } = setup();
    const zone = await rejection(backend.patchSettings({ timeZone: 'Mars/Olympus' }));
    expect(zone.status).toBe(400);
    expect(zone.errors.map((entry) => entry.pointer)).toEqual(['/timeZone']);
    const density = await rejection(
      backend.patchSettings({ density: 'huge' as unknown as 'auto' }),
    );
    expect(density.errors.map((entry) => entry.pointer)).toEqual(['/density']);
    const notify = await rejection(
      backend.patchSettings({ notifyComments: 'off' as unknown as boolean }),
    );
    expect(notify.errors.map((entry) => entry.pointer)).toEqual(['/notifyComments']);
  });
});

describe('telling about changes', () => {
  it('tells everybody who watches the board, the writer included', async () => {
    const { backend, other, boardId, byTitle } = await firstBoard();
    const mine: Change[] = [];
    const theirs: Change[] = [];
    const stopMine = backend.watchContainer(boardId, (change) => mine.push(change));
    other.watchContainer(boardId, (change) => theirs.push(change));
    await other.patchCard(boardId, byTitle('Alpha').id, { title: 'Alpha!' });
    await settle();
    for (const seen of [mine, theirs]) {
      expect(seen).toHaveLength(1);
      expect(seen[0]).toMatchObject({ kind: 'blocks', blocks: [{ title: 'Alpha!' }] });
    }
    stopMine();
    await other.patchCard(boardId, byTitle('Alpha').id, { title: 'Alpha?' });
    await settle();
    expect(mine).toHaveLength(1);
    expect(theirs).toHaveLength(2);
  });
});

describe('files', () => {
  it('takes a file in, tells the progress, and gives an address for it', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const seen: number[] = [];
    const file = new File(['hello'], 'hello.txt', { type: 'text/plain' });
    const uploaded = await backend.upload(boardId, file, {
      onProgress: ({ sentBytes }) => seen.push(sentBytes),
    });
    expect(uploaded).toMatchObject({ name: 'hello.txt', size: 5, contentType: 'text/plain' });
    expect(seen.at(-1)).toBe(5);
    expect(backend.fileUrl(boardId, uploaded.fileId)).toMatch(/^blob:/);
    const attachment = await backend.createAttachment(boardId, byTitle('Beta').id, uploaded);
    expect(attachment).toMatchObject({ type: 'attachment', name: 'hello.txt', size: 5 });
  });

  it('stops an upload that was given up', async () => {
    const { backend, boardId } = await firstBoard();
    const controller = new AbortController();
    controller.abort();
    const error = await rejection(
      backend.upload(boardId, new File(['x'], 'x.txt'), { signal: controller.signal }),
    );
    expect(error.status).toBe(0);
  });

  it('leaves the failure asked for the next upload to that one, not to one given up meanwhile', async () => {
    const { backend, boardId } = await firstBoard();
    const controller = new AbortController();
    backend.latency = 30;
    const givenUp = rejection(
      backend.upload(boardId, new File(['x'], 'slow.txt'), { signal: controller.signal }),
    );
    controller.abort();
    backend.failNext('upload', new BackendError(413, { limit: 1 }));
    // The one given up ends as given up, although the failure was waiting when its turn came.
    expect((await givenUp).status).toBe(0);
    backend.latency = 0;
    const refused = await rejection(backend.upload(boardId, new File(['y'], 'next.txt')));
    expect(refused.status).toBe(413);
  });
});

describe('file blocks', () => {
  const withFile = (fileId: string): DocNode => ({
    type: 'doc',
    content: [{ type: 'file', attrs: { fileId } }],
  });

  it('reads the name and size of a file of the board, and no file of another', async () => {
    const made = setup();
    const { backend, boardId } = await firstBoard(made);
    const uploaded = await backend.upload(boardId, new File(['%PDF'], 'minutes.pdf'));
    expect(await backend.fileDetails(boardId, uploaded.fileId)).toEqual({
      name: 'minutes.pdf',
      size: 4,
    });
    expect((await rejection(backend.fileDetails(boardId, 'nowhere'))).status).toBe(404);
    const [workspace] = await backend.workspaces();
    const other = (await backend.boards(workspace?.id ?? '')).find((one) => one.id !== boardId);
    expect((await rejection(backend.fileDetails(other?.id ?? '', uploaded.fileId))).status).toBe(
      404,
    );
  });

  it('stores a card body naming a file of the board, and refuses one of no board', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const uploaded = await backend.upload(boardId, new File(['%PDF'], 'minutes.pdf'));
    const card = await backend.card(boardId, byTitle('Alpha').id);
    const written = await backend.writeCardBody(boardId, card.id, {
      body: withFile(uploaded.fileId),
      readAt: card.updatedAt,
    });
    expect(written.body).toEqual(withFile(uploaded.fileId));
    const error = await rejection(
      backend.writeCardBody(boardId, card.id, {
        body: withFile('nowhere'),
        readAt: written.updatedAt,
      }),
    );
    expect(error).toMatchObject({ status: 400, errors: [{ pointer: '/body' }] });
  });

  it('refuses a comment with a file block, written or changed, as the server does', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const uploaded = await backend.upload(boardId, new File(['%PDF'], 'minutes.pdf'));
    const cardId = byTitle('Alpha').id;
    const refused = await rejection(
      backend.createComment(boardId, cardId, withFile(uploaded.fileId)),
    );
    expect(refused).toMatchObject({ status: 400, errors: [{ pointer: '/body' }] });
    const comment = await backend.createComment(boardId, cardId, text('fine'));
    const changed = await rejection(
      backend.patchComment(
        boardId,
        comment.id,
        { body: withFile(uploaded.fileId), readAt: comment.updatedAt },
        cardId,
      ),
    );
    expect(changed).toMatchObject({ status: 400, errors: [{ pointer: '/body' }] });
  });

  it('refuses an archive whose comment has a file block, whole, as the server does', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const exported = await backend.exportContainer(boardId);
    const comment = {
      type: 'block',
      data: {
        id: 'comment-with-a-file',
        containerId: boardId,
        parentId: byTitle('Alpha').id,
        type: 'comment',
        title: '',
        fields: {},
        createdAt: 1,
        updatedAt: 1,
        body: withFile('7x.pdf'),
      },
    };
    const archive = `${await exported.bytes.text()}${JSON.stringify(comment)}
`;
    const [workspace] = await backend.workspaces();
    const workspaceId = workspace?.id ?? '';
    const before = (await backend.boards(workspaceId)).length;
    const refused = await rejection(
      backend.importArchive(workspaceId, new File([archive], 'with-a-file.fvoci')),
    );
    expect(refused).toMatchObject({ status: 400, errors: [{ pointer: '/body' }] });
    expect(await backend.boards(workspaceId)).toHaveLength(before);
  });

  it('clears deletedAt on containers and blocks imported from an archive', async () => {
    const { backend, boardId } = await firstBoard();
    const exported = await backend.exportContainer(boardId);
    const stamped = (await exported.bytes.text())
      .trim()
      .split('\n')
      .map((line) => {
        const entry = JSON.parse(line) as { type: string; data?: Record<string, unknown> };
        if ((entry.type === 'container' || entry.type === 'block') && entry.data) {
          entry.data.deletedAt = 99_999;
        }
        return JSON.stringify(entry);
      })
      .join('\n');
    const [workspace] = await backend.workspaces();
    const workspaceId = workspace?.id ?? '';
    const beforeIds = new Set((await backend.boards(workspaceId)).map((b) => b.id));
    await backend.importArchive(workspaceId, new File([`${stamped}\n`], 'trashed.fvoci'));
    const imported = (await backend.boards(workspaceId)).find((b) => !beforeIds.has(b.id));
    expect(imported).toBeDefined();
    expect(imported?.deletedAt).toBeUndefined();
    const cards = await backend.cards(imported?.id ?? '');
    expect(cards.length).toBeGreaterThan(0);
    for (const card of cards) {
      expect(card.deletedAt).toBeUndefined();
    }
  });

  it('gives a copy of the board files of its own for its file blocks, with the same bytes', async () => {
    const { backend, boardId, byTitle } = await firstBoard();
    const uploaded = await backend.upload(boardId, new File(['%PDF'], 'minutes.pdf'));
    const card = await backend.card(boardId, byTitle('Alpha').id);
    await backend.writeCardBody(boardId, card.id, {
      body: withFile(uploaded.fileId),
      readAt: card.updatedAt,
    });
    const [workspace] = await backend.workspaces();
    const copy = await backend.duplicateContainer(boardId, false, workspace?.id ?? '');
    const copied = (await backend.cards(copy.id)).find((one) => one.title === 'Alpha');
    const body = (await backend.card(copy.id, copied?.id ?? '')).body;
    const [node] = body?.content ?? [];
    const fileId = String(node?.attrs?.fileId);
    expect(node?.type).toBe('file');
    expect(fileId).not.toBe(uploaded.fileId);
    expect(await backend.fileDetails(copy.id, fileId)).toEqual({ name: 'minutes.pdf', size: 4 });
  });
});

describe('the name of an upload', () => {
  it('is refused as the server refuses it where it names no file', async () => {
    const { backend, boardId } = await firstBoard();
    const longest = '가'.repeat(255);
    // U+0085 is blank to the server and not to `trim`; U+FEFF the other way round.
    for (const name of ['dir/', 'dir\\', ' ', '\u0085', '.', 'dir/..', 'a\0.txt', `${longest}가`]) {
      const refused = await rejection(backend.upload(boardId, new File(['x'], name)));
      expect(refused.status, JSON.stringify(name)).toBe(400);
    }
    // Kept as the server keeps it: what follows the last `/`.
    for (const [name, kept] of [
      ['dir/a.txt', 'a.txt'],
      ['dir\\a.txt', 'dir\\a.txt'],
      ['...', '...'],
      ['\uFEFF', '\uFEFF'],
      [longest, longest],
    ] as const) {
      const uploaded = await backend.upload(boardId, new File(['x'], name));
      expect((await backend.fileDetails(boardId, uploaded.fileId)).name).toBe(kept);
    }
  });
});

describe('a copy of a board', () => {
  it('is titled as the server does: in the language of the settings, or else of the browser', async () => {
    const { backend, boardId } = await firstBoard();
    const [workspace] = await backend.workspaces();
    const to = workspace?.id ?? '';
    const source = (await backend.boards(to)).find((board) => board.id === boardId)?.title;
    expect(source).toBeTruthy();
    const browser = vi.spyOn(navigator, 'languages', 'get');
    try {
      browser.mockReturnValue(['ja']);
      expect((await backend.duplicateContainer(boardId, false, to)).title).toBe(`${source} copy`);
      const template = await backend.duplicateContainer(boardId, true, to);
      expect(template.title).toBe('New board template');
      expect((await backend.duplicateContainer(template.id, false, to)).title).toBe(template.title);

      browser.mockReturnValue(['ja', 'ko-KR', 'en']);
      expect((await backend.duplicateContainer(boardId, false, to)).title).toBe(`${source} 사본`);
      expect((await backend.duplicateContainer(boardId, true, to)).title).toBe('새 보드 템플릿');
      await backend.patchSettings({ language: 'de' });
      expect((await backend.duplicateContainer(boardId, false, to)).title).toBe(`${source} 사본`);

      await backend.patchSettings({ language: 'en-GB' });
      expect((await backend.duplicateContainer(boardId, false, to)).title).toBe(`${source} copy`);
      browser.mockReturnValue(['en']);
      await backend.patchSettings({ language: 'ko' });
      expect((await backend.duplicateContainer(boardId, false, to)).title).toBe(`${source} 사본`);
    } finally {
      browser.mockRestore();
    }
  });

  it('refuses a view patch whose readAt is stale (409), forwards queued writes, and skips check without readAt', async () => {
    const { backend, other, boardId } = await firstBoard();
    const [view] = await backend.views(boardId);
    if (!view) throw new Error('missing view');
    // Matching readAt succeeds and advances updatedAt.
    const first = await backend.patchView(boardId, view.id, {
      updatedFields: { order: 'V' },
      readAt: view.updatedAt,
    });
    expect(first.updatedAt).toBeGreaterThan(view.updatedAt);
    // Stale readAt from another user is refused with 409, same shape as the server.
    const conflict = await rejection(
      other.patchView(boardId, view.id, {
        updatedFields: { order: 'W' },
        readAt: view.updatedAt,
      }),
    );
    expect(conflict).toMatchObject({
      status: 409,
      type: '/problems/conflict',
      message: 'the block has changed since it was read',
    });
    // Without readAt, a write is unconditional.
    const renamed = await other.patchView(boardId, view.id, { title: 'Renamed' });
    expect(renamed.title).toBe('Renamed');
    // Two queued writes on the same backend forward readAt along the queue.
    const [a, b] = await Promise.all([
      backend.patchView(boardId, view.id, {
        updatedFields: { order: 'W' },
        readAt: renamed.updatedAt,
      }),
      backend.patchView(boardId, view.id, {
        updatedFields: { order: 'X' },
        readAt: renamed.updatedAt,
      }),
    ]);
    expect(a.fields.order).toBe('W');
    expect(b.fields.order).toBe('X');
  });
});

describe('the seed made of the boards of `seed/`', () => {
  it('holds every English board as a live board the reader is on, plus localized built-in templates', async () => {
    const templates = await loadTemplates();
    expect(templates).toHaveLength(18);
    const { backend, world } = setup(templates);
    const [workspace] = await backend.workspaces();
    const workspaceId = workspace?.id ?? '';
    const boards = await backend.boards(workspaceId);
    expect(boards).toHaveLength(7);
    const cards = (await Promise.all(boards.map((board) => backend.cards(board.id)))).flat();
    expect(cards).toHaveLength(44);
    expect(cards.filter((card) => card.hasBody)).toHaveLength(44);
    expect(new Set([...world.blocks.keys(), ...world.containers.keys()]).size).toBe(
      world.blocks.size + world.containers.size,
    );
    const courses = await backend.course.courses(workspaceId);
    expect(courses).toHaveLength(1);
    const categories = await backend.categories(workspaceId);
    expect(categories.flatMap((category) => category.containerIds).sort()).toEqual(
      [...boards, ...courses].map((container) => container.id).sort(),
    );
  });

  it('filters built-in templates and onboarding by the reader’s language (en, ko, and fallbacks)', async () => {
    const spy = vi.spyOn(navigator, 'languages', 'get');
    try {
      spy.mockReturnValue(['ja']);
      const { backend } = setup(await loadTemplates());
      const [workspace] = await backend.workspaces();
      const workspaceId = workspace?.id ?? '';

      // Unsupported browser language under 'auto' falls back to English.
      const enTemplates = await backend.templates(workspaceId);
      expect(enTemplates).toHaveLength(8);
      expect(
        enTemplates.every((t) => t.isBuiltin === true && t.properties.templateLanguage === 'en'),
      ).toBe(true);
      expect(enTemplates.map((t) => t.properties.templateKey)).toEqual([
        'welcome',
        'project-schedule',
        'issue-tracker',
        'meeting-notes',
        'content-calendar',
        'customer-contacts',
        'study-notes',
        'course-study-plan',
      ]);
      const enWelcomeId = await backend.onboard(workspaceId);
      const enWelcome = await backend.container(enWelcomeId);
      expect(enWelcome.title).toBe('Getting started with fvoci');
      expect(enWelcome.isTemplate).toBe(false);
      expect(enWelcome.isBuiltin).toBe(false);
      expect(enWelcome.properties.onboarding).toBeUndefined();
      expect(enWelcome.properties.templateLanguage).toBeUndefined();
      expect(enWelcome.properties.templateKey).toBeUndefined();

      // Regional browser tag ('ko-KR') under 'auto' selects Korean.
      spy.mockReturnValue(['ja', 'ko-KR', 'en']);
      const autoKoTemplates = await backend.templates(workspaceId);
      expect(autoKoTemplates).toHaveLength(8);
      expect(autoKoTemplates.every((t) => t.properties.templateLanguage === 'ko')).toBe(true);
      expect(autoKoTemplates[0]?.title).toBe('fvoci 시작하기');

      // Explicit unsupported language ('de') falls back to the browser list ('ko-KR' -> 'ko').
      await backend.patchSettings({ language: 'de' });
      const fallbackKo = await backend.templates(workspaceId);
      expect(fallbackKo.every((t) => t.properties.templateLanguage === 'ko')).toBe(true);

      // Regional explicit setting ('en-GB') selects English even when browser prefers Korean.
      await backend.patchSettings({ language: 'en-GB' });
      const regionalEn = await backend.templates(workspaceId);
      expect(regionalEn.every((t) => t.properties.templateLanguage === 'en')).toBe(true);

      await backend.patchSettings({ language: 'ko' });
      const koTemplates = await backend.templates(workspaceId);
      expect(koTemplates).toHaveLength(8);
      expect(
        koTemplates.every((t) => t.isBuiltin === true && t.properties.templateLanguage === 'ko'),
      ).toBe(true);
      expect(koTemplates[0]?.title).toBe('fvoci 시작하기');
      expect(koTemplates.at(-1)?.title).toBe('2주 집중 학습 계획');
      const koWelcomeId = await backend.onboard(workspaceId);
      const koWelcome = await backend.container(koWelcomeId);
      expect(koWelcome.title).toBe('fvoci 시작하기');
      expect(koWelcome.isTemplate).toBe(false);
      expect(koWelcome.isBuiltin).toBe(false);
    } finally {
      spy.mockRestore();
    }
  });

  it('comes out the same every time', async () => {
    const templates = await loadTemplates();
    const dump = (): string => {
      const { world } = setup(templates);
      return JSON.stringify([[...world.containers], [...world.blocks], [...world.categories]]);
    };
    expect(dump()).toBe(dump());
  });
});
