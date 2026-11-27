/** Seeds the sample course and built-in course template for in-memory worlds. */
import type { DocNode } from '../../editor/schema.ts';
import { createBoard, type Container } from '../../logic/wire.ts';
import type { Category, User } from '../backend.ts';

import { memberOf } from './backend.ts';
import type { World } from './world.ts';

const paragraph = (text: string): DocNode => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
});

/** Seeds one sample course with chapters and units into the default category, plus the built-in course template. */
export function seedCourse(
  world: World,
  reader: User,
  users: readonly User[],
  categories: Category[],
  withBuiltinTemplate = true,
): void {
  const courseId = world.newId();
  const at = world.now();
  const course: Container = {
    ...createBoard(),
    id: courseId,
    workspaceId: world.workspace.id,
    kind: 'course',
    createdBy: reader.id,
    modifiedBy: reader.id,
    visibility: 'open',
    minimumRole: 'editor',
    title: 'Workspace Onboarding',
    description: 'A guided course for getting started with the workspace.',
    icon: '',
    cardProperties: [],
    createdAt: at,
    updatedAt: at,
  };
  world.containers.set(courseId, course);
  world.members.set(
    courseId,
    users
      .slice(0, 3)
      .map((user, index) => memberOf(courseId, user.id, index === 0 ? 'admin' : 'editor')),
  );
  const addNode = (
    type: 'chapter' | 'unit',
    parentId: string,
    title: string,
    order: string,
    body?: string,
  ): string => {
    const id = world.newId();
    const made = world.now();
    world.blocks.set(id, {
      id,
      containerId: courseId,
      parentId,
      createdBy: reader.id,
      modifiedBy: reader.id,
      type,
      title,
      fields: { order },
      body: type === 'unit' && body ? paragraph(body) : null,
      createdAt: made,
      updatedAt: made,
    });
    return id;
  };
  const ch1 = addNode('chapter', '', 'Getting started', 'V');
  addNode(
    'unit',
    ch1,
    'Workspace overview',
    'V',
    'Welcome to the workspace. Explore boards, wikis, and courses here.',
  );
  addNode(
    'unit',
    ch1,
    'Organizing your work',
    'W',
    'Group items into categories and keep outlines tidy.',
  );
  const ch2 = addNode('chapter', '', 'Working together', 'W');
  addNode(
    'unit',
    ch2,
    'Collaborating in real time',
    'V',
    'Edit unit bodies together and see live presence.',
  );
  categories.find((category) => category.type === 'system')?.containerIds.push(courseId);
  if (withBuiltinTemplate) seedBuiltinCourseTemplate(world, reader);
}

const textNode = (text: string): DocNode => ({ type: 'text', text });
const paraNode = (text: string): DocNode => ({ type: 'paragraph', content: [textNode(text)] });
const headingNode = (text: string): DocNode => ({
  type: 'heading',
  attrs: { level: 2 },
  content: [textNode(text)],
});

function kickoffBody(): DocNode {
  return {
    type: 'doc',
    content: [
      headingNode('Kickoff Checklist'),
      {
        type: 'taskList',
        content: [
          {
            type: 'taskItem',
            attrs: { checked: false },
            content: [paraNode('Write down three concrete outcomes for this course')],
          },
        ],
      },
    ],
  };
}

function conceptsTableBody(): DocNode {
  const cell = (type: 'tableHeader' | 'tableCell', text: string): DocNode => ({
    type,
    content: [paraNode(text)],
  });
  return {
    type: 'doc',
    content: [
      headingNode('Concept Comparison Table'),
      {
        type: 'table',
        content: [
          {
            type: 'tableRow',
            content: [cell('tableHeader', 'Concept'), cell('tableHeader', 'Key Summary')],
          },
          {
            type: 'tableRow',
            content: [
              cell('tableCell', 'Core Principle'),
              cell('tableCell', 'State the definition and conditions in one sentence'),
            ],
          },
        ],
      },
    ],
  };
}

function walkthroughBody(): DocNode {
  return {
    type: 'doc',
    content: [
      headingNode('Solution Walkthrough Notes'),
      {
        type: 'bulletList',
        content: [
          {
            type: 'listItem',
            content: [paraNode('Separate the given constraints from the target quantity first.')],
          },
        ],
      },
    ],
  };
}

function practiceBody(conceptsUnitId: string): DocNode {
  return {
    type: 'doc',
    content: [
      {
        type: 'paragraph',
        content: [
          textNode('When a concept feels fuzzy, revisit '),
          { type: 'reference', attrs: { id: conceptsUnitId } },
          textNode(' and track any missed items below.'),
        ],
      },
    ],
  };
}

/** Seeds the built-in course study-plan template the window of new things offers. */
function seedBuiltinCourseTemplate(world: World, reader: User): void {
  const templateId = world.newId();
  const at = world.now();
  const template: Container = {
    ...createBoard(),
    id: templateId,
    workspaceId: world.workspace.id,
    kind: 'course',
    createdBy: reader.id,
    modifiedBy: reader.id,
    visibility: 'open',
    minimumRole: 'editor',
    isTemplate: true,
    isBuiltin: true,
    title: 'Two-Week Study Plan',
    description:
      'A built-in course template structured over two weeks, from core concepts and worked examples to practice problems and final review.',
    icon: '🎓',
    cardProperties: [],
    properties: {
      templateKey: 'course-study-plan',
      templateLanguage: 'en',
      templateBaseDate: '2026-12-01',
    },
    createdAt: at,
    updatedAt: at,
  };
  world.containers.set(templateId, template);
  world.courseSchedules.set(templateId, {
    startDate: '2026-12-01',
    targetDate: '2026-12-14',
    studyDays: 31,
    updatedAt: at,
  });
  const addNode = (
    type: 'chapter' | 'unit',
    parentId: string,
    title: string,
    fields: Record<string, unknown>,
    body?: DocNode,
  ): string => {
    const id = world.newId();
    const made = world.now();
    world.blocks.set(id, {
      id,
      containerId: templateId,
      parentId,
      createdBy: reader.id,
      modifiedBy: reader.id,
      type,
      title,
      fields,
      body: type === 'unit' ? (body ?? null) : null,
      createdAt: made,
      updatedAt: made,
    });
    return id;
  };
  const ch1 = addNode('chapter', '', 'Chapter 1. Setup and Core Concepts', { order: 'V' });
  addNode(
    'unit',
    ch1,
    'Define Goals and Scope',
    { order: 'V', amount: 30, amountUnit: 'minutes', scheduledDate: '2026-12-01' },
    kickoffBody(),
  );
  const u2 = addNode(
    'unit',
    ch1,
    'Summarize Core Concepts',
    { order: 'W', amount: 20, amountUnit: 'pages', scheduledDate: '2026-12-03' },
    conceptsTableBody(),
  );
  addNode(
    'unit',
    ch1,
    'Walk Through Worked Examples',
    { order: 'X', amount: 45, amountUnit: 'minutes', scheduledDate: '2026-12-07' },
    walkthroughBody(),
  );
  const ch2 = addNode('chapter', '', 'Chapter 2. Practice and Review', { order: 'W' });
  addNode(
    'unit',
    ch2,
    'Solve Practice Problems',
    { order: 'V', amount: 15, amountUnit: 'problems', scheduledDate: '2026-12-10' },
    practiceBody(u2),
  );
  addNode(
    'unit',
    ch2,
    'Review Mistakes and Final Check',
    { order: 'W', amount: 30, amountUnit: 'minutes', scheduledDate: '2026-12-14' },
    paragraph('Retry missed problems one day later and summarize the hardest concept.'),
  );
}
