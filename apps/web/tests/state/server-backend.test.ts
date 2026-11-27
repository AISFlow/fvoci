/**
 * The backend that calls the server, against a stand-in for `fetch`: what
 * it sends, and how the server's shapes become the screens'. What the real
 * server does with it is the work of `e2e-live/`.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { serverBackend } from './support.ts';
import { cardOfBlock } from '$lib/api/convert.ts';
import { Http } from '$lib/api/http.ts';
import { isBackendError } from '$lib/state/index.ts';

interface Sent {
  url: string;
  method: string;
  headers: Record<string, string>;
  body: unknown;
}

/** Answers every request with `answer`, and keeps what was sent. */
function serve(answer: unknown, status = 200): Sent[] {
  const sent: Sent[] = [];
  vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
    sent.push({
      url,
      method: init.method ?? 'GET',
      headers: init.headers as Record<string, string>,
      body: typeof init.body === 'string' ? JSON.parse(init.body) : undefined,
    });
    return Promise.resolve(
      new Response(status === 204 ? null : JSON.stringify(answer), {
        status,
        // An error is the server's problem details (RFC 9457).
        headers: {
          'Content-Type': status >= 400 ? 'application/problem+json' : 'application/json',
        },
      }),
    );
  });
  return sent;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

const BLOCK = {
  containerId: 'b',
  body: null,
  parentId: 'c',
  createdAt: 1,
  updatedAt: 2,
  createdBy: 'u',
  modifiedBy: 'u',
  fields: {},
};

