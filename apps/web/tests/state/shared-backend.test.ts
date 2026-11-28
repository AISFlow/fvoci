/**
 * The backend of a reader by a board's public link, against a stand-in for
 * `fetch`: every reading goes to `/api/v1/shared/{containerId}` with the read
 * token and without the session's cookie, and nothing is ever written.
 */
import { afterEach, describe, expect, it, vi } from 'vitest';

import { SHARED_PLACE, SharedBackend } from '$lib/api/shared-backend.ts';
import { isBackendError } from '$lib/state/index.ts';

interface Sent {
  url: string;
  method: string;
  credentials: RequestCredentials | undefined;
}

/** Answers every request with `answer`, and keeps what was sent. */
function serve(answer: unknown): Sent[] {
  const sent: Sent[] = [];
  vi.stubGlobal('fetch', (url: string, init: RequestInit = {}) => {
    sent.push({ url, method: init.method ?? 'GET', credentials: init.credentials });
    return Promise.resolve(
      new Response(JSON.stringify(answer), { headers: { 'Content-Type': 'application/json' } }),
    );
  });
  return sent;
}

afterEach(() => {
  vi.unstubAllGlobals();
});

const BOARD = {
  id: 'b1',
  workspaceId: '',
  title: 'Shared',
  visibility: 'private',
  cardProperties: [],
  properties: {},
  createdAt: 1,
  updatedAt: 1,
};

describe('the backend of a public link', () => {
  it('reads the board by the token alone, in a place of its own, without the cookie', async () => {
    const sent = serve(BOARD);
    const backend = new SharedBackend('/base', 'b1', 'to/ken');
    const board = await backend.container('b1');
    expect(board.workspaceId).toBe(SHARED_PLACE);
    expect(sent).toEqual([
      { url: '/base/api/v1/shared/b1?readToken=to%2Fken', method: 'GET', credentials: 'omit' },
    ]);
    expect(backend.fileUrl('b1', 'f1')).toBe('/base/api/v1/shared/b1/files/f1?readToken=to%2Fken');
  });

  it('reads the views, the comments of a card and its files under the board', async () => {
    const sent = serve([]);
    const backend = new SharedBackend('', 'b1', 't');
    await backend.views('b1');
    await backend.comments('b1', 'c1');
    await backend.attachments('b1', 'c1');
    expect(sent.map((one) => one.url)).toEqual([
      '/api/v1/shared/b1/blocks?type=view&readToken=t',
      '/api/v1/shared/b1/blocks?parentId=c1&type=comment&readToken=t',
      '/api/v1/shared/b1/blocks?parentId=c1&type=attachment&readToken=t',
    ]);
  });

  it('has only the link’s board, and nobody in it', async () => {
    const sent = serve([]);
    const backend = new SharedBackend('', 'b1', 't');
    const other = await backend.container('b2').catch((error: unknown) => error);
    expect(isBackendError(other) && other.status).toBe(404);
    expect(backend.fileUrl('b2', 'f1')).toBeUndefined();
    expect(await backend.members('b1')).toEqual([]);
    expect(await backend.usersById(SHARED_PLACE, ['u1'])).toEqual([]);
    expect((await backend.me()).id).toBe('');
    expect(sent).toEqual([]);
  });

  it('writes nothing: every change is refused before a request is made', async () => {
    const sent = serve({});
    const backend = new SharedBackend('', 'b1', 't');
    const refusals = await Promise.all([
      backend.patchCard('b1', 'c1', { title: 'x' }).catch((error: unknown) => error),
      backend.createComment('b1', 'c1', { type: 'doc', content: [] }).catch((e: unknown) => e),
      backend.deleteAttachment('b1', 'c1', 'a1').catch((error: unknown) => error),
      backend.patchView('b1', 'v1', {}).catch((error: unknown) => error),
      backend.joinContainer('b1').catch((error: unknown) => error),
    ]);
    for (const refusal of refusals) expect(isBackendError(refusal) && refusal.status).toBe(403);
    expect(sent).toEqual([]);
  });

  it('memoizes initial container and cards reads across probes and reference resolution', async () => {
    const sent = serve(BOARD);
    const backend = new SharedBackend('', 'b1', 't');
    const [first, second] = await Promise.all([backend.container('b1'), backend.container('b1')]);
    expect(first.id).toBe('b1');
    expect(second.id).toBe('b1');
    expect(sent.map((one) => one.url)).toEqual(['/api/v1/shared/b1?readToken=t']);

    const cardsSent = serve([]);
    const cardsBackend = new SharedBackend('', 'b1', 't');
    await Promise.all([
      cardsBackend.cards('b1'),
      cardsBackend.references.resolve(SHARED_PLACE, ['c1']),
    ]);
    expect(cardsSent.filter((one) => one.url.startsWith('/api/v1/shared/b1/cards'))).toHaveLength(
      1,
    );
  });
});

