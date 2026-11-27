/**
 * Archives on the server, against a stand-in for `fetch`: an export read
 * with its progress and saved under the server's name, a refusal read as
 * every other one, and an import sent in parts and ended by `importArchive`.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { serverBackend } from './support.ts';
import { dispositionName } from '$lib/api/download.ts';
import { isBackendError } from '$lib/state/index.ts';

afterEach(() => {
  vi.unstubAllGlobals();
});

/** A body that arrives in the given pieces. */
function streamOf(pieces: string[]): ReadableStream<Uint8Array> {
  const encoder = new TextEncoder();
  return new ReadableStream({
    start(controller) {
      for (const piece of pieces) controller.enqueue(encoder.encode(piece));
      controller.close();
    },
  });
}

describe('an export', () => {
  it('is read with its progress, under the name the server gives', async () => {
    const urls: string[] = [];
    vi.stubGlobal('fetch', (url: string) => {
      urls.push(url);
      return Promise.resolve(
        new Response(streamOf(['PK', '1234', 'ab']), {
          status: 200,
          headers: {
            'Content-Length': '8',
            'Content-Disposition': 'attachment; filename=archive-2026-11-11.fvoci',
          },
        }),
      );
    });
    const seen: [number, number][] = [];
    const archive = await serverBackend('/base').exportContainer('b1', {
      onProgress: (progress) => seen.push([progress.sentBytes, progress.totalBytes]),
    });
    expect(urls).toEqual(['/base/api/v1/w/w1/containers/b1/archive/export']);
    expect(archive.name).toBe('archive-2026-11-11.fvoci');
    expect(await archive.bytes.text()).toBe('PK1234ab');
    expect(seen).toEqual([
      [0, 8],
      [2, 8],
      [6, 8],
      [8, 8],
    ]);
  });

  it('lets a refusal through as every request does, asking for it once', async () => {
    const urls: string[] = [];
    vi.stubGlobal('fetch', (url: string) => {
      urls.push(url);
      return Promise.resolve(
        new Response(
          JSON.stringify({
            type: '/problems/sharing-disabled',
            title: 'Sharing disabled',
            status: 403,
            detail: 'no',
            reason: 'sharing_disabled',
          }),
          { status: 403, headers: { 'Content-Type': 'application/problem+json' } },
        ),
      );
    });
    const refused = await serverBackend('')
      .exportWorkspace('t1')
      .catch((error: unknown) => error);
    expect(isBackendError(refused) && [refused.status, refused.reason]).toEqual([
      403,
      'sharing_disabled',
    ]);
    expect(urls).toEqual(['/api/v1/w/t1/archive/export']);
  });

  it('reads the name of a Content-Disposition, plain, quoted or encoded', () => {
    expect(dispositionName('attachment; filename=a.fvoci')).toBe('a.fvoci');
    expect(dispositionName('attachment; filename="a b.zip"')).toBe('a b.zip');
    expect(dispositionName("attachment; filename*=UTF-8''%EB%B3%B4%EB%93%9C.zip")).toBe('보드.zip');
    expect(dispositionName(null)).toBeUndefined();
  });
});