describe('the server backend', () => {
  it('signs in with the text as the name and as the email address, and says it is a script', async () => {
    const sent = serve(undefined, 204);
    await serverBackend('/base').signIn('ann@home', 'secret pass');
    expect(sent).toHaveLength(1);
    expect(sent[0]?.url).toBe('/base/api/v1/login');
    expect(sent[0]?.body).toEqual({
      username: 'ann@home',
      email: 'ann@home',
      password: 'secret pass',
    });
    expect(sent[0]?.headers['X-Requested-With']).toBe('XMLHttpRequest');
  });

  it('lists the shared organizations the reader is a member of', async () => {
    const listed = [{ id: 'org-1', name: 'Acme', role: 'admin' }];
    const sent = serve(listed);
    expect(await serverBackend('/base').organizations()).toEqual(listed);
    expect(sent.map(({ method, url }) => ({ method, url }))).toEqual([
      { method: 'GET', url: '/base/api/v1/users/me/organizations' },
    ]);
  });

  it("reads the reader's own address and the one the account waits for, and asks to change it with the password", async () => {
    const user = { id: 'u', username: 'ada', displayName: 'Ada', createdAt: 1, updatedAt: 2 };
    serve({ ...user, hasPassword: true, email: 'a@old.example' });
    const backend = serverBackend('/base');
    const alone = await backend.me();
    expect(alone).toMatchObject({ email: 'a@old.example', hasPassword: true });
    expect(alone.pendingEmail).toBeUndefined();
    serve({
      ...user,
      hasPassword: true,
      email: 'a@old.example',
      pendingEmail: 'a@new.example',
    });
    expect(await backend.me()).toMatchObject({ pendingEmail: 'a@new.example' });

    const sent = serve(undefined, 204);
    await backend.emailChange.request('a@new.example', 'secret pass', 'ko');
    await backend.emailChange.resend('ko');
    await backend.emailChange.cancel();
    await backend.emailChange.confirm('the token');
    expect(sent.map(({ method, url, body }) => ({ method, url, body }))).toEqual([
      {
        method: 'POST',
        url: '/base/api/v1/users/me/email-change',
        body: { email: 'a@new.example', password: 'secret pass', language: 'ko' },
      },
      {
        method: 'POST',
        url: '/base/api/v1/users/me/email-change/resend',
        body: { language: 'ko' },
      },
      { method: 'DELETE', url: '/base/api/v1/users/me/email-change', body: undefined },
      // The token is in the body: an address is logged, and kept in histories.
      { method: 'POST', url: '/base/api/v1/email-change/confirm', body: { token: 'the token' } },
    ]);
  });

  it('keeps its connection as the session gives its password, and asks on it again for its workspace once that is entered again', async () => {
    const made: Socket[] = [];
    class Socket extends EventTarget {
      readonly sent: unknown[] = [];
      closed = false;
      constructor() {
        super();
        made.push(this);
      }
      send(data: string): void {
        this.sent.push(JSON.parse(data));
      }
      close(): void {
        this.closed = true;
      }
    }
    vi.stubGlobal('WebSocket', Socket);
    serve(undefined, 204);
    const backend = serverBackend('');
    const unwatch = backend.watchSelf(() => undefined);
    await vi.waitFor(() => {
      expect(made).toHaveLength(1);
    });
    const socket = made[0];
    socket?.dispatchEvent(new Event('open'));
    const subscribe = { type: 'subscribeWorkspace', workspaceId: 'w1' };
    expect(socket?.sent).toEqual([subscribe]);
    // The account was given its first password: the server ends what the session follows
    // of its own space, and ignores the asking again that the page answers with.
    const ended = JSON.stringify({ type: 'workspaceUnsubscribed', workspaceId: 'w1' });
    socket?.dispatchEvent(new MessageEvent('message', { data: ended }));
    expect(socket?.sent).toEqual([subscribe, subscribe]);

    await backend.confirmPassword('a long password');
    await new Promise((done) => setTimeout(done, 20));
    // No new connection: the server takes this one as the session's that it is now.
    expect(made).toHaveLength(1);
    expect(socket?.closed).toBe(false);
    // The workspace entered again (`Workspace.enter`): what was ignored is asked for again.
    backend.enter('w1');
    expect(socket?.sent).toEqual([subscribe, subscribe, subscribe]);
    unwatch();
  });

  it('asks for a board read before under its own workspace, also after entering another', async () => {
    const sent = serve({
      ...BLOCK,
      id: 'b9',
      workspaceId: 'w2',
      title: 'Elsewhere',
      visibility: 'open',
    });
    const backend = serverBackend('');
    await backend.container('b9');
    backend.enter('w3');
    await backend.views('b9').catch(() => undefined);
    await backend.patchCard('b9', 'c9', { title: 'x' }).catch(() => undefined);
    expect(sent.map((one) => one.url)).toEqual([
      '/api/v1/w/w1/containers/b9',
      '/api/v1/w/w2/boards/b9/blocks?type=view',
      '/api/v1/w/w2/cards/c9',
    ]);
  });

  it('asks once for the people by their identifiers, each once, and not for none', async () => {
    const sent = serve([{ id: 'u1', username: 'ann', email: '', createdAt: 1, updatedAt: 1 }]);
    const backend = serverBackend('');
    expect(await backend.usersById('w1', [])).toEqual([]);
    const found = await backend.usersById('w1', ['u1', 'u2', 'u1']);
    expect(found.map((user) => user.id)).toEqual(['u1']);
    expect(sent.map((one) => [one.method, one.url, one.body])).toEqual([
      ['POST', '/api/v1/w/w1/users', ['u1', 'u2']],
    ]);
  });

  it('asks for a long list of people in parts the server takes', async () => {
    const sent = serve([]);
    const ids = Array.from({ length: 450 }, (_, at) => `u${at}`);
    await serverBackend('').usersById('w1', [...ids, ...ids]);
    expect(sent.map((one) => (one.body as string[]).length)).toEqual([200, 200, 50]);
    expect(sent.flatMap((one) => one.body as string[])).toEqual(ids);
  });

  it('makes a card from a draft, its body along', async () => {
    const sent = serve({
      ...BLOCK,
      id: 'c1',
      title: 'New',
      icon: '',
      isTemplate: false,
      properties: { status: 'todo' },
      hasBody: false,
      tasksTotal: 0,
      tasksDone: 0,
    });
    const backend = serverBackend('');
    const card = await backend.createCard('b', { properties: { status: 'todo' } });
    expect(sent[0]).toMatchObject({
      url: '/api/v1/w/w1/boards/b/cards',
      method: 'POST',
      body: { title: '', icon: '', isTemplate: false, properties: { status: 'todo' } },
    });
    expect(card).toMatchObject({
      id: 'c1',
      type: 'card',
      fields: { properties: { status: 'todo' } },
    });

    const body = {
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'x' }] }],
    };
    await backend.createCard('b', { title: 'Copy', body, quiet: true });
    expect(sent[1]?.body).toMatchObject({ title: 'Copy', body });
    // A copy tells nobody whom it names or is given to; a card made anew does.
    expect(sent[1]?.url).toBe('/api/v1/w/w1/boards/b/cards?disableNotify=true');
  });

  it('reads and marks what the reader was told, of no workspace', async () => {
    const subject = {
      kind: 'comment',
      actorId: 'u2',
      times: 2,
      people: 1,
      workspaceId: 'w2',
      containerId: 'b',
      containerKind: 'board',
      blockId: 'c',
      blockKind: 'card',
      title: 'Plan',
      commentId: 'm',
    };
    const listed = [{ id: 'n1', createdAt: 1, updatedAt: 5, readAt: 0, subject }];
    const sent = serve(listed);
    const notifications = serverBackend('').notifications;
    expect(await notifications.list({ page: 1, perPage: 20 })).toEqual(listed);
    expect(sent[0]?.url).toBe('/api/v1/notifications?page=1&perPage=20');
    const marked = serve(null, 204);
    await notifications.read('n1');
    await notifications.readAll(5);
    await notifications.readAll();
    expect(marked.map(({ method, url }) => `${method} ${url}`)).toEqual([
      'POST /api/v1/notifications/n1/read',
      'POST /api/v1/notifications/read?through=5',
      'POST /api/v1/notifications/read',
    ]);
    serve({ count: 3 });
    expect(await notifications.unread()).toBe(3);
  });

  it('makes a comment via the formal card comments route', async () => {
    const body = {
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Hi' }] }],
    };
    const sent = serve({ ...BLOCK, id: 'm', type: 'comment', title: '', body }, 201);
    const comment = await serverBackend('').createComment('b', 'c', body);
    expect(sent[0]).toMatchObject({
      url: '/api/v1/w/w1/boards/b/cards/c/comments',
      method: 'POST',
      body: { body },
    });
    expect(comment).toMatchObject({ id: 'm', type: 'comment', body });
  });

  it('changes a comment via the formal card comment PATCH route with readAt', async () => {
    const body = {
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'New' }] }],
    };
    let updatedAt = 7;
    const sent: Sent[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
      const parsedBody =
        typeof init.body === 'string'
          ? (JSON.parse(init.body) as Record<string, unknown>)
          : undefined;
      sent.push({
        url,
        method: init.method ?? 'GET',
        headers: {},
        body: parsedBody,
      });
      if (init.method === 'PATCH') {
        if (parsedBody?.readAt !== updatedAt) {
          return Promise.resolve(
            new Response(
              JSON.stringify({
                type: '/problems/conflict',
                title: 'Conflict',
                status: 409,
                detail: 'stale readAt',
              }),
              { status: 409, headers: { 'Content-Type': 'application/problem+json' } },
            ),
          );
        }
        updatedAt += 2;
        return Promise.resolve(
          new Response(
            JSON.stringify({ ...BLOCK, id: 'm', type: 'comment', title: '', updatedAt, body }),
            { status: 200, headers: { 'Content-Type': 'application/json' } },
          ),
        );
      }
      return Promise.resolve(new Response(JSON.stringify([]), { status: 200 }));
    });
    const backend = serverBackend('');
    const changed = await backend.patchComment('b', 'm', { body, readAt: 7 }, 'c');
    expect(sent.map((one) => `${one.method} ${one.url}`)).toEqual([
      'PATCH /api/v1/w/w1/boards/b/cards/c/comments/m',
    ]);
    expect(sent[0]?.body).toEqual({ body, readAt: 7 });
    expect(changed).toMatchObject({ id: 'm', body, updatedAt: 9 });

    // Back-to-back edit using the returned updatedAt succeeds without self-conflict 409.
    const second = await backend.patchComment('b', 'm', { body, readAt: changed.updatedAt }, 'c');
    expect(second.updatedAt).toBe(11);

    // No text is no comment: refused before anything is sent.
    const none = serve([]);
    const empty = await serverBackend('')
      .patchComment('b', 'm', { body: null, readAt: 1 }, 'c')
      .catch((error: unknown) => error);
    expect(isBackendError(empty) && empty.status).toBe(400);
    expect(none).toHaveLength(0);

    // Server refusal (404 / 409) propagates in one round-trip.
    const other = serve(
      { type: '/problems/not-found', title: 'Not found', status: 404, detail: 'no comment' },
      404,
    );
    const refused = await serverBackend('')
      .patchComment('b', 'm', { body, readAt: 1 }, 'c')
      .catch((error: unknown) => error);
    expect(isBackendError(refused) && refused.status).toBe(404);
    expect(other.map((one) => one.method)).toEqual(['PATCH']);
  });

  it('reads the earlier texts of a comment via its formal versions route', async () => {
    const body = {
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Before' }] }],
    };
    const sent = serve([
      { version: 1, body, writtenBy: 'u1', writtenAt: 5 },
      { version: 3, body: null, writtenBy: 'u2', writtenAt: 8 },
    ]);
    expect(await serverBackend('').commentVersions('b', 'm', 'c')).toEqual([
      { version: 1, body, writtenBy: 'u1', writtenAt: 5 },
      { version: 3, body: null, writtenBy: 'u2', writtenAt: 8 },
    ]);
    expect(sent.map((one) => `${one.method} ${one.url}`)).toEqual([
      'GET /api/v1/w/w1/boards/b/cards/c/comments/m/versions',
    ]);

    serve([{ ...BLOCK, id: 'n', type: 'comment', title: '', body, editedAt: 9, modifiedBy: 'u2' }]);
    const [comment] = await serverBackend('').comments('b', 'c');
    expect(comment).toMatchObject({ editedAt: 9, modifiedBy: 'u2' });
  });

  it("reads a card's comments from `/cards/{cardId}/comments` and attachments under it", async () => {
    const body = {
      type: 'doc',
      content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Body' }] }],
    };
    serve([{ ...BLOCK, id: 'n', type: 'comment', title: '', body }]);
    expect((await serverBackend('').comments('b', 'c'))[0]?.body).toEqual(body);
    // One stored before comments had bodies: its title, a paragraph a line.
    const sent = serve([{ ...BLOCK, id: 'm', type: 'comment', title: 'Looks good\nShip it' }]);
    const backend = serverBackend('');
    const [comment] = await backend.comments('b', 'c');
    expect(sent[0]?.url).toBe('/api/v1/w/w1/boards/b/cards/c/comments');
    expect(comment?.body).toEqual({
      type: 'doc',
      content: [
        { type: 'paragraph', content: [{ type: 'text', text: 'Looks good' }] },
        { type: 'paragraph', content: [{ type: 'text', text: 'Ship it' }] },
      ],
    });

    serve([
      {
        ...BLOCK,
        id: 'a',
        type: 'attachment',
        title: 'plan.pdf',
        fields: { fileId: 'f1', size: 120, contentType: 'application/pdf' },
      },
    ]);
    expect(await backend.attachments('b', 'c')).toMatchObject([
      { id: 'a', fileId: 'f1', name: 'plan.pdf', size: 120, contentType: 'application/pdf' },
    ]);
    // An older block names its file `attachmentId`, and says no size.
    serve([
      { ...BLOCK, id: 'o', type: 'attachment', title: 'old.png', fields: { attachmentId: 'f0' } },
    ]);
    expect(await backend.attachments('b', 'c')).toMatchObject([
      { id: 'o', fileId: 'f0', name: 'old.png', size: 0, contentType: '' },
    ]);
  });
});