describe('the backend of a wiki’s public link', () => {
  it('reads the tree and a page under the wiki by the token, and refuses every change', async () => {
    const sent = serve([]);
    const backend = new SharedBackend('', 'w1', 't');
    const wikis = backend.wiki;
    expect(await wikis.pages('w1')).toEqual([]);
    sent.length = 0;
    serve({
      id: 'p1',
      wikiId: 'w1',
      parentId: '',
      title: 'Planting',
      icon: '',
      order: 'a',
      body: null,
      createdBy: '',
      modifiedBy: '',
      createdAt: 1,
      updatedAt: 1,
    });
    const page = await wikis.page('w1', 'p1');
    expect([page.title, page.createdBy]).toEqual(['Planting', '']);
    await expect(wikis.pages('w2')).rejects.toSatisfy(isBackendError);
    for (const write of [
      () => wikis.createPage('w1', { title: 'x' }),
      () => wikis.patchPage('w1', 'p1', { title: 'y' }),
      () => wikis.deletePage('w1', 'p1'),
    ]) {
      await expect(write()).rejects.toSatisfy(isBackendError);
    }
  });

  it('reads no comments or versions of a page, asking the server nothing, and writes none', async () => {
    const sent = serve([]);
    const wikis = new SharedBackend('', 'w1', 't').wiki;
    expect([
      await wikis.comments('w1', 'p1'),
      await wikis.commentVersions('w1', 'p1', 'c1'),
      await wikis.bodyVersions.list('w1', 'p1'),
    ]).toEqual([[], [], []]);
    expect(sent).toEqual([]);
    for (const write of [
      () => wikis.createComment('w1', 'p1', { type: 'doc', content: [] }),
      () => wikis.deleteComment('w1', 'p1', 'c1'),
      () => wikis.restoreComment('w1', 'p1', 'c1'),
    ]) {
      await expect(write()).rejects.toSatisfy(isBackendError);
    }
  });

  it('asks the server at the wiki’s addresses of the link', async () => {
    const sent = serve([]);
    const backend = new SharedBackend('', 'w1', 'to/ken');
    await backend.wiki.pages('w1');
    expect(sent.map((one) => [one.url, one.credentials])).toEqual([
      ['/api/v1/shared/w1/pages?readToken=to%2Fken', 'omit'],
    ]);
  });
});

describe('the backend of a course’s public link', () => {
  it('reads the outline and a unit under the course by the token, and refuses every change', async () => {
    const sent = serve([]);
    const backend = new SharedBackend('', 'c1', 't');
    const courses = backend.course;
    expect(await courses.units('c1')).toEqual([]);
    sent.length = 0;
    serve({
      id: 'u1',
      courseId: 'c1',
      kind: 'unit',
      parentId: '',
      title: 'Lesson 1',
      order: 'a',
      body: null,
      createdBy: '',
      modifiedBy: '',
      createdAt: 1,
      updatedAt: 1,
    });
    const unit = await courses.unit('c1', 'u1');
    expect([unit.title, unit.createdBy, unit.kind]).toEqual(['Lesson 1', '', 'unit']);
    await expect(courses.units('c2')).rejects.toSatisfy(isBackendError);
    for (const write of [
      () => courses.createUnit('c1', { title: 'x' }),
      () => courses.patchUnit('c1', 'u1', { title: 'y' }),
      () => courses.deleteUnit('c1', 'u1'),
      () => courses.watchUnit('shared', 'u1', ''),
      () => courses.unwatchUnit('shared', 'u1', ''),
      () => courses.putUnitProgress('c1', 'u1', { completed: true }),
      () =>
        courses.putSchedule('c1', {
          startDate: '2026-05-01',
          targetDate: '2026-05-10',
          studyDays: 31,
        }),
      () => courses.deleteSchedule('c1'),
      () => courses.redistributeSchedule('c1', {}),
      () => courses.redistributeMySchedule('c1', {}),
      () => courses.putUnitSchedule('c1', 'u1', { plannedDate: '2026-05-02' }),
      () => courses.meetings.create('c1', { title: 'Sync', startsAt: 1000 }),
      () => courses.meetings.patch('c1', 'm1', { title: 'Sync 2' }),
      () => courses.meetings.delete('c1', 'm1'),
      () => courses.meetings.putAttendance('c1', 'm1', 'u1', { status: 'present' }),
      () => courses.exams.create('c1', { title: 'Midterm', maxScore: 100 }),
      () => courses.exams.patch('c1', 'e1', { title: 'Final' }),
      () => courses.exams.delete('c1', 'e1'),
      () => courses.exams.putMyScore('c1', 'e1', { score: 90 }),
      () => courses.exams.deleteMyScore('c1', 'e1'),
    ]) {
      await expect(write()).rejects.toSatisfy(isBackendError);
    }
  });

  it('reads no comments, versions, subscriptions, progress, or schedules of a unit, asking the server nothing', async () => {
    const sent = serve([]);
    const courses = new SharedBackend('', 'c1', 't').course;
    expect([
      await courses.comments('c1', 'u1'),
      await courses.commentVersions('c1', 'u1', 'cm1'),
      await courses.bodyVersions.list('c1', 'u1'),
      await courses.watching('shared', ''),
      await courses.myProgress('c1'),
      await courses.progressSummary('c1'),
      await courses.schedule('c1'),
      await courses.learningToday('shared'),
      await courses.meetings.list('c1'),
      await courses.exams.list('c1'),
    ]).toEqual([
      [],
      [],
      [],
      [],
      [],
      { totalUnits: 0, members: [] },
      { schedule: null, today: '1970-01-01', todayCount: 0, overdueCount: 0, units: [] },
      { today: '1970-01-01', todayUnits: [], overdueUnits: [] },
      [],
      [],
    ]);
    expect(sent).toEqual([]);
    for (const write of [
      () => courses.createComment('c1', 'u1', { type: 'doc', content: [] }),
      () => courses.deleteComment('c1', 'u1', 'cm1'),
      () => courses.restoreComment('c1', 'u1', 'cm1'),
    ]) {
      await expect(write()).rejects.toSatisfy(isBackendError);
    }
  });

  it('asks the server at the course’s addresses of the link without credentials', async () => {
    const sent = serve([]);
    const backend = new SharedBackend('', 'c1', 'to/ken');
    await backend.course.units('c1');
    expect(sent.map((one) => [one.url, one.credentials])).toEqual([
      ['/api/v1/shared/c1/units?readToken=to%2Fken', 'omit'],
    ]);
  });
});
