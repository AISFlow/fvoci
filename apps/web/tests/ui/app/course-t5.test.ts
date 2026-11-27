/**
 * Unit tests for course-t5 app wiring: unit reference places, unit notification
 * routes, and the public course reader (`SharedCourse.svelte`).
 */
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { afterEach, describe, expect, it, vi } from 'vitest';

import { routeOfPlace } from '../../../src/app/places.ts';
import { referencePlaces } from '../../../src/app/reference-places.ts';
import { Router } from '../../../src/app/router.svelte.ts';
import SharedTreeReader from '../../../src/app/SharedTreeReader.svelte';
import type { Backend, CourseUnit, CourseUnitDetail } from '$lib/state/index.ts';

afterEach(cleanup);

const WS = '01930000-0000-7000-8000-0000000000a1';
const COURSE = '01930000-0000-7000-8000-0000000000c1';
const CHAPTER = '01930000-0000-7000-8000-0000000000c2';
const U1 = '01930000-0000-7000-8000-0000000000c3';
const U2 = '01930000-0000-7000-8000-0000000000c4';
const OUTSIDE = '01930000-0000-7000-8000-0000000000f9';

describe('unit reference places and notification links', () => {
  it('resolves unit references and notification places to /w/<ws>/courses/<course>/<unit>', () => {
    history.replaceState(null, '', `/w/${WS}/courses/${COURSE}/${U1}`);
    const router = new Router('');
    const places = referencePlaces(router);
    const linked = {
      id: U2,
      kind: 'unit' as const,
      title: 'Lesson 2',
      icon: '',
      containerId: COURSE,
      containerKind: 'course' as const,
    };
    expect(places.href(linked)).toBe(`/w/${WS}/courses/${COURSE}/${U2}`);
    places.open(linked);
    expect(router.route).toMatchObject({ kind: 'course', courseId: COURSE, unitId: U2 });

    const notified = routeOfPlace(router, {
      kind: 'unit',
      id: U1,
      containerId: COURSE,
      workspaceId: WS,
    });
    expect(notified).toEqual({ workspaceId: WS, courseId: COURSE, unitId: U1 });
  });
});

describe('SharedCourse public reader', () => {
  it('renders chapters as headings, units as links, same-course references clickable, and strips comments/editing', async () => {
    history.replaceState(null, '', `/shared/${COURSE}?r=tok123`);
    const router = new Router('');
    const chapter: CourseUnit = {
      id: CHAPTER,
      courseId: COURSE,
      kind: 'chapter',
      parentId: '',
      title: 'Chapter 1',
      order: 'a',
      createdAt: 1,
      updatedAt: 1,
    };
    const unit1: CourseUnit = {
      id: U1,
      courseId: COURSE,
      kind: 'unit',
      parentId: CHAPTER,
      title: 'Lesson 1',
      order: 'a',
      createdAt: 2,
      updatedAt: 2,
    };
    const unit2: CourseUnit = {
      id: U2,
      courseId: COURSE,
      kind: 'unit',
      parentId: CHAPTER,
      title: 'Lesson 2',
      order: 'b',
      createdAt: 3,
      updatedAt: 3,
    };
    const units: CourseUnit[] = [chapter, unit1, unit2];
    const details: Record<string, CourseUnitDetail> = {
      [U1]: {
        ...unit1,
        body: {
          type: 'doc',
          content: [
            {
              type: 'paragraph',
              content: [
                { type: 'text', text: 'Next up: ' },
                { type: 'reference', attrs: { id: U2 } },
                { type: 'text', text: ' and hidden: ' },
                { type: 'reference', attrs: { id: OUTSIDE } },
              ],
            },
          ],
        },
        createdBy: '',
        modifiedBy: '',
      },
      [U2]: {
        ...unit2,
        body: {
          type: 'doc',
          content: [{ type: 'paragraph', content: [{ type: 'text', text: 'Second body' }] }],
        },
        createdBy: '',
        modifiedBy: '',
      },
    };
    const setPresence = vi.fn();
    const myProgress = vi.fn();
    const putUnitProgress = vi.fn();
    const progressSummary = vi.fn();
    const schedule = vi.fn();
    const putUnitSchedule = vi.fn();
    const backend = {
      presence: { setPresence, watchPresence: () => () => undefined },
      course: {
        units: vi.fn().mockResolvedValue(units),
        unit: vi.fn().mockImplementation((_c: string, id: string) => Promise.resolve(details[id])),
        myProgress,
        putUnitProgress,
        progressSummary,
        schedule,
        putUnitSchedule,
      },
    } as unknown as Backend;

    render(SharedTreeReader, { kind: 'course', backend, containerId: COURSE, router });

    await waitFor(() => {
      expect(screen.getByTestId('unit-title')).toHaveTextContent('Lesson 1');
    });
    const tree = screen.getByTestId('shared-unit-tree');
    expect(tree.querySelector('[data-kind="chapter"]')).toHaveTextContent('Chapter 1');
    expect(tree.querySelectorAll('a')).toHaveLength(2);
    // No comment composer, no versions menu, no watch toggle, no editable textbox, no progress or schedule UI/calls.
    expect(screen.queryByTestId('comment-composer')).toBeNull();
    expect(screen.queryByTestId('unit-menu')).toBeNull();
    expect(screen.queryByTestId('unit-watch-toggle')).toBeNull();
    expect(screen.queryByTestId('course-my-progress')).toBeNull();
    expect(screen.queryByTestId('course-schedule-panel')).toBeNull();
    expect(screen.queryByTestId('unit-schedule-controls')).toBeNull();
    expect(screen.queryByTestId('unit-complete-toggle')).toBeNull();
    expect(screen.queryByTestId('unit-last-position')).toBeNull();
    expect(screen.queryByTestId('unit-tree-progress')).toBeNull();
    expect(screen.queryByRole('textbox')).toBeNull();
    expect(myProgress).not.toHaveBeenCalled();
    expect(putUnitProgress).not.toHaveBeenCalled();
    expect(progressSummary).not.toHaveBeenCalled();
    expect(schedule).not.toHaveBeenCalled();
    expect(putUnitSchedule).not.toHaveBeenCalled();

    // Same-course reference is a link; outside reference is unseen (no link).
    const body = screen.getByTestId('unit-body');
    const sameRef = body.querySelector(`[data-block-id="${U2}"] a`);
    if (!sameRef) throw new Error('expected same-course reference link');
    expect(sameRef).toHaveTextContent('Lesson 2');
    const externalRef = body.querySelector(`[data-block-id="${OUTSIDE}"]`);
    expect(externalRef?.getAttribute('data-state')).toBe('unseen');
    expect(externalRef?.querySelector('a')).toBeNull();

    // Clicking the same-course reference opens Lesson 2.
    await fireEvent.click(sameRef);
    await waitFor(() => {
      expect(screen.getByTestId('unit-title')).toHaveTextContent('Lesson 2');
    });
    expect(screen.getByTestId('unit-body')).toHaveTextContent('Second body');
  });
});