describe('changing a card on the server', () => {
  const CARD = {
    ...BLOCK,
    id: 'c1',
    parentId: '',
    title: 'Card',
    icon: '',
    isTemplate: false,
    body: null,
    hasBody: false,
    tasksTotal: 0,
    tasksDone: 0,
  };

  /**
   * A server of one card whose values are `values`. It keeps what it is
   * sent as the server does: the values set are merged in, the ones named
   * cleared.
   */
  function oneCard(values: Record<string, unknown>): {
    sent: Sent[];
    card: () => typeof CARD & { properties: Record<string, unknown> };
  } {
    let card = { ...CARD, properties: values };
    const sent: Sent[] = [];
    vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
      const body =
        typeof init.body === 'string'
          ? (JSON.parse(init.body) as Record<string, unknown>)
          : undefined;
      sent.push({
        url,
        method: init.method ?? 'GET',
        headers: init.headers as Record<string, string>,
        body,
      });
      // A little wait, as a network has: two changes could pass each other.
      await new Promise((resolve) => setTimeout(resolve, 5));
      if (init.method === 'PATCH' && url.endsWith('/cards/c1')) {
        const patch = body as {
          title?: string;
          updatedProperties?: Record<string, unknown>;
          deletedProperties?: string[];
        };
        const properties = { ...card.properties, ...patch.updatedProperties };
        for (const id of patch.deletedProperties ?? []) {
          // eslint-disable-next-line @typescript-eslint/no-dynamic-delete -- the patch names them
          delete properties[id];
        }
        card = { ...card, title: patch.title ?? card.title, properties };
      }
      return new Response(JSON.stringify(card), { status: 200 });
    });
    return { sent, card: () => card };
  }

  it('sends only the title when no value changes', async () => {
    const { sent } = oneCard({ a: '1' });
    await serverBackend('').patchCard('b', 'c1', { title: 'New' });
    expect(sent.map((one) => `${one.method} ${one.url}`)).toEqual(['PATCH /api/v1/w/w1/cards/c1']);
    expect(sent[0]?.body).toEqual({ title: 'New' });
  });

  it('sends the values it sets and the ones it clears, and nothing else, without reading the card', async () => {
    const { sent, card } = oneCard({ a: '1', b: '2', c: '3' });
    const answer = await serverBackend('').patchCard('b', 'c1', {
      updatedProperties: { b: '4' },
      deletedProperties: ['a'],
    });
    expect(sent.map((one) => `${one.method} ${one.url}`)).toEqual(['PATCH /api/v1/w/w1/cards/c1']);
    expect(sent[0]?.body).toEqual({ updatedProperties: { b: '4' }, deletedProperties: ['a'] });
    expect(card().properties).toEqual({ b: '4', c: '3' });
    expect(answer.fields.properties).toEqual({ b: '4', c: '3' });
  });

  it('changes a card quietly, as undo and redo do, telling nobody whom it gives the card to', async () => {
    const { sent } = oneCard({ a: '1' });
    await serverBackend('').patchCard(
      'b',
      'c1',
      { updatedProperties: { a: '2' } },
      { quiet: true },
    );
    expect(sent.map((one) => `${one.method} ${one.url}`)).toEqual([
      'PATCH /api/v1/w/w1/cards/c1?disableNotify=true',
    ]);
  });

  it('takes away the last value by the card patch, with a new title', async () => {
    const { sent, card } = oneCard({ a: '1' });
    await serverBackend('').patchCard('b', 'c1', { title: 'T', deletedProperties: ['a'] });
    expect(sent.map((one) => [one.method, one.url, one.body])).toEqual([
      ['PATCH', '/api/v1/w/w1/cards/c1', { title: 'T', deletedProperties: ['a'] }],
    ]);
    expect(card()).toMatchObject({ title: 'T', properties: {} });
  });

  it('sends two quick changes of one card one after the other, in the order they were made', async () => {
    const { sent, card } = oneCard({});
    const backend = serverBackend('');
    await Promise.all([
      backend.patchCard('b', 'c1', { updatedProperties: { a: '1' } }),
      backend.patchCard('b', 'c1', { updatedProperties: { a: '2' } }),
    ]);
    expect(sent.map((one) => one.body)).toEqual([
      { updatedProperties: { a: '1' } },
      { updatedProperties: { a: '2' } },
    ]);
    expect(card().properties).toEqual({ a: '2' });
  });

  it('reads the badges of a card from the server, and takes a change message’s from its summary', async () => {
    serve({
      ...CARD,
      properties: {},
      hasBody: true,
      tasksTotal: 3,
      tasksDone: 1,
      lastCommentAt: 44,
    });
    const answer = await serverBackend('').patchCard('b', 'c1', { title: 'x' });
    expect(answer).toMatchObject({ hasBody: true, tasksTotal: 3, tasksDone: 1, lastCommentAt: 44 });
    expect(answer).not.toHaveProperty('body');
    // The block of a change message has no body; its summary says how its tasks stand.
    const block = cardOfBlock(
      {
        ...BLOCK,
        id: 'c2',
        type: 'card',
        title: 'With tasks',
        fields: { properties: {} },
        body: null,
      },
      { hasBody: true, tasksTotal: 2, tasksDone: 1, preview: { imageFileId: '', text: '' } },
    );
    expect(block).toMatchObject({ hasBody: true, tasksTotal: 2, tasksDone: 1 });
    expect(block).not.toHaveProperty('body');
    // A change message does not say when the last comment was posted.
    expect(block.lastCommentAt).toBeUndefined();
  });
});

describe('joining an open board on the server', () => {
  it('asks to join by the board and gives the membership', async () => {
    const sent = serve({
      containerId: 'b',
      userId: 'u',
      role: 'editor',
    });
    const member = await serverBackend('').joinContainer('b');
    expect(sent.map((one) => `${one.method} ${one.url}`)).toEqual([
      'POST /api/v1/w/w1/containers/b/join',
    ]);
    expect(member).toMatchObject({ containerId: 'b', userId: 'u', role: 'editor' });
  });
});

