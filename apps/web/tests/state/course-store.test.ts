/**
 * The store of an open course (`state/course.svelte.ts`) over the backend in
 * memory: loads chapters and units as a 2-level tree, joins open courses,
 * follows live updates from others, undoes creation/rename/move/delete, and
 * rejects non-course container IDs.
 */
import { describe, expect, it, vi } from 'vitest';

import { pageRows } from '$lib/logic/page-tree.ts';
import { keepVersion, unitVersions } from '$lib/features/card/body-versions.svelte.ts';
import {
  BackendError,
  CourseStore,
  referenceTitles,
  Workspace,
  type Change,
  type CourseUnit,
} from '$lib/state/index.ts';

import { settle, setup, text } from './support.ts';

async function openCourse(visibility: 'open' | 'private' = 'private') {
  const made = setup();
  await made.workspace.load();
  const workspaceId = made.world.workspace.id;
  const course = await made.backend.course.createCourse(workspaceId, {
    title: 'Onboarding',
    visibility,
  });
  const store = new CourseStore(made.workspace, course.id);
  await store.load();
  return { ...made, workspaceId, courseId: course.id, store };
}

const titles = (store: CourseStore): string[] =>
  pageRows<CourseUnit>(store.tree, () => true).map(
    (row) =>
      `${'  '.repeat(row.depth)}${row.page.kind === 'chapter' ? '[C] ' : ''}${row.page.title}`,
  );