describe('an import', () => {
  function importServer(maxImportBytes: number) {
    const calls: { method: string; url: string; json: unknown }[] = [];
    vi.stubGlobal('fetch', (url: string, init: RequestInit = {}) => {
      const method = init.method ?? 'GET';
      calls.push({
        method,
        url,
        json: typeof init.body === 'string' ? JSON.parse(init.body) : undefined,
      });
      const answer = (status: number, body?: unknown) =>
        Promise.resolve(
          new Response(body === undefined ? null : JSON.stringify(body), {
            status,
            headers: {
              'Content-Type': status >= 400 ? 'application/problem+json' : 'application/json',
            },
          }),
        );
      if (url.endsWith('/client-config')) return answer(200, { maxFileBytes: 10, maxImportBytes });
      if (url.endsWith('/archive/uploads')) {
        return answer(201, { uploadId: 'up9', partSize: 4, partCount: 2, expiresAt: 1 });
      }
      if (url.includes('/parts/')) return answer(204);
      if (url.endsWith('/archive/import')) {
        const body = (typeof init.body === 'string' ? JSON.parse(init.body) : {}) as {
          format?: 'fvoci' | 'trello' | 'notion';
        };
        return answer(200, {
          format: body.format ?? 'fvoci',
          imported: { boards: 1, wikis: 0, cards: 2, pages: 0, comments: 0, files: 0 },
          membersLeftOut: 2,
          containerIds: ['c1'],
          skipped: [],
          warnings: [],
        });
      }
      return answer(404, { type: '/problems/not-found', status: 404, detail: 'no' });
    });
    return calls;
  }

  it('opens an import upload, sends the parts and names the upload to import', async () => {
    const calls = importServer(100);
    const seen: number[] = [];
    const imported = await serverBackend('').importArchive(
      't1',
      new File(['12345678'], 'boards.fvoci', { type: 'application/zip' }),
      { onProgress: (progress) => seen.push(progress.sentBytes) },
    );
    expect(imported).toEqual({
      format: 'fvoci',
      imported: { boards: 1, wikis: 0, cards: 2, pages: 0, comments: 0, files: 0 },
      membersLeftOut: 2,
      containerIds: ['c1'],
      skipped: [],
      warnings: [],
    });
    expect(calls.map((call) => `${call.method} ${call.url}`)).toEqual([
      'GET /api/v1/client-config',
      'POST /api/v1/w/t1/archive/uploads',
      'PUT /api/v1/w/t1/uploads/up9/parts/1',
      'PUT /api/v1/w/t1/uploads/up9/parts/2',
      'POST /api/v1/w/t1/archive/import',
    ]);
    expect(calls[1]?.json).toEqual({
      name: 'boards.fvoci',
      size: 8,
      contentType: 'application/zip',
    });
    expect(calls[4]?.json).toEqual({ uploadId: 'up9' });
    expect(seen).toEqual([4, 8]);
  });

  it('forwards explicit format (`trello` / `notion`) on the import request body', async () => {
    const calls = importServer(100);
    const trello = await serverBackend('').importArchive(
      't1',
      new File(['{"lists":[]}'], 'trello.json', { type: 'application/json' }),
      { format: 'trello' },
    );
    expect(trello.format).toBe('trello');
    expect(calls.at(-1)?.json).toEqual({ uploadId: 'up9', format: 'trello' });

    const notion = await serverBackend('').importArchive(
      't1',
      new File(['PK123456'], 'notion.zip', { type: 'application/zip' }),
      { format: 'notion' },
    );
    expect(notion.format).toBe('notion');
    expect(calls.at(-1)?.json).toEqual({ uploadId: 'up9', format: 'notion' });
  });

  it('refuses an archive over the server’s limit before sending anything', async () => {
    const calls = importServer(5);
    const refused = await serverBackend('')
      .importArchive('t1', new File(['12345678'], 'big.fvoci'))
      .catch((error: unknown) => error);
    expect(isBackendError(refused) && [refused.status, refused.limit, refused.reason]).toEqual([
      413,
      5,
      'archive_too_large',
    ]);
    expect(calls.map((call) => call.url)).toEqual(['/api/v1/client-config']);
  });
});

describe('an export that does not arrive whole', () => {
  it('is no archive when the body ends before its length', async () => {
    vi.stubGlobal('fetch', () =>
      Promise.resolve(
        new Response(streamOf(['PK12']), { status: 200, headers: { 'Content-Length': '10' } }),
      ),
    );
    const refused = await serverBackend('')
      .exportContainer('b1')
      .catch((error: unknown) => error);
    expect(isBackendError(refused) && refused.status).toBe(0);
  });
});