describe('the problems of the server', () => {
  it('reads a refusal as the problem details the server sends', async () => {
    serve(
      {
        type: '/problems/username-taken',
        title: 'User name taken',
        status: 409,
        detail: 'the user name is taken',
        instance: 'urn:uuid:r1',
        reason: 'username_taken',
        errors: [
          { pointer: '/username', detail: 'the user name is taken', reason: 'username_taken' },
          { parameter: 'page', detail: 'no number', reason: 'invalid' },
          { pointer: '/x' },
          'nothing',
        ],
        limit: 7,
        retryAfter: 3,
        requestId: 'r1',
      },
      409,
    );
    const refused = await serverBackend('')
      .me()
      .catch((error: unknown) => error);
    if (!isBackendError(refused)) throw new Error('not refused');
    expect(refused).toMatchObject({
      status: 409,
      type: '/problems/username-taken',
      reason: 'username_taken',
      message: 'the user name is taken',
      limit: 7,
      retryAfter: 3,
      requestId: 'r1',
    });
    // An entry that does not say why is no entry.
    expect(refused.errors).toEqual([
      { pointer: '/username', detail: 'the user name is taken', reason: 'username_taken' },
      { parameter: 'page', detail: 'no number', reason: 'invalid' },
    ]);
    expect(refused.refusalAt('/username')?.reason).toBe('username_taken');
    expect(refused.refusalAt('/email')).toBeUndefined();
  });

  it("goes by the status alone when an error answer is not the server's", async () => {
    // A proxy's page, or the old shape: nothing of it is read.
    vi.stubGlobal('fetch', () =>
      Promise.resolve(
        new Response(JSON.stringify({ error: 'conflict', message: 'x', reason: 'r', field: 'f' }), {
          status: 502,
          headers: { 'Content-Type': 'application/json' },
        }),
      ),
    );
    const refused = await serverBackend('')
      .me()
      .catch((error: unknown) => error);
    if (!isBackendError(refused)) throw new Error('not refused');
    expect(refused).toMatchObject({ status: 500, type: 'about:blank', reason: undefined });
    expect(refused.errors).toEqual([]);
  });
});

describe('changing a view on the server', () => {
  it('patches its block in one PATCH request without extra GET round-trips', async () => {
    const sent = serve(undefined, 204);
    const view = await serverBackend('').patchView('b', 'v', {
      updatedFields: { cardOrder: ['c2', 'c1'] },
    });
    expect(sent.map((one) => [one.method, one.url, one.body])).toEqual([
      ['PATCH', '/api/v1/w/w1/boards/b/blocks/v', { updatedFields: { cardOrder: ['c2', 'c1'] } }],
    ]);
    expect(view.fields.cardOrder).toEqual(['c2', 'c1']);
  });

  const VIEW = {
    ...BLOCK,
    id: 'v',
    parentId: '',
    type: 'view',
    title: 'Board',
    fields: { cardOrder: ['c1', 'c2'] },
  };

  it('propagates 404 when the server refuses the view patch', async () => {
    const sent = serve(
      { type: '/problems/not-found', title: 'Not found', status: 404, detail: 'no view' },
      404,
    );
    const refused = await serverBackend('')
      .patchView('b', 'v', { updatedFields: { cardOrder: [] } })
      .catch((error: unknown) => error);
    expect(isBackendError(refused) && refused.status).toBe(404);
    expect(sent.map((one) => one.method)).toEqual(['PATCH']);
  });

  it('preserves previously read view metadata when patching fields in 1 round-trip', async () => {
    const backend = serverBackend('');
    serve([VIEW]);
    await backend.views('b');
    const sent = serve(undefined, 204);
    const view = await backend.patchView('b', 'v', {
      updatedFields: { cardOrder: ['c2', 'c1'] },
    });
    expect(sent).toHaveLength(1);
    expect(view.title).toBe('Board');
    expect(view.fields.cardOrder).toEqual(['c2', 'c1']);
  });

  it('makes a view via the fractional-indexed views route', async () => {
    const sent = serve(
      { view: { ...VIEW, fields: { viewType: 'calendar', order: 'V' } }, companionViews: [] },
      201,
    );
    const view = await serverBackend('').createView('b', {
      title: 'Calendar view',
      fields: { viewType: 'calendar', dateProperty: 'due' } as never,
    });
    expect(sent.map((one) => [one.method, one.url])).toEqual([
      ['POST', '/api/v1/w/w1/boards/b/views'],
    ]);
    expect(sent[0]?.body).toEqual({
      title: 'Calendar view',
      viewType: 'calendar',
      fields: { viewType: 'calendar', dateProperty: 'due' },
    });
    expect(view.id).toBe('v');
    expect(view.fields.viewType).toBe('calendar');
  });

  it('sends two quick changes of one view one after the other in two PATCH requests', async () => {
    const order: string[] = [];
    vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
      order.push(`${init.method ?? 'GET'} ${typeof init.body === 'string' ? init.body : url}`);
      await new Promise((resolve) => setTimeout(resolve, order.length === 1 ? 20 : 1));
      return new Response(null, { status: 204 });
    });
    const backend = serverBackend('');
    await Promise.all([
      backend.patchView('b', 'v', { updatedFields: { cardOrder: ['a'] } }),
      backend.patchView('b', 'v', { updatedFields: { cardOrder: ['b'] } }),
    ]);
    expect(order).toEqual([
      'PATCH {"updatedFields":{"cardOrder":["a"]}}',
      'PATCH {"updatedFields":{"cardOrder":["b"]}}',
    ]);
  });

  it('sends readAt on patchView when given, forwards it along queued writes, and stops on failure', async () => {
    let updatedAt = 10;
    const patches: Record<string, unknown>[] = [];
    vi.stubGlobal('fetch', (_url: string, init: RequestInit) => {
      if (init.method === 'PATCH') {
        const body = JSON.parse(typeof init.body === 'string' ? init.body : '{}') as Record<
          string,
          unknown
        >;
        patches.push(body);
        if (body.readAt !== undefined && body.readAt !== updatedAt) {
          return Promise.resolve(
            new Response(
              JSON.stringify({
                type: '/problems/conflict',
                title: 'Conflict',
                status: 409,
                detail: 'the block has changed since it was read',
              }),
              { status: 409, headers: { 'Content-Type': 'application/problem+json' } },
            ),
          );
        }
        updatedAt += 1;
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      return Promise.resolve(
        new Response(
          JSON.stringify([
            {
              ...VIEW,
              updatedAt,
              fields: {
                ...VIEW.fields,
                order: patches.at(-1)?.updatedFields
                  ? (patches.at(-1)?.updatedFields as { order?: string }).order
                  : undefined,
              },
            },
          ]),
          { status: 200 },
        ),
      );
    });
    const backend = serverBackend('');
    // Two queued conditional writes with the same initial readAt: the second advances to 11.
    const [firstView, secondView] = await Promise.all([
      backend.patchView('b', 'v', { updatedFields: { order: 'V' }, readAt: 10 }),
      backend.patchView('b', 'v', { updatedFields: { order: 'W' }, readAt: 10 }),
    ]);
    expect([firstView.updatedAt, secondView.updatedAt]).toEqual([11, 12]);
    // Sequential conditional write using returned updatedAt also succeeds without WS echo.
    const thirdView = await backend.patchView('b', 'v', {
      updatedFields: { order: 'U' },
      readAt: secondView.updatedAt,
    });
    expect(thirdView.updatedAt).toBe(13);
    expect(patches).toEqual([
      { updatedFields: { order: 'V' }, readAt: 10 },
      { updatedFields: { order: 'W' }, readAt: 11 },
      { updatedFields: { order: 'U' }, readAt: 12 },
    ]);

    // When the first fails with 409, the second keeps its stale readAt and also gets 409.
    patches.length = 0;
    updatedAt = 20;
    const results = await Promise.allSettled([
      backend.patchView('b', 'v', { updatedFields: { order: 'X' }, readAt: 13 }),
      backend.patchView('b', 'v', { updatedFields: { order: 'Y' }, readAt: 13 }),
    ]);
    expect(results.map((r) => r.status)).toEqual(['rejected', 'rejected']);
    expect(patches).toEqual([
      { updatedFields: { order: 'X' }, readAt: 13 },
      { updatedFields: { order: 'Y' }, readAt: 13 },
    ]);

    // Unconditional write over a view somebody else already bumped does not advance a stale queued readAt.
    patches.length = 0;
    updatedAt = 30;
    const mixed = await Promise.allSettled([
      backend.patchView('b', 'v', { title: 'Renamed' }),
      backend.patchView('b', 'v', { updatedFields: { order: 'Z' }, readAt: 20 }),
    ]);
    expect(mixed.map((r) => r.status)).toEqual(['fulfilled', 'rejected']);
    expect(patches).toEqual([{ title: 'Renamed' }, { updatedFields: { order: 'Z' }, readAt: 20 }]);

    // An external order change between GET and read-back GET of an unconditional write
    // must not be masked for a queued conditional place write behind it.
    let getCount = 0;
    const maskedPatches: Record<string, unknown>[] = [];
    vi.stubGlobal('fetch', (_url: string, init: RequestInit) => {
      if (init.method === 'PATCH') {
        const body = JSON.parse(typeof init.body === 'string' ? init.body : '{}') as Record<
          string,
          unknown
        >;
        maskedPatches.push(body);
        if (body.readAt !== undefined && body.readAt !== 12) {
          return Promise.resolve(
            new Response(
              JSON.stringify({
                type: '/problems/conflict',
                title: 'Conflict',
                status: 409,
                detail: 'the block has changed since it was read',
              }),
              { status: 409, headers: { 'Content-Type': 'application/problem+json' } },
            ),
          );
        }
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      getCount += 1;
      // First GET sees order 'V' at 10; before read-back GET another editor moved it to 'Z' at 12.
      const view =
        getCount === 1
          ? { ...VIEW, updatedAt: 10, fields: { ...VIEW.fields, order: 'V' } }
          : { ...VIEW, updatedAt: 12, fields: { ...VIEW.fields, order: 'Z' } };
      return Promise.resolve(new Response(JSON.stringify([view]), { status: 200 }));
    });
    const concurrent = await Promise.allSettled([
      backend.patchView('b', 'v', { title: 'Renamed' }),
      backend.patchView('b', 'v', { updatedFields: { order: 'X' }, readAt: 10 }),
    ]);
    expect(concurrent.map((r) => r.status)).toEqual(['fulfilled', 'rejected']);
    expect(maskedPatches).toEqual([
      { title: 'Renamed' },
      { updatedFields: { order: 'X' }, readAt: 10 },
    ]);
  });
});

