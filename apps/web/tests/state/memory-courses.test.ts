/**
 * The courses of the backend in memory keep the server's rules
 * (`crates/store/src/units.rs`): a course is a container of kind `course` that
 * board/card operations and duplication refuse, its nodes are a 2-level tree
 * (`chapter` at top only, `unit` at top or under a chapter), chapters have no
 * body, counts are capped separately (200 chapters / 2000 units), deleting a
 * chapter takes its units along, restoring an orphaned unit moves it to top,
 * and open courses are readable without joining.
 */
import { describe, expect, it } from 'vitest';

import { pageTree } from '$lib/logic/page-tree.ts';
import { parseIsoDateDays } from '$lib/state/memory/archive.ts';
import { MemoryBackend } from '$lib/state/memory/backend.ts';
import { seedWorld } from '$lib/state/memory/seed.ts';
import { BackendError, isOwnSpace, type CourseUnit } from '$lib/state/index.ts';

import { SMALL, setup, text } from './support.ts';

const rejection = async (promise: Promise<unknown>): Promise<BackendError> => {
  try {
    await promise;
  } catch (error) {
    if (error instanceof BackendError) return error;
    throw error;
  }
  throw new Error('the call did not reject');
};

async function aCourse(made = setup(), visibility: 'open' | 'private' = 'private') {
  const workspaceId = made.world.workspace.id;
  const course = await made.backend.course.createCourse(workspaceId, {
    title: 'Onboarding',
    visibility,
  });
  return { ...made, workspaceId, course };
}

/** The titles of a course's tree, as nested lists. */
async function shape(made: Awaited<ReturnType<typeof aCourse>>): Promise<unknown[]> {
  const units = await made.backend.course.units(made.course.id);
  const walk = (nodes: ReturnType<typeof pageTree<CourseUnit>>): unknown[] =>
    nodes.map((node) =>
      node.children.length > 0 ? [node.page.title, walk(node.children)] : node.page.title,
    );
  return walk(pageTree(units));
}

