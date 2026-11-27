/**
 * Uploading a file to the server in parts, against a stand-in for `fetch`:
 * what is sent for each step, the digest of each part, progress, a part sent
 * again, giving up, and the limit read from the server's settings.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { serverBackend } from './support.ts';
import { contentDigest } from '$lib/api/upload.ts';
import { isBackendError } from '$lib/state/index.ts';

interface Call {
  method: string;
  url: string;
  size: number | undefined;
  headers: Record<string, string>;
  json: unknown;
}

/** A server that takes uploads in parts of `partSize` bytes; `fail` decides a part's answer. */
function uploadServer(options: {
  partSize: number;
  maxFileBytes?: number;
  fail?: (number: number, attempt: number) => number | undefined;
}): Call[] {
  const calls: Call[] = [];
  const attempts = new Map<number, number>();
  vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
    const method = init.method ?? 'GET';
    init.signal?.throwIfAborted();
    calls.push({
      method,
      url,
      size: init.body instanceof Blob ? init.body.size : undefined,
      headers: init.headers as Record<string, string>,
      json: typeof init.body === 'string' ? JSON.parse(init.body) : undefined,
    });
    const answer = (status: number, body?: unknown): Response =>
      new Response(body === undefined ? null : JSON.stringify(body), {
        status,
        headers: status >= 400 ? { 'Content-Type': 'application/problem+json' } : undefined,
      });
    const problem = (status: number, name: string, detail: string): Response =>
      answer(status, { type: `/problems/${name}`, title: name, status, detail });
    if (url.endsWith('/client-config')) {
      return answer(200, { maxFileBytes: options.maxFileBytes ?? 1000 });
    }
    if (method === 'POST' && url.endsWith('/uploads')) {
      const size = (calls.at(-1)?.json as { size: number }).size;
      return answer(201, {
        uploadId: 'up1',
        partSize: options.partSize,
        partCount: Math.ceil(size / options.partSize),
        expiresAt: 1,
      });
    }
    const part = /\/parts\/(\d+)$/.exec(url);
    if (method === 'PUT' && part) {
      const number = Number(part[1]);
      const attempt = (attempts.get(number) ?? 0) + 1;
      attempts.set(number, attempt);
      const status = options.fail?.(number, attempt);
      if (status === 0) throw new TypeError('no answer');
      if (status === 503) {
        return answer(503, { type: '/problems/database-busy', status, reason: 'database_busy' });
      }
      return status ? problem(status, 'internal', 'x') : answer(204);
    }
    if (url.endsWith('/complete')) return answer(201, { fileId: 'f1.txt' });
    if (method === 'DELETE') return answer(204);
    return problem(404, 'not-found', url);
  });
  return calls;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('uploading a file to the server', () => {
  it('opens an upload, sends each part with its digest, tells the progress, and completes', async () => {
    const calls = uploadServer({ partSize: 4 });
    const file = new File(['0123456789'], 'notes.txt', { type: 'text/plain' });
    const progress: number[] = [];
    const uploaded = await serverBackend('/base').upload('b1', file, {
      onProgress: ({ sentBytes }) => progress.push(sentBytes),
    });
    expect(calls.map((call) => `${call.method} ${call.url}`)).toEqual([
      'GET /base/api/v1/client-config',
      'POST /base/api/v1/w/w1/containers/b1/uploads',
      'PUT /base/api/v1/w/w1/uploads/up1/parts/1',
      'PUT /base/api/v1/w/w1/uploads/up1/parts/2',
      'PUT /base/api/v1/w/w1/uploads/up1/parts/3',
      'POST /base/api/v1/w/w1/uploads/up1/complete',
    ]);
    expect(calls[1]?.json).toEqual({ name: 'notes.txt', size: 10, contentType: 'text/plain' });
    expect(calls.slice(2, 5).map((call) => call.size)).toEqual([4, 4, 2]);
    expect(calls[2]?.headers['Content-Digest']).toBe(await contentDigest(new Blob(['0123'])));
    expect(calls[2]?.headers['Content-Type']).toBe('application/octet-stream');
    expect(progress).toEqual([4, 8, 10]);
    expect(uploaded).toEqual({
      fileId: 'f1.txt',
      name: 'notes.txt',
      size: 10,
      contentType: 'text/plain',
    });
  });

  it('writes the digest as RFC 9530 does', async () => {
    // The SHA-256 of "abc".
    expect(await contentDigest(new Blob(['abc']))).toBe(
      'sha-256=:ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0=:',
    );
  });

  it('completes an empty file without parts', async () => {
    const calls = uploadServer({ partSize: 4 });
    await serverBackend('').upload('b1', new File([], 'empty.txt'));
    expect(calls.map((call) => call.method)).toEqual(['GET', 'POST', 'POST']);
  });

  it('sends a part again when it got no answer or the server failed, and gives up after three', async () => {
    const calls = uploadServer({
      partSize: 4,
      fail: (number, attempt) =>
        number === 2 && attempt === 1 ? 0 : number === 2 && attempt === 2 ? 500 : undefined,
    });
    await serverBackend('').upload('b1', new File(['0123456789'], 'a.txt'));
    expect(calls.filter((call) => call.url.endsWith('/parts/2'))).toHaveLength(3);

    const failing = uploadServer({ partSize: 4, fail: () => 500 });
    const error = await serverBackend('')
      .upload('b1', new File(['0123456789'], 'a.txt'))
      .catch((caught: unknown) => caught);
    expect(isBackendError(error) && error.status).toBe(500);
    expect(failing.filter((call) => call.method === 'PUT')).toHaveLength(3);
    // What the server held of it is thrown away.
    expect(failing.at(-1)).toMatchObject({ method: 'DELETE', url: '/api/v1/w/w1/uploads/up1' });
  });

  it('sends a part again that the database did not take in time', async () => {
    const calls = uploadServer({
      partSize: 4,
      fail: (number, attempt) => (number === 1 && attempt === 1 ? 503 : undefined),
    });
    await serverBackend('').upload('b1', new File(['0123456789'], 'a.txt'));
    expect(calls.filter((call) => call.url.endsWith('/parts/1'))).toHaveLength(2);
  });

  it('does not send a part again that the server refused', async () => {
    const calls = uploadServer({ partSize: 4, fail: () => 400 });
    const error = await serverBackend('')
      .upload('b1', new File(['0123456789'], 'a.txt'))
      .catch((caught: unknown) => caught);
    expect(isBackendError(error) && error.status).toBe(400);
    expect(calls.filter((call) => call.method === 'PUT')).toHaveLength(1);
  });

  it('throws the upload away when it is given up', async () => {
    const calls = uploadServer({ partSize: 4 });
    const controller = new AbortController();
    const uploading = serverBackend('').upload('b1', new File(['0123456789'], 'a.txt'), {
      signal: controller.signal,
      onProgress: () => {
        controller.abort();
      },
    });
    await expect(uploading).rejects.toThrow();
    expect(calls.some((call) => call.url.endsWith('/complete'))).toBe(false);
    expect(calls.at(-1)).toMatchObject({ method: 'DELETE', url: '/api/v1/w/w1/uploads/up1' });
  });

  it('refuses a file over the server’s limit before sending anything, with the limit', async () => {
    const calls = uploadServer({ partSize: 4, maxFileBytes: 5 });
    const error = await serverBackend('')
      .upload('b1', new File(['0123456789'], 'big.bin'))
      .catch((caught: unknown) => caught);
    expect(isBackendError(error) && [error.status, error.limit, error.reason]).toEqual([
      413,
      5,
      'file_too_large',
    ]);
    expect(calls.map((call) => call.method)).toEqual(['GET']);
  });

  it('attaches an uploaded file via the verified card attachments route', async () => {
    const calls: Call[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit) => {
      const json = JSON.parse(init.body as string) as { fileId: string };
      calls.push({ method: init.method ?? 'GET', url, size: undefined, headers: {}, json });
      return Promise.resolve(
        new Response(
          JSON.stringify({
            id: 'a1',
            containerId: 'b1',
            parentId: 'c1',
            type: 'attachment',
            title: 'notes.txt',
            fields: { fileId: json.fileId, size: 10, contentType: 'text/plain' },
            body: null,
            createdAt: 1,
            updatedAt: 1,
            createdBy: 'u',
            modifiedBy: 'u',
          }),
          { status: 201 },
        ),
      );
    });
    const attachment = await serverBackend('').createAttachment('b1', 'c1', {
      fileId: 'f1.txt',
      name: 'notes.txt',
      size: 10,
      contentType: 'text/plain',
    });
    expect(calls[0]).toMatchObject({
      method: 'POST',
      url: '/api/v1/w/w1/boards/b1/cards/c1/attachments',
      json: { fileId: 'f1.txt' },
    });
    expect(attachment).toMatchObject({ id: 'a1', fileId: 'f1.txt', name: 'notes.txt', size: 10 });
    expect(serverBackend('/base').fileUrl('b1', 'f1.txt')).toBe(
      '/base/api/v1/w/w1/containers/b1/files/f1.txt',
    );
  });

  it('reads a file’s bytes from its board', async () => {
    const urls: string[] = [];
    vi.stubGlobal('fetch', (url: string) => {
      urls.push(url);
      return Promise.resolve(new Response('bytes', { status: 200 }));
    });
    const bytes = await serverBackend('/base').fileBytes('b1', 'f1.txt');
    expect(await bytes.text()).toBe('bytes');
    expect(urls).toEqual(['/base/api/v1/w/w1/containers/b1/files/f1.txt']);
  });

  it('throws away an upload given up while it was being opened', async () => {
    const calls = uploadServer({ partSize: 4 });
    const controller = new AbortController();
    // The server answers the opening only once the reader has given up.
    const answer = globalThis.fetch;
    vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
      if (init.method === 'POST' && url.endsWith('/uploads')) controller.abort();
      return answer(url, init);
    });
    const uploading = serverBackend('').upload('b1', new File(['0123456789'], 'a.txt'), {
      signal: controller.signal,
    });
    await expect(uploading).rejects.toThrow();
    await vi.waitFor(() => {
      expect(calls.at(-1)).toMatchObject({ method: 'DELETE', url: '/api/v1/w/w1/uploads/up1' });
    });
    expect(calls.some((call) => call.method === 'PUT')).toBe(false);
  });

  it('leaves an upload alone whose completion got no answer: the file may be made', async () => {
    const calls = uploadServer({ partSize: 4 });
    const answer = globalThis.fetch;
    vi.stubGlobal('fetch', (url: string, init: RequestInit) =>
      url.endsWith('/complete') ? Promise.reject(new TypeError('no answer')) : answer(url, init),
    );
    const error = await serverBackend('')
      .upload('b1', new File(['0123456789'], 'a.txt'))
      .catch((caught: unknown) => caught);
    expect(isBackendError(error) && error.status).toBe(0);
    expect(calls.some((call) => call.method === 'DELETE')).toBe(false);
  });

  it('sends three files at once and the rest after, so as not to open more than the server keeps', async () => {
    let open = 0;
    let most = 0;
    uploadServer({ partSize: 4 });
    const answer = globalThis.fetch;
    vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
      if (init.method === 'POST' && url.endsWith('/uploads')) open += 1;
      most = Math.max(most, open);
      await new Promise((resolve) => setTimeout(resolve, 2));
      const response = await answer(url, init);
      if (url.endsWith('/complete')) open -= 1;
      return response;
    });
    const backend = serverBackend('');
    const files = Array.from({ length: 7 }, (_, at) => new File(['0123'], `f${String(at)}.txt`));
    await Promise.all(files.map((file) => backend.upload('b1', file)));
    expect(most).toBe(3);
    expect(open).toBe(0);
  });

  it('gives the turn of an upload given up while it waited to the next that waits', async () => {
    uploadServer({ partSize: 4 });
    const answer = globalThis.fetch;
    let release: () => void = () => undefined;
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    vi.stubGlobal('fetch', async (url: string, init: RequestInit) => {
      // The first three hold their part until released.
      if (url.includes('/parts/')) await held;
      return answer(url, init);
    });
    const backend = serverBackend('');
    const file = (at: number): File => new File(['0123'], `f${String(at)}.txt`);
    const running = [0, 1, 2].map((at) => backend.upload('b1', file(at)));
    const stops = [3, 4, 5].map(() => new AbortController());
    const given = stops.map((stop, at) =>
      backend.upload('b1', file(at + 3), { signal: stop.signal }).then(
        () => 'done',
        () => 'given up',
      ),
    );
    const last = backend.upload('b1', file(6));
    await new Promise((resolve) => setTimeout(resolve, 5));
    for (const stop of stops) stop.abort();
    expect(await Promise.all(given)).toEqual(['given up', 'given up', 'given up']);
    release();
    await Promise.all(running);
    await expect(last).resolves.toMatchObject({ fileId: 'f1.txt' });
  });
});