describe('the trash on the server', () => {
  it('deletes a block and takes it back out by its own routes', async () => {
    const sent = serve(undefined, 204);
    const backend = serverBackend('');
    await backend.deleteBlock('b', 'c1');
    const restoring = serve({ ...BLOCK, id: 'c1', type: 'card', title: 'Back' });
    await backend.restoreBlock('b', 'c1');
    expect(sent.map((one) => `${one.method} ${one.url}`)).toEqual([
      'DELETE /api/v1/w/w1/boards/b/blocks/c1',
    ]);
    expect(restoring.map((one) => `${one.method} ${one.url}`)).toEqual([
      'POST /api/v1/w/w1/boards/b/blocks/c1/restore',
    ]);
    expect(sent[0]?.headers['X-Requested-With']).toBe('XMLHttpRequest');
  });
});

describe('the settings on the server', () => {
  it('are read and written under the contract’s names', async () => {
    const stored = {
      theme: 'dark',
      cardOpen: 'center',
      density: 'compact',
      haptics: false,
      timeZone: 'Asia/Seoul',
      weekStart: 'monday',
      hourCycle: 'h23',
    };
    serve(stored);
    const backend = serverBackend('');
    expect(await backend.settings()).toMatchObject({ theme: 'dark', haptics: false });
    const sent = serve({ ...stored, theme: 'light' });
    await backend.patchSettings({ theme: 'light', haptics: true });
    expect(sent.map((one) => `${one.method} ${one.url}`)).toEqual([
      'PATCH /api/v1/users/me/settings',
    ]);
    expect(sent[0]?.body).toEqual({ theme: 'light', haptics: true });
  });
});

/** Answers each request by the first route whose `METHOD path` starts it, and keeps what was sent. */
function serveRoutes(routes: [string, unknown, number?][]): Sent[] {
  const sent: Sent[] = [];
  vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
    const method = init.method ?? 'GET';
    sent.push({
      url,
      method,
      headers: init.headers as Record<string, string>,
      body: typeof init.body === 'string' ? JSON.parse(init.body) : undefined,
    });
    const route = routes.find(([start]) => `${method} ${url}`.startsWith(start));
    if (!route) throw new Error(`nothing serves ${method} ${url}`);
    const [, answer, status = 200] = route;
    return Promise.resolve(
      new Response(status === 204 ? null : JSON.stringify(answer), {
        status,
        headers: {
          'Content-Type': status >= 400 ? 'application/problem+json' : 'application/json',
        },
      }),
    );
  });
  return sent;
}

const sentLines = (sent: Sent[]) => sent.map((one) => `${one.method} ${one.url}`);

describe('the sidebar on the server', () => {
  const CATEGORY = {
    id: 'k1',
    name: 'Work',
    userId: 'me',
    workspaceId: 't',
    type: 'custom',
    collapsed: true,
    sortOrder: 10,
    createdAt: 1,
    updatedAt: 2,
  };

  it('makes a category of the reader without re-calling me() once known', async () => {
    const sent = serveRoutes([
      ['GET /api/v1/users/me', { id: 'me', username: 'me', createdAt: 1, updatedAt: 1 }],
      ['POST /api/v1/w/t/categories', CATEGORY, 201],
    ]);
    const backend = serverBackend('');
    const made = await backend.createCategory('t', { name: 'Work' });
    await backend.createCategory('t', { name: 'Next' });
    expect(sentLines(sent)).toEqual([
      'GET /api/v1/users/me',
      'POST /api/v1/w/t/categories',
      'POST /api/v1/w/t/categories',
    ]);
    expect(sent[1]?.body).toEqual({ name: 'Work', userId: 'me', workspaceId: 't' });
    expect(sent[2]?.body).toEqual({ name: 'Next', userId: 'me', workspaceId: 't' });
    expect(made).toMatchObject({ id: 'k1', containerIds: [], hiddenContainerIds: [] });
  });

  it('patches a category via single-transaction PATCH /categories/{id}', async () => {
    const sent = serveRoutes([['PATCH /api/v1/w/t/categories/k1', { ...CATEGORY, name: 'Home' }]]);
    const renamed = await serverBackend('').patchCategory('t', 'k1', { name: 'Home' });
    expect(sentLines(sent)).toEqual(['PATCH /api/v1/w/t/categories/k1']);
    expect(sent[0]?.body).toEqual({ name: 'Home' });
    expect(renamed).toMatchObject({ name: 'Home', collapsed: true });
  });

  it('folds a category via PATCH /categories/{id}', async () => {
    const sent = serveRoutes([
      ['PATCH /api/v1/w/t/categories/k1', { ...CATEGORY, collapsed: false }],
    ]);
    await serverBackend('').patchCategory('t', 'k1', { collapsed: false });
    expect(sent[0]?.body).toEqual({ collapsed: false });
  });

  it('propagates 404 from PATCH /categories/{id} when the category is missing', async () => {
    const sent = serveRoutes([
      [
        'PATCH /api/v1/w/t/categories/k9',
        { type: '/problems/not-found', title: 'Not Found', status: 404, detail: 'missing' },
        404,
      ],
    ]);
    const refused = await serverBackend('')
      .patchCategory('t', 'k9', { name: 'X' })
      .catch((error: unknown) => error);
    expect(isBackendError(refused) && refused.status).toBe(404);
    expect(sentLines(sent)).toEqual(['PATCH /api/v1/w/t/categories/k9']);
  });

  it('deletes a category and orders the categories by their routes', async () => {
    const sent = serveRoutes([
      ['DELETE /api/v1/w/t/categories/k1', undefined, 204],
      ['PUT /api/v1/w/t/category-order', ['k2', 'k1']],
    ]);
    const backend = serverBackend('');
    await backend.deleteCategory('t', 'k1');
    await backend.reorderCategories('t', ['k2', 'k1']);
    expect(sentLines(sent)).toEqual([
      'DELETE /api/v1/w/t/categories/k1',
      'PUT /api/v1/w/t/category-order',
    ]);
    expect(sent[1]?.body).toEqual(['k2', 'k1']);
  });

  it('moves a board into a category at an index in one PUT request', async () => {
    const sent = serveRoutes([['PUT /api/v1/w/t/categories/k1/containers/b9', undefined, 204]]);
    await serverBackend('').moveContainerToCategory('t', 'b9', 'k1', 1);
    expect(sentLines(sent)).toEqual(['PUT /api/v1/w/t/categories/k1/containers/b9']);
    expect(sent[0]?.body).toEqual({ index: 1 });
  });

  it('serializes the sidebar’s writes of a workspace one after another', async () => {
    const sent = serveRoutes([
      ['PUT /api/v1/w/t/categories/k1/containers/', undefined, 204],
      ['PATCH /api/v1/w/t/categories/k1', CATEGORY],
    ]);
    const backend = serverBackend('');
    await Promise.all([
      backend.moveContainerToCategory('t', 'b3', 'k1', 0),
      backend.patchCategory('t', 'k1', { collapsed: false }),
      backend.moveContainerToCategory('t', 'b2', 'k1', 0),
    ]);
    expect(sentLines(sent)).toEqual([
      'PUT /api/v1/w/t/categories/k1/containers/b3',
      'PATCH /api/v1/w/t/categories/k1',
      'PUT /api/v1/w/t/categories/k1/containers/b2',
    ]);
  });

  it('hides a board and shows it again via PUT /categories/{id}/containers/{id}', async () => {
    const sent = serveRoutes([['PUT /api/v1/w/t/categories/k1/containers/b1', undefined, 204]]);
    const backend = serverBackend('');
    await backend.setContainerHidden('t', 'k1', 'b1', true);
    await backend.setContainerHidden('t', 'k1', 'b1', false);
    expect(sentLines(sent)).toEqual([
      'PUT /api/v1/w/t/categories/k1/containers/b1',
      'PUT /api/v1/w/t/categories/k1/containers/b1',
    ]);
    expect(sent.map((one) => one.body)).toEqual([{ hidden: true }, { hidden: false }]);
  });
});