describe('a course in memory', () => {
  it('is filed in the maker’s default category with admin role, outside boards and templates', async () => {
    const made = await aCourse();
    expect(made.course.kind).toBe('course');
    expect(made.course.cardProperties).toEqual([]);
    const categories = await made.backend.categories(made.workspaceId);
    const system = categories.find((category) => category.type === 'system');
    expect(system?.containerIds).toContain(made.course.id);
    const [member] = await made.backend.members(made.course.id);
    expect(member?.role).toBe('admin');
    expect((await made.backend.course.courses(made.workspaceId)).map((c) => c.id)).toEqual([
      made.course.id,
    ]);
    expect((await made.backend.boards(made.workspaceId)).some((b) => b.id === made.course.id)).toBe(
      false,
    );
    expect(
      (await made.backend.templates(made.workspaceId)).some((b) => b.id === made.course.id),
    ).toBe(false);
    // Courses can be duplicated into a copy or a course template (excluding personal progress).
    const copied = await made.backend.duplicateContainer(made.course.id, false, made.workspaceId);
    expect(copied.kind).toBe('course');
    expect(copied.isTemplate).toBe(false);
    expect(copied.id).not.toBe(made.course.id);
    const asTemplate = await made.backend.duplicateContainer(
      made.course.id,
      true,
      made.workspaceId,
    );
    expect(asTemplate.kind).toBe('course');
    expect(asTemplate.isTemplate).toBe(true);
    expect(
      (await made.backend.templates(made.workspaceId)).some((b) => b.id === asTemplate.id),
    ).toBe(true);
    // Board/card endpoints do not reach a course.
    expect((await rejection(made.backend.cards(made.course.id))).status).toBe(404);
  });

  it('duplicates and archives courses with schedule and amounts while excluding personal progress', async () => {
    const made = await aCourse();
    const courses = made.backend.course;
    const ch1 = await courses.createUnit(made.course.id, { kind: 'chapter', title: 'Chapter 1' });
    const u1 = await courses.createUnit(made.course.id, {
      kind: 'unit',
      title: 'Lesson 1',
      parentId: ch1.id,
    });
    made.world.courseSchedules.set(made.course.id, {
      startDate: '2026-12-01',
      targetDate: '2026-12-14',
      studyDays: 31,
      updatedAt: 1,
    });
    await courses.putUnitProgress(made.course.id, u1.id, {
      completed: true,
      lastPosition: 'secret-pos',
    });

    // Duplication preserves chapters, units, and shared schedule, never personal progress.
    const dup = await made.backend.duplicateContainer(made.course.id, false, made.workspaceId);
    expect(made.world.courseSchedules.get(dup.id)).toMatchObject({
      startDate: '2026-12-01',
      targetDate: '2026-12-14',
      studyDays: 31,
    });
    expect(await courses.myProgress(dup.id)).toEqual([]);
    const dupUnits = await courses.units(dup.id);
    expect(dupUnits.map((n) => n.title)).toEqual(['Chapter 1', 'Lesson 1']);

    // Export -> import preserves chapters, units, and shared schedule, never personal progress.
    const exported = await made.backend.exportContainer(made.course.id);
    const textArchive = await exported.bytes.text();
    expect(textArchive).toContain('"type":"courseSchedule"');
    expect(textArchive).not.toContain('secret-pos');
    expect(textArchive).not.toContain('unitProgress');

    const file = new File([exported.bytes], exported.name);
    const summary = await made.backend.importArchive(made.workspaceId, file);
    expect(summary.imported.courses).toBe(1);
    expect(summary.imported.units).toBe(1);
    const importedId = summary.containerIds[0] ?? '';
    expect(made.world.courseSchedules.get(importedId)).toMatchObject({
      startDate: '2026-12-01',
      targetDate: '2026-12-14',
      studyDays: 31,
    });
    expect(await courses.myProgress(importedId)).toEqual([]);
  });

  it('rejects malformed courseSchedule archives without mutating the workspace', async () => {
    const made = await aCourse();
    const beforeCount = made.world.containers.size;
    const courseLine = JSON.stringify({
      type: 'container',
      data: {
        id: 'c1',
        workspaceId: made.workspaceId,
        visibility: 'private',
        kind: 'course',
        title: 'Bad',
      },
    });
    const boardLine = JSON.stringify({
      type: 'container',
      data: {
        id: 'b1',
        workspaceId: made.workspaceId,
        visibility: 'private',
        kind: 'board',
        title: 'Board',
      },
    });
    const cases = [
      [
        courseLine,
        JSON.stringify({
          type: 'courseSchedule',
          data: {
            boardId: 'c1',
            startDate: '2026-12-01',
            targetDate: '2026-12-10',
            studyDays: 31,
          },
        }),
      ],
      [
        courseLine,
        JSON.stringify({
          type: 'courseSchedule',
          data: {
            containerId: 'c1',
            startDate: '2026-12-01',
            targetDate: '2026-12-10',
            studyDays: 1.5,
          },
        }),
      ],
      [
        courseLine,
        JSON.stringify({
          type: 'courseSchedule',
          data: {
            containerId: 'c1',
            startDate: '2026-12-01',
            targetDate: '2026-12-10',
            studyDays: 31,
            plannedDate: '2026-12-05',
          },
        }),
      ],
      [
        boardLine,
        JSON.stringify({
          type: 'courseSchedule',
          data: {
            containerId: 'b1',
            startDate: '2026-12-01',
            targetDate: '2026-12-10',
            studyDays: 31,
          },
        }),
      ],
      [
        courseLine,
        JSON.stringify({
          type: 'courseSchedule',
          data: {
            containerId: 'c1',
            startDate: '2026-02-30',
            targetDate: '2026-03-05',
            studyDays: 31,
          },
        }),
      ],
      [
        courseLine,
        JSON.stringify({
          type: 'courseSchedule',
          data: {
            containerId: 'missing',
            startDate: '2026-12-01',
            targetDate: '2026-12-10',
            studyDays: 31,
          },
        }),
      ],
      [
        courseLine,
        JSON.stringify({
          type: 'courseSchedule',
          data: {
            containerId: 'c1',
            startDate: '2020-01-01',
            targetDate: '2035-01-01',
            studyDays: 31,
          },
        }),
      ],
    ];
    for (const lines of cases) {
      const payload = [JSON.stringify({ version: 'fvoci-memory-1' }), ...lines].join('\n');
      const file = new File([payload], 'bad.fvoci');
      expect((await rejection(made.backend.importArchive(made.workspaceId, file))).status).toBe(
        400,
      );
    }
    expect(made.world.containers.size).toBe(beforeCount);
  });

  it('shifts built-in course template schedule and unit dates and remaps internal references', async () => {
    const { world, readerId } = seedWorld([SMALL, { ...SMALL, key: 'second' }]);
    const backend = new MemoryBackend(world, readerId);
    const workspaceId = world.workspace.id;
    const templates = await backend.templates(workspaceId);
    const builtin = templates.find((item) => item.isBuiltin === true && item.kind === 'course');
    if (!builtin) throw new Error('expected built-in course template');
    const copy = await backend.duplicateContainer(builtin.id, false, workspaceId);
    expect(copy.isBuiltin).toBe(false);
    expect(copy.properties.templateKey).toBeUndefined();
    expect(copy.properties.templateSchedule).toBeUndefined();
    const today = new Date(world.now()).toISOString().slice(0, 10);
    expect(world.courseSchedules.get(copy.id)?.startDate).toBe(today);
    const units = await backend.course.units(copy.id);
    const concepts = units.find((u) => u.title === 'Summarize Core Concepts');
    const practice = units.find((u) => u.title === 'Solve Practice Problems');
    if (!concepts || !practice) throw new Error('expected template units');
    const detail = await backend.course.unit(copy.id, practice.id);
    expect(JSON.stringify(detail.body)).toContain(concepts.id);
  });

  it('enforces depth-2, chapter-only parenting, top-only chapters, and bodyless chapters', async () => {
    const made = await aCourse();
    const courses = made.backend.course;
    const ch1 = await courses.createUnit(made.course.id, { kind: 'chapter', title: 'Chapter 1' });
    const u1 = await courses.createUnit(made.course.id, {
      kind: 'unit',
      title: 'Unit 1',
      parentId: ch1.id,
    });
    const u2 = await courses.createUnit(made.course.id, { kind: 'unit', title: 'Unit 2' });
    expect(await shape(made)).toEqual([['Chapter 1', ['Unit 1']], 'Unit 2']);

    // Chapter cannot be created under a chapter or unit.
    const nestedChapter = await rejection(
      courses.createUnit(made.course.id, { kind: 'chapter', title: 'Sub', parentId: ch1.id }),
    );
    expect([nestedChapter.status, nestedChapter.refusalAt('/parentId') !== undefined]).toEqual([
      400,
      true,
    ]);

    // Unit cannot be created under another unit.
    const nestedUnit = await rejection(
      courses.createUnit(made.course.id, { kind: 'unit', title: 'Sub', parentId: u1.id }),
    );
    expect([nestedUnit.status, nestedUnit.refusalAt('/parentId') !== undefined]).toEqual([
      400,
      true,
    ]);

    // Moving a chapter under anything is refused.
    const ch2 = await courses.createUnit(made.course.id, { kind: 'chapter', title: 'Chapter 2' });
    expect(
      (await rejection(courses.moveUnit(made.course.id, ch2.id, { parentId: ch1.id }))).status,
    ).toBe(400);

    // Moving a unit under another unit is refused; under a chapter succeeds.
    expect(
      (await rejection(courses.moveUnit(made.course.id, u2.id, { parentId: u1.id }))).status,
    ).toBe(400);
    await courses.moveUnit(made.course.id, u2.id, { parentId: ch2.id });
    expect(await shape(made)).toEqual([
      ['Chapter 1', ['Unit 1']],
      ['Chapter 2', ['Unit 2']],
    ]);

    // Chapters have no body: unit() returns body null and patchUnit(body) rejects.
    const chDetail = await courses.unit(made.course.id, ch1.id);
    expect(chDetail.body).toBeNull();
    expect(
      (
        await rejection(
          courses.patchUnit(made.course.id, ch1.id, {
            body: text('nope'),
            readAt: chDetail.updatedAt,
          }),
        )
      ).status,
    ).toBe(400);

    // Units do have bodies and enforce readAt conflict checks.
    const detail = await courses.unit(made.course.id, u1.id);
    expect(detail.body).toBeNull();
    const updated = await courses.patchUnit(made.course.id, u1.id, {
      body: text('Hello learners'),
      readAt: detail.updatedAt,
    });
    expect(updated.body).toEqual(text('Hello learners'));
    expect(
      (
        await rejection(
          courses.patchUnit(made.course.id, u1.id, {
            body: text('stale'),
            readAt: detail.updatedAt,
          }),
        )
      ).status,
    ).toBe(409);
  });

  it('caps chapters and units separately as the server does', async () => {
    const made = await aCourse();
    const courses = made.backend.course;
    courses.limits = { depth: 2, chapters: 1, units: 2 };
    const ch = await courses.createUnit(made.course.id, { kind: 'chapter', title: 'Only chapter' });
    const tooManyChapters = await rejection(
      courses.createUnit(made.course.id, { kind: 'chapter', title: 'Second' }),
    );
    expect([tooManyChapters.reason, tooManyChapters.limit]).toEqual(['too_many_chapters', 1]);
    await courses.createUnit(made.course.id, { kind: 'unit', title: 'U1', parentId: ch.id });
    await courses.createUnit(made.course.id, { kind: 'unit', title: 'U2' });
    const tooManyUnits = await rejection(
      courses.createUnit(made.course.id, { kind: 'unit', title: 'U3' }),
    );
    expect([tooManyUnits.reason, tooManyUnits.limit]).toEqual(['too_many_units', 2]);
  });

  it('deletes a chapter with its units, lists them in trash, and moves orphaned restored units to top', async () => {
    const made = await aCourse();
    const courses = made.backend.course;
    const ch = await courses.createUnit(made.course.id, { kind: 'chapter', title: 'Basics' });
    const u1 = await courses.createUnit(made.course.id, {
      kind: 'unit',
      title: 'Welcome',
      parentId: ch.id,
    });
    await courses.createUnit(made.course.id, {
      kind: 'unit',
      title: 'Included',
      parentId: ch.id,
    });
    await courses.createUnit(made.course.id, { kind: 'unit', title: 'Standalone' });

    // Delete u1 first, then delete its chapter (taking Included along).
    await courses.deleteUnit(made.course.id, u1.id);
    await courses.deleteUnit(made.course.id, ch.id);
    expect(await shape(made)).toEqual(['Standalone']);

    const trash = await courses.deletedUnits(made.course.id);
    expect(trash.map((entry) => [entry.title, entry.below])).toEqual([
      ['Basics', 1],
      ['Welcome', 0],
    ]);

    // Restoring u1 while its parent chapter is still deleted places it at top.
    const restoredUnit = await courses.restoreUnit(made.course.id, u1.id);
    expect(restoredUnit.parentId).toBe('');
    expect(await shape(made)).toEqual(['Standalone', 'Welcome']);

    // Restoring the chapter brings it and its child back.
    await courses.restoreUnit(made.course.id, ch.id);
    expect(await shape(made)).toEqual([['Basics', ['Included']], 'Standalone', 'Welcome']);
    expect(await courses.deletedUnits(made.course.id)).toEqual([]);
  });

  it('allows workspace members to read an open course without joining and exports/imports archives', async () => {
    const made = await aCourse(setup(), 'open');
    const ch = await made.backend.course.createUnit(made.course.id, {
      kind: 'chapter',
      title: 'Intro',
    });
    await made.backend.course.createUnit(made.course.id, {
      kind: 'unit',
      title: 'Start',
      parentId: ch.id,
    });

    // Another workspace member can read units of an open course without membership.
    expect((await made.other.course.units(made.course.id)).map((u) => u.title)).toEqual([
      'Intro',
      'Start',
    ]);
    // But cannot write until joining.
    expect(
      (await rejection(made.other.course.createUnit(made.course.id, { title: 'Blocked' }))).status,
    ).toBe(403);

    // Archive export & import roundtrip preserves course kind and 2-level tree.
    const saved = await made.backend.exportContainer(made.course.id);
    const file = new File([saved.bytes], saved.name);
    await made.backend.importArchive(made.workspaceId, file);
    const all = await made.backend.course.courses(made.workspaceId);
    const imported = all.find((c) => c.id !== made.course.id);
    expect(imported).toMatchObject({ kind: 'course', title: 'Onboarding' });
    expect(await shape({ ...made, course: imported ?? made.course })).toEqual([
      ['Intro', ['Start']],
    ]);
  });

  it('supports unit comments, versions, conflict checks, and rejects chapters and unjoined writes', async () => {
    const made = await aCourse(setup(), 'open');
    const courses = made.backend.course;
    const ch = await courses.createUnit(made.course.id, { kind: 'chapter', title: 'Ch 1' });
    const u = await courses.createUnit(made.course.id, {
      kind: 'unit',
      title: 'Lesson 1',
      parentId: ch.id,
    });

    // Chapters refuse comments.
    expect((await rejection(courses.comments(made.course.id, ch.id))).status).toBe(404);
    expect((await rejection(courses.createComment(made.course.id, ch.id, text('hi')))).status).toBe(
      404,
    );

    // Comment CRUD + edit history + stale readAt conflict + undo restore.
    const c1 = await courses.createComment(made.course.id, u.id, text('First note'));
    expect((await courses.comments(made.course.id, u.id)).map((c) => c.id)).toEqual([c1.id]);

    // Unjoined workspace member reads comments (200) but cannot write (403).
    expect((await made.other.course.comments(made.course.id, u.id)).map((c) => c.id)).toEqual([
      c1.id,
    ]);
    expect(
      (await rejection(made.other.course.createComment(made.course.id, u.id, text('blocked'))))
        .status,
    ).toBe(403);

    const edited = await courses.patchComment(made.course.id, u.id, c1.id, {
      body: text('Edited note'),
      readAt: c1.updatedAt,
    });
    expect(edited.editedAt).toBeGreaterThan(0);
    expect(
      (
        await rejection(
          courses.patchComment(made.course.id, u.id, c1.id, {
            body: text('Stale'),
            readAt: c1.updatedAt,
          }),
        )
      ).status,
    ).toBe(409);

    const versions = await courses.commentVersions(made.course.id, u.id, c1.id);
    expect(versions).toHaveLength(1);

    await courses.deleteComment(made.course.id, u.id, c1.id);
    expect(await courses.comments(made.course.id, u.id)).toEqual([]);
    const restored = await courses.restoreComment(made.course.id, u.id, c1.id);
    expect(restored.id).toBe(c1.id);
    expect((await courses.comments(made.course.id, u.id)).map((c) => c.id)).toEqual([c1.id]);
  });

  it('keeps and restores unit body versions and rejects chapters', async () => {
    const made = await aCourse();
    const courses = made.backend.course;
    const ch = await courses.createUnit(made.course.id, { kind: 'chapter', title: 'Ch 1' });
    const u = await courses.createUnit(made.course.id, {
      kind: 'unit',
      title: 'Lesson 1',
      parentId: ch.id,
    });

    expect((await rejection(courses.bodyVersions.list(made.course.id, ch.id))).status).toBe(404);
    expect((await rejection(courses.bodyVersions.keep(made.course.id, ch.id))).status).toBe(404);

    const v1 = await courses.patchUnit(made.course.id, u.id, {
      body: text('Draft one'),
      readAt: u.updatedAt,
    });
    await courses.patchUnit(made.course.id, u.id, {
      body: text('Draft two'),
      readAt: v1.updatedAt,
    });
    const list = await courses.bodyVersions.list(made.course.id, u.id);
    expect(list.length).toBeGreaterThanOrEqual(2);
    const oldest = list.at(-1);
    if (!oldest) throw new Error('expected version');
    const content = await courses.bodyVersions.read(made.course.id, u.id, oldest.version);
    expect(content.body).toEqual(text('Draft one'));

    await courses.bodyVersions.restore(made.course.id, u.id, oldest.version);
    expect((await courses.unit(made.course.id, u.id)).body).toEqual(text('Draft one'));
  });

  it('resolves, searches, and backlinks units while excluding chapters, and allows course sharing', async () => {
    const made = await aCourse(setup(), 'open');
    const courses = made.backend.course;
    const ch = await courses.createUnit(made.course.id, { kind: 'chapter', title: 'Chapter One' });
    const u1 = await courses.createUnit(made.course.id, {
      kind: 'unit',
      title: 'Calculus Intro',
      parentId: ch.id,
    });
    const u2 = await courses.createUnit(made.course.id, {
      kind: 'unit',
      title: 'Limits',
      parentId: ch.id,
    });
    await courses.patchUnit(made.course.id, u1.id, {
      body: {
        type: 'doc',
        content: [
          {
            type: 'paragraph',
            content: [{ type: 'reference', attrs: { id: u2.id } }],
          },
        ],
      },
      readAt: u1.updatedAt,
    });

    const resolved = await made.backend.references.resolve(made.workspaceId, [ch.id, u1.id, u2.id]);
    expect(resolved.map((r) => [r.id, r.kind, r.containerKind])).toEqual([
      [u1.id, 'unit', 'course'],
      [u2.id, 'unit', 'course'],
    ]);
    const searched = await made.backend.references.search(made.workspaceId, 'Calculus');
    expect(searched.map((r) => r.id)).toContain(u1.id);
    expect(searched.some((r) => r.id === ch.id)).toBe(false);
    const backlinks = await made.backend.references.to(made.workspaceId, u2.id);
    expect(backlinks.map((r) => r.id)).toEqual([u1.id]);

    // Watch / unwatch and notifications for shared workspace courses; own-space courses emit none.
    const me = await made.backend.me();
    expect(await courses.watching(made.workspaceId, me.id)).toContain(u1.id);
    expect((await rejection(courses.watchUnit(made.workspaceId, ch.id, me.id))).status).toBe(404);
    await courses.unwatchUnit(made.workspaceId, u1.id, me.id);
    expect(await courses.watching(made.workspaceId, me.id)).not.toContain(u1.id);
    await courses.watchUnit(made.workspaceId, u1.id, me.id);
    expect(await courses.watching(made.workspaceId, me.id)).toContain(u1.id);

    await made.backend.addMember(made.course.id, made.other.userId, 'commenter');
    await made.other.course.createComment(made.course.id, u1.id, text('Question on unit'));
    const notes = await made.backend.notifications.list();
    expect(notes[0]?.subject).toMatchObject({
      kind: 'comment',
      containerId: made.course.id,
      containerKind: 'course',
      blockId: u1.id,
      blockKind: 'unit',
    });

    // Public sharing enabled for workspace course (and diary remains 404).
    const shared = await made.backend.setPublicLink(made.course.id, { enabled: true });
    expect(shared.enabled).toBe(true);
    expect(shared.token).not.toBe('');

    const spaces = await made.backend.workspaces();
    const ownSpace = spaces.find(isOwnSpace);
    if (!ownSpace) throw new Error('expected own space');
    const ownCourse = await courses.createCourse(ownSpace.id, {
      title: 'My Course',
      visibility: 'private',
    });
    const ownUnit = await courses.createUnit(ownCourse.id, { kind: 'unit', title: 'Own Unit' });
    await courses.createComment(ownCourse.id, ownUnit.id, text('Self note'));
    // Server parity (`notification_noting.rs`): personal own-space comments never auto-watch.
    expect(await courses.watching(ownSpace.id, me.id)).not.toContain(ownUnit.id);
    const ownShared = await made.backend.setPublicLink(ownCourse.id, { enabled: true });
    expect(ownShared.enabled).toBe(true);
    const diary = await made.backend.diary.createDiary(ownSpace.id, { title: 'Diary' });
    expect((await rejection(made.backend.publicLink(diary.id))).status).toBe(404);
  });

  it('parses years 0001-0099 accurately in archive parseIsoDateDays', () => {
    const day1 = parseIsoDateDays('0001-01-01');
    const day2 = parseIsoDateDays('0001-01-02');
    expect(day1).toBeDefined();
    expect(day2).toBe((day1 ?? 0) + 1);
    expect(parseIsoDateDays('0050-03-15')).toBeDefined();
    expect(parseIsoDateDays('0004-02-29')).toBeDefined();
    expect(parseIsoDateDays('0050-02-29')).toBeUndefined();
    expect(parseIsoDateDays('0000-01-01')).toBeUndefined();
  });
});