describe('an open course', () => {
  it('reads the course, its chapters and units as a tree, and the reader’s role', async () => {
    const made = setup();
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Onboarding',
      visibility: 'private',
    });
    const ch = await made.backend.course.createUnit(course.id, {
      kind: 'chapter',
      title: 'Chapter 1',
    });
    await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Unit 1',
      parentId: ch.id,
    });
    const store = new CourseStore(made.workspace, course.id);
    await store.load();
    expect(store.status).toBe('ready');
    expect(store.course?.title).toBe('Onboarding');
    expect(titles(store)).toEqual(['[C] Chapter 1', '  Unit 1']);
    expect([store.role, store.canEdit, store.canManage]).toEqual(['admin', true, true]);
    store.close();
  });

  it('reports missing when given a board ID instead of a course ID', async () => {
    const { workspace, backend, world } = setup();
    await workspace.load();
    const [board] = await backend.boards(world.workspace.id);
    const store = new CourseStore(workspace, board?.id ?? '');
    await store.load();
    expect(store.missing).toBe(true);
    expect(store.status).toBe('failed');
    store.close();
  });

  it('reads an open course unjoined and lets a workspace member join as commenter by default', async () => {
    const made = await openCourse('open');
    const otherWorkspace = new Workspace({ backend: made.other, locale: () => 'en' });
    await otherWorkspace.load();
    const store = new CourseStore(otherWorkspace, made.courseId);
    await store.load();
    expect([store.joinable, store.missing, store.status, store.canEdit]).toEqual([
      true,
      false,
      'ready',
      false,
    ]);
    expect(await store.join()).toBe(true);
    expect([
      store.joinable,
      store.status,
      store.role,
      store.canEdit,
      store.permissions.canComment,
    ]).toEqual([false, 'ready', 'commenter', false, true]);
    store.close();
    made.store.close();
  });

  it('creates, moves, deletes, and undoes chapters and units', async () => {
    const made = await openCourse();
    const ch = await made.store.createUnit({ kind: 'chapter', title: 'Part 1' });
    const u1 = await made.store.createUnit({ kind: 'unit', title: 'Lesson 1', parentId: ch?.id });
    const u2 = await made.store.createUnit({ kind: 'unit', title: 'Lesson 2' });
    if (!ch || !u1 || !u2) throw new Error('expected units');
    expect(titles(made.store)).toEqual(['[C] Part 1', '  Lesson 1', 'Lesson 2']);

    await made.store.moveUnit(u2.id, { parentId: ch.id });
    expect(titles(made.store)).toEqual(['[C] Part 1', '  Lesson 1', '  Lesson 2']);

    await made.workspace.undoLast();
    expect(titles(made.store)).toEqual(['[C] Part 1', '  Lesson 1', 'Lesson 2']);

    const shown = made.store.showUnit(u1.id);
    await settle();
    expect(shown?.status).toBe('ready');

    await made.store.deleteUnit(ch.id);
    expect(titles(made.store)).toEqual(['Lesson 2']);
    expect(shown?.removed).toEqual({ by: 'reader', title: 'Lesson 1' });

    await made.workspace.undoLast();
    expect(titles(made.store)).toEqual(['[C] Part 1', '  Lesson 1', 'Lesson 2']);
    made.store.close();
  });

  it('follows live changes made by another editor', async () => {
    const made = await openCourse();
    await made.backend.addMember(made.courseId, made.other.userId, 'editor');
    const ch = await made.other.course.createUnit(made.courseId, {
      kind: 'chapter',
      title: 'Their Chapter',
    });
    const u = await made.other.course.createUnit(made.courseId, {
      kind: 'unit',
      title: 'Their Unit',
      parentId: ch.id,
    });
    await settle();
    expect(titles(made.store)).toEqual(['[C] Their Chapter', '  Their Unit']);

    const shown = made.store.showUnit(u.id);
    await settle();
    await made.other.course.deleteUnit(made.courseId, ch.id);
    await settle();
    expect(titles(made.store)).toEqual([]);
    expect(shown?.missing).toBe(true);
    expect(shown?.removed).toEqual({ by: 'others', title: 'Their Unit' });
    made.store.close();
  });

  it('loads unit comments, receives live comment changes, toggles watching, and updates reference titles', async () => {
    const made = await openCourse('open');
    await made.backend.addMember(made.courseId, made.other.userId, 'editor');
    const ch = await made.store.createUnit({ kind: 'chapter', title: 'Ch 1' });
    const u = await made.store.createUnit({ kind: 'unit', title: 'Unit A', parentId: ch?.id });
    if (!ch || !u) throw new Error('expected unit');

    const shown = made.store.showUnit(u.id);
    await settle();
    if (!shown) throw new Error('expected open unit');
    expect(shown.status).toBe('ready');
    expect(shown.comments.status).toBe('ready');
    expect(shown.canWatch).toBe(true);
    expect(shown.watching).toBe(true);

    // Toggle watching off and back on.
    await shown.setWatching(false);
    expect(shown.watching).toBe(false);
    await shown.setWatching(true);
    expect(shown.watching).toBe(true);

    // Add comment, receive live comment from other, delete and undo.
    expect(await shown.comments.addComment(text('Mine'))).toBe(true);
    const theirs = await made.other.course.createComment(made.courseId, u.id, text('Theirs'));
    await settle();
    expect(shown.comments.comments.map((c) => c.id)).toContain(theirs.id);

    const mine = shown.comments.comments.find((c) => c.id !== theirs.id);
    if (!mine) throw new Error('expected own comment');
    await shown.comments.deleteComment(mine.id);
    expect(shown.comments.comments.some((c) => c.id === mine.id)).toBe(false);
    await made.workspace.undoLast();
    expect(shown.comments.comments.some((c) => c.id === mine.id)).toBe(true);

    // Keep version works once room is open and writable.
    const outcome = await keepVersion(unitVersions(shown));
    expect(outcome).toBe('notNow');

    // ReferenceTitles tracks live renames of units.
    const titlesStore = referenceTitles(made.workspace);
    expect(titlesStore.of(u.id)).toBeUndefined();
    await settle();
    expect(titlesStore.of(u.id)).toMatchObject({ id: u.id, kind: 'unit', title: 'Unit A' });
    await made.store.changeUnitTitle(u.id, 'Unit A Renamed');
    await settle();
    expect(titlesStore.of(u.id)?.title).toBe('Unit A Renamed');

    made.store.close();
  });

  it('closes and clears the open unit when the course is deleted or access is revoked, and opens a fresh store on recovery', async () => {
    const made = await openCourse('private');
    await made.backend.addMember(made.courseId, made.other.userId, 'editor');
    const unit = await made.backend.course.createUnit(made.courseId, {
      kind: 'unit',
      title: 'Lesson 1',
    });

    const otherWorkspace = new Workspace({ backend: made.other, locale: () => 'en' });
    await otherWorkspace.load();
    const store = new CourseStore(otherWorkspace, made.courseId);
    await store.load();
    const firstOpen = store.showUnit(unit.id);
    await settle();
    expect(firstOpen?.status).toBe('ready');

    // A containerDeleted event on a private course marks it missing and drops openUnit.
    await made.backend.deleteContainer(made.courseId);
    await settle();
    expect(store.missing).toBe(true);
    expect(store.openUnit).toBeUndefined();

    // Restoring the course and reloading creates a fresh UnitStore instead of reusing the stale one.
    await made.backend.restoreContainer(made.courseId);
    await store.load();
    expect(store.missing).toBe(false);
    const secondOpen = store.showUnit(unit.id);
    await settle();
    expect(secondOpen).toBeDefined();
    expect(secondOpen).not.toBe(firstOpen);
    expect(secondOpen?.status).toBe('ready');

    // A 404/403 on reload (e.g., member removed) also marks missing and clears openUnit.
    await made.backend.removeMember(made.courseId, made.other.userId);
    await store.load();
    expect(store.missing).toBe(true);
    expect(store.openUnit).toBeUndefined();

    store.close();
    made.store.close();
  });

  it('reports 401 session expiry on course resync and unit refresh', async () => {
    const made = setup();
    let emit: ((change: Change) => void) | undefined;
    const origWatch = made.backend.watchContainer.bind(made.backend);
    made.backend.watchContainer = (id, listener) => {
      const stop = origWatch(id, listener);
      emit = listener;
      return stop;
    };
    await made.workspace.load();
    const course = await made.backend.course.createCourse(made.world.workspace.id, {
      title: 'Session Check',
      visibility: 'private',
    });
    const unit = await made.backend.course.createUnit(course.id, {
      kind: 'unit',
      title: 'Unit 1',
    });
    const store = new CourseStore(made.workspace, course.id);
    await store.load();
    const open = store.showUnit(unit.id);
    await settle();
    expect(open?.status).toBe('ready');

    // 401 during UnitStore.refresh() reports session expiration to Workspace.
    made.backend.course.unit = () => Promise.reject(new BackendError(401, { detail: 'expired' }));
    await open?.refresh();
    expect(made.workspace.status).toBe('signedOut');

    // Reset workspace status and verify 401 during CourseStore resync also reports it.
    made.workspace.status = 'ready';
    made.backend.container = () => Promise.reject(new BackendError(401, { detail: 'expired' }));
    emit?.({ kind: 'resync' });
    await settle();
    expect(made.workspace.status).toBe('signedOut');

    // A late 401 from resync after store.close() must not sign out the workspace.
    made.workspace.status = 'ready';
    let rejectLate: ((reason?: unknown) => void) | undefined;
    made.backend.container = () =>
      new Promise((_, reject) => {
        rejectLate = reject;
      });
    emit?.({ kind: 'resync' });
    store.close();
    rejectLate?.(new BackendError(401, { detail: 'late' }));
    await settle();
    expect(made.workspace.status).toBe('ready');
  });

  it('does not evict an open course when a member is removed, and tears down openUnit on join 404', async () => {
    const made = await openCourse('open');
    await made.backend.addMember(made.courseId, made.other.userId, 'editor');
    const unit = await made.backend.course.createUnit(made.courseId, {
      kind: 'unit',
      title: 'Open Unit',
    });

    const otherWorkspace = new Workspace({ backend: made.other, locale: () => 'en' });
    await otherWorkspace.load();
    const evictSpy = vi.spyOn(otherWorkspace, 'evictContainer');
    const store = new CourseStore(otherWorkspace, made.courseId);
    await store.load();
    store.showUnit(unit.id);
    await settle();

    // Removing the member from an open course keeps it readable (joinable) without eviction.
    await made.backend.removeMember(made.courseId, made.other.userId);
    await settle();
    expect(store.joinable).toBe(true);
    expect(evictSpy).not.toHaveBeenCalled();

    // If the course disappears before join() finishes, join() evicts and tears down openUnit.
    made.other.joinContainer = () => Promise.reject(new BackendError(404, { detail: 'gone' }));
    expect(await store.join()).toBe(false);
    expect([store.missing, store.status, store.openUnit]).toEqual([true, 'failed', undefined]);
    expect(evictSpy).toHaveBeenCalledWith(made.courseId);

    store.close();
    made.store.close();
  });
});