describe('boards on the server', () => {
  const BOARD = {
    id: 'b1',
    workspaceId: 't',
    createdBy: 'me',
    modifiedBy: 'me',
    visibility: 'private',
    minimumRole: 'editor',
    title: 'Plan',
    description: '',
    icon: '',
    showDescription: false,
    isTemplate: false,
    templateVersion: 0,
    properties: {},
    cardProperties: [],
    createdAt: 1,
    updatedAt: 2,
  };
  const VIEW = {
    ...BLOCK,
    id: 'v1',
    containerId: 'b1',
    parentId: '',
    type: 'view',
    title: 'Board view',
  };

  it('makes a board and its first view in one request, the view tied to the board', async () => {
    const sent = serveRoutes([
      ['POST /api/v1/w/t/containers-and-blocks', { containers: [BOARD], blocks: [VIEW] }, 201],
    ]);
    const made = await serverBackend('').createBoard(
      't',
      { title: 'Plan', cardProperties: [{ id: 's', name: 'Status', type: 'select', options: [] }] },
      { title: 'Board view', fields: { viewType: 'board' } as never },
    );
    expect(sentLines(sent)).toEqual(['POST /api/v1/w/t/containers-and-blocks']);
    const body = sent[0]?.body as {
      containers: { id: string }[];
      blocks: { containerId: string }[];
    };
    expect(body.containers).toEqual([
      {
        id: body.blocks[0]?.containerId,
        visibility: 'private',
        title: 'Plan',
        description: '',
        icon: '',
        cardProperties: [{ id: 's', name: 'Status', type: 'select', options: [] }],
      },
    ]);
    expect(body.blocks[0]).toMatchObject({ type: 'view', title: 'Board view' });
    expect(body.blocks[0]).not.toHaveProperty('parentId');
    expect(made.board.id).toBe('b1');
    expect(made.view).toMatchObject({ id: 'v1', title: 'Board view' });
  });

  it('changes, deletes and brings back a board by its routes', async () => {
    const sent = serveRoutes([
      ['PATCH /api/v1/w/w1/containers/b1', { ...BOARD, title: 'New' }],
      ['DELETE /api/v1/w/t/containers/b1', undefined, 204],
      ['POST /api/v1/w/t/containers/b1/restore', BOARD],
    ]);
    const backend = serverBackend('');
    expect(
      (await backend.patchContainer('b1', { title: 'New', minimumRole: 'viewer' })).title,
    ).toBe('New');
    await backend.deleteContainer('b1');
    expect((await backend.restoreContainer('b1')).id).toBe('b1');
    expect(sentLines(sent)).toEqual([
      'PATCH /api/v1/w/w1/containers/b1',
      'DELETE /api/v1/w/t/containers/b1',
      'POST /api/v1/w/t/containers/b1/restore',
    ]);
    expect(sent[0]?.body).toEqual({ title: 'New', minimumRole: 'viewer' });
  });

  it('refuses a minimum role the contract does not have, sending nothing', async () => {
    const sent = serveRoutes([]);
    const refused = await serverBackend('')
      .patchContainer('b1', { minimumRole: 'admin' })
      .catch((error: unknown) => error);
    expect(isBackendError(refused) && refused.status).toBe(400);
    expect(sent).toEqual([]);
  });

  it('makes a view via POST /boards/{boardId}/views', async () => {
    const sent = serveRoutes([
      ['POST /api/v1/w/w1/boards/b1/views', { view: VIEW, companionViews: [] }, 201],
    ]);
    const view = await serverBackend('').createView('b1', {
      title: 'Table view',
      fields: { viewType: 'table' } as never,
      after: null,
    });
    expect(sent[0]?.body).toEqual({
      title: 'Table view',
      viewType: 'table',
      fields: { viewType: 'table' },
      after: null,
    });
    expect(view.id).toBe('v1');
  });
});

describe('the password on the server', () => {
  it('is changed with the old one, and a wrong old one is let through with its reason', async () => {
    const sent = serveRoutes([['POST /api/v1/users/me/password', undefined, 204]]);
    const backend = serverBackend('');
    await backend.changePassword('old secret', 'new secret');
    expect(sentLines(sent)).toEqual(['POST /api/v1/users/me/password']);
    expect(sent[0]?.body).toEqual({ oldPassword: 'old secret', newPassword: 'new secret' });
    serveRoutes([
      [
        'POST /api/v1/users/me/password',
        {
          type: '/problems/invalid-credentials',
          title: 'Invalid credentials',
          status: 400,
          detail: 'wrong',
          reason: 'invalid_credentials',
          errors: [{ pointer: '/oldPassword', detail: 'wrong', reason: 'invalid_credentials' }],
        },
        400,
      ],
    ]);
    const refused = await backend
      .changePassword('x', 'new secret')
      .catch((error: unknown) => error);
    expect(isBackendError(refused) && refused.reason).toBe('invalid_credentials');
    expect(isBackendError(refused) && refused.refusalAt('/oldPassword')?.reason).toBe(
      'invalid_credentials',
    );
  });
});

describe('sharing a board on the server', () => {
  const MEMBER = {
    containerId: 'b1',
    userId: 'u2',
    role: 'commenter' as const,
  };

  it('adds, changes and removes a member with the requested role', async () => {
    const sent = serveRoutes([
      ['POST /api/v1/w/w1/containers/b1/members', MEMBER],
      ['PUT /api/v1/w/w1/containers/b1/members/u2', { ...MEMBER, role: 'viewer' }],
      ['DELETE /api/v1/w/w1/containers/b1/members/u2', undefined, 204],
    ]);
    const backend = serverBackend('');
    expect((await backend.addMember('b1', 'u2', 'commenter')).role).toBe('commenter');
    expect((await backend.setMemberRole('b1', 'u2', 'viewer')).role).toBe('viewer');
    await backend.removeMember('b1', 'u2');
    expect(sentLines(sent)).toEqual([
      'POST /api/v1/w/w1/containers/b1/members',
      'PUT /api/v1/w/w1/containers/b1/members/u2',
      'DELETE /api/v1/w/w1/containers/b1/members/u2',
    ]);
    expect(sent[0]?.body).toEqual({
      userId: 'u2',
      role: 'commenter',
    });
    expect(sent[1]?.body).toEqual({
      role: 'viewer',
    });
  });

  it('reads and sets the public link, asking for a new token only when told', async () => {
    const sent = serveRoutes([
      ['GET /api/v1/w/w1/containers/b1/public-link', { enabled: false }],
      [
        'POST /api/v1/w/w1/containers/b1/public-link',
        { enabled: true, token: 'tok', updatedAt: 3 },
      ],
    ]);
    const backend = serverBackend('');
    expect(await backend.publicLink('b1')).toEqual({ enabled: false, token: '' });
    expect(await backend.setPublicLink('b1', { enabled: true })).toEqual({
      enabled: true,
      token: 'tok',
    });
    await backend.setPublicLink('b1', { enabled: true, regenerate: true });
    expect(sent.slice(1).map((one) => one.body)).toEqual([
      { enabled: true },
      { enabled: true, regenerate: true },
    ]);
  });

  it('finds people of the workspace by the search of their list', async () => {
    const sent = serveRoutes([['GET /api/v1/w/t/users', []]]);
    await serverBackend('').searchUsers('t', 'ann lee');
    expect(sentLines(sent)).toEqual(['GET /api/v1/w/t/users?search=ann+lee']);
  });

  it("takes the reader's workspaces from their workspaces, those they are a guest of too, with what they are in each", async () => {
    const workspace = (id: string, role: string) => ({
      id,
      orgId: 'o',
      name: `n-${id}`,
      role,
      ssoRequired: false,
    });
    const sent = serveRoutes([
      [
        'GET /api/v1/users/me/workspaces',
        [workspace('w1', 'guest'), workspace('w2', 'member'), workspace('w3', 'admin')],
      ],
    ]);
    expect(await serverBackend('').workspaces()).toEqual([
      { id: 'w1', title: 'n-w1', role: 'guest', orgId: 'o', ssoRequired: false },
      { id: 'w2', title: 'n-w2', role: 'member', orgId: 'o', ssoRequired: false },
      { id: 'w3', title: 'n-w3', role: 'admin', orgId: 'o', ssoRequired: false },
    ]);
    expect(sentLines(sent)).toEqual(['GET /api/v1/users/me/workspaces']);
  });
});

describe('templates on the server', () => {
  const BOARD = {
    id: 'b2',
    workspaceId: 't',
    createdBy: 'me',
    modifiedBy: 'me',
    visibility: 'private',
    minimumRole: 'editor',
    title: 'Project Tasks',
    description: '',
    icon: '',
    showDescription: false,
    isTemplate: false,
    isBuiltin: false,
    templateVersion: 0,
    properties: {},
    cardProperties: [],
    createdAt: 1,
    updatedAt: 2,
  };

  it('reads the templates, copies a board either way, and makes the welcome board', async () => {
    const sent = serveRoutes([
      ['GET /api/v1/w/t/templates', [{ ...BOARD, id: 'tp', isTemplate: true, isBuiltin: true }]],
      ['POST /api/v1/w/w1/containers/tp/duplicate', { containers: [BOARD], blocks: [] }, 201],
      ['POST /api/v1/w/t/containers/b2/duplicate', { containers: [BOARD], blocks: [] }, 201],
      ['POST /api/v1/w/t/onboard', { boardId: 'w1' }],
    ]);
    const backend = serverBackend('');
    const [template] = await backend.templates('t');
    expect(template).toMatchObject({ id: 'tp', isTemplate: true, isBuiltin: true });
    expect((await backend.duplicateContainer('tp', false, 't')).id).toBe('b2');
    await backend.duplicateContainer('b2', true, 't');
    expect(await backend.onboard('t')).toBe('w1');
    expect(sentLines(sent)).toEqual([
      'GET /api/v1/w/t/templates',
      'POST /api/v1/w/w1/containers/tp/duplicate?asTemplate=false&toWorkspace=t',
      'POST /api/v1/w/t/containers/b2/duplicate?asTemplate=true&toWorkspace=t',
      'POST /api/v1/w/t/onboard',
    ]);
  });
});

describe('searching boards on the server', () => {
  it('asks the workspace’s search by title', async () => {
    const sent = serveRoutes([['GET /api/v1/w/t/boards/search', []]]);
    expect(await serverBackend('').searchBoards('t', 'garden plan')).toEqual([]);
    expect(sentLines(sent)).toEqual(['GET /api/v1/w/t/boards/search?q=garden+plan']);
  });
});

describe('session cookie rotation during password changes', () => {
  const problem = (status: number, detail: string, reason?: string): Response =>
    new Response(
      JSON.stringify({
        type: '/problems/unauthorized',
        title: 'No valid session',
        status,
        detail,
        ...(reason ? { reason } : {}),
      }),
      { status, headers: { 'Content-Type': 'application/problem+json' } },
    );

  it('retries concurrent GET and POST requests refused with 401 while setPassword rotates the cookie', async () => {
    let cookie = 'old';
    let finishSet!: () => void;
    const setGate = new Promise<void>((resolve) => {
      finishSet = resolve;
    });
    const calls: string[] = [];

    vi.stubGlobal('fetch', async (url: string, init?: RequestInit) => {
      const method = init?.method ?? 'GET';
      const usedCookie = cookie;
      calls.push(`${method} ${url} [${usedCookie}]`);
      if (method === 'PUT' && url === '/api/v1/users/me/password') {
        await setGate;
        cookie = 'new';
        return new Response(null, { status: 204 });
      }
      if (usedCookie !== 'new') return problem(401, 'invalid session token');
      if (method === 'POST' && url === '/api/v1/w/w1/users') {
        return new Response(
          JSON.stringify([
            { id: 'u1', username: 'ada', displayName: 'Ada', createdAt: 1, updatedAt: 2 },
          ]),
          { status: 200, headers: { 'Content-Type': 'application/json' } },
        );
      }
      return new Response(JSON.stringify([]), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    });

    const backend = serverBackend('');
    const setting = backend.setPassword('a long password');
    const boards = backend.boards('w1');
    const users = backend.usersById('w1', ['u1']);
    await Promise.resolve();
    finishSet();

    await setting;
    expect(await boards).toEqual([]);
    expect((await users).map((user) => user.id)).toEqual(['u1']);
    expect(calls).toEqual([
      'PUT /api/v1/users/me/password [old]',
      'GET /api/v1/w/w1/boards [old]',
      'POST /api/v1/w/w1/users [old]',
      'GET /api/v1/w/w1/boards [new]',
      'POST /api/v1/w/w1/users [new]',
    ]);
  });

  it('retries a slow request started before confirmPassword whose 401 arrives after rotation finishes, and bytes too', async () => {
    let cookie = 'old';
    let releaseSlow!: () => void;
    const slowGate = new Promise<void>((resolve) => {
      releaseSlow = resolve;
    });
    let firstBoards = true;
    let firstFile = true;

    vi.stubGlobal('fetch', async (url: string, init?: RequestInit) => {
      const method = init?.method ?? 'GET';
      const usedCookie = cookie;
      if (method === 'POST' && url === '/api/v1/users/me/password/confirm') {
        cookie = 'new';
        return new Response(null, { status: 204 });
      }
      if (url === '/api/v1/w/w1/boards' && firstBoards) {
        firstBoards = false;
        await slowGate;
        return problem(401, 'invalid session token');
      }
      if (url.includes('/files/f1') && firstFile) {
        firstFile = false;
        await slowGate;
        return problem(401, 'invalid session token');
      }
      if (usedCookie !== 'new') return problem(401, 'invalid session token');
      if (url.includes('/files/f1')) return new Response('ok', { status: 200 });
      return new Response(JSON.stringify([]), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    });

    const backend = serverBackend('');
    const slowBoards = backend.boards('w1');
    const slowBytes = backend.fileBytes('b1', 'f1');
    await backend.confirmPassword('a long password');
    releaseSlow();

    expect(await slowBoards).toEqual([]);
    expect(await (await slowBytes).text()).toBe('ok');
  });

  it('does not hide real session loss, failed rotation, remote logout, or account deletion pending', async () => {
    // 1. Plain 401 with no rotation fails on the first try.
    let tries = 0;
    vi.stubGlobal('fetch', () => {
      tries += 1;
      return Promise.resolve(problem(401, 'invalid session token'));
    });
    await expect(serverBackend('').boards('w1')).rejects.toMatchObject({ status: 401 });
    expect(tries).toBe(1);

    // 2. Failed changePassword does not trigger retries of concurrent 401s.
    let failRot!: () => void;
    const rotGate = new Promise<void>((resolve) => {
      failRot = resolve;
    });
    let boardTries = 0;
    vi.stubGlobal('fetch', async (url: string, init?: RequestInit) => {
      if (init?.method === 'POST' && url === '/api/v1/users/me/password') {
        await rotGate;
        return problem(403, 'wrong password', 'invalid_credentials');
      }
      boardTries += 1;
      return problem(401, 'invalid session token');
    });
    const b1 = serverBackend('');
    const changing = b1.changePassword('wrong', 'new password');
    const reading = b1.boards('w1');
    await Promise.resolve();
    failRot();
    await expect(changing).rejects.toMatchObject({ status: 403 });
    await expect(reading).rejects.toMatchObject({ status: 401 });
    expect(boardTries).toBe(1);

    // 3. Remote logout right after rotation: retries once under the new cookie, then fails 401.
    let finishRevoked!: () => void;
    const revokedGate = new Promise<void>((resolve) => {
      finishRevoked = resolve;
    });
    let revokedTries = 0;
    vi.stubGlobal('fetch', async (url: string, init?: RequestInit) => {
      if (init?.method === 'PUT' && url === '/api/v1/users/me/password') {
        await revokedGate;
        return new Response(null, { status: 204 });
      }
      revokedTries += 1;
      return problem(401, 'invalid session token');
    });
    const b2 = serverBackend('');
    const setting = b2.setPassword('a long password');
    const revokedRead = b2.boards('w1');
    await Promise.resolve();
    finishRevoked();
    await setting;
    await expect(revokedRead).rejects.toMatchObject({ status: 401 });
    expect(revokedTries).toBe(2);

    // 4. account_deletion_pending is never retried even while a rotation succeeds.
    let finishPending!: () => void;
    const pendingGate = new Promise<void>((resolve) => {
      finishPending = resolve;
    });
    let leavingTries = 0;
    vi.stubGlobal('fetch', async (url: string, init?: RequestInit) => {
      if (init?.method === 'PUT' && url === '/api/v1/users/me/password') {
        await pendingGate;
        return new Response(null, { status: 204 });
      }
      leavingTries += 1;
      return problem(401, 'account deletion pending', 'account_deletion_pending');
    });
    const b3 = serverBackend('');
    const set3 = b3.setPassword('a long password');
    const leaving = b3.boards('w1');
    await Promise.resolve();
    finishPending();
    await set3;
    await expect(leaving).rejects.toMatchObject({
      status: 401,
      reason: 'account_deletion_pending',
    });
    expect(leavingTries).toBe(1);
  });

  it('aborts cleanly while waiting on an in-flight rotation without surfacing a stale 401', async () => {
    let finishSet!: () => void;
    const setGate = new Promise<void>((resolve) => {
      finishSet = resolve;
    });
    vi.stubGlobal('fetch', async (url: string, init?: RequestInit) => {
      if (init?.method === 'PUT' && url === '/api/v1/users/me/password') {
        await setGate;
        return new Response(null, { status: 204 });
      }
      return problem(401, 'invalid session token');
    });

    const http = new Http('');
    const setting = http.request(
      '/api/v1/users/me/password',
      {},
      { method: 'PUT', json: { password: 'a long password' }, rotates: true },
    );
    const bytesAbort = new AbortController();
    const fileRead = http.bytes(
      '/api/v1/w/{workspaceId}/containers/{containerId}/files/{fileId}',
      { workspaceId: 'w1', containerId: 'b1', fileId: 'f1' },
      {},
      bytesAbort.signal,
    );
    const reqAbort = new AbortController();
    const boardsRead = http.request(
      '/api/v1/w/{workspaceId}/boards',
      { workspaceId: 'w1' },
      { signal: reqAbort.signal },
    );
    await Promise.resolve();
    await Promise.resolve();
    bytesAbort.abort();
    reqAbort.abort();

    // bytes() reports status-0 no answer (never stale 401); request() rethrows the AbortError.
    await expect(fileRead).rejects.toMatchObject({ status: 0 });
    await expect(boardsRead).rejects.toThrow();
    finishSet();
    await setting;
  });

  it('survives back-to-back setPassword and confirmPassword rotations up to the retry cap', async () => {
    let cookie = 'c0';
    let finishFirst!: () => void;
    let finishSecond!: () => void;
    const firstGate = new Promise<void>((resolve) => {
      finishFirst = resolve;
    });
    const secondGate = new Promise<void>((resolve) => {
      finishSecond = resolve;
    });
    const backend = serverBackend('');
    let reads = 0;
    let secondPromise: Promise<void> | undefined;

    vi.stubGlobal('fetch', async (url: string, init?: RequestInit) => {
      if (init?.method === 'PUT' && url === '/api/v1/users/me/password') {
        await firstGate;
        cookie = 'c1';
        return new Response(null, { status: 204 });
      }
      if (init?.method === 'POST' && url === '/api/v1/users/me/password/confirm') {
        await secondGate;
        cookie = 'c2';
        return new Response(null, { status: 204 });
      }
      reads += 1;
      if (reads === 2 && !secondPromise) {
        secondPromise = backend.confirmPassword('a long password');
      }
      if (cookie !== 'c2') return problem(401, 'invalid session token');
      return new Response(JSON.stringify([]), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    });

    const first = backend.setPassword('a long password');
    const boards = backend.boards('w1');
    await Promise.resolve();
    finishFirst();
    await first;
    // Let the second read start confirmPassword and hit 401 under c1 before c2 commits.
    await Promise.resolve();
    await Promise.resolve();
    finishSecond();
    await secondPromise;
    expect(await boards).toEqual([]);
    expect(reads).toBe(3);
  });
});
