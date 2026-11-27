/**
 * Built-in template helpers for the in-memory world: seeding boards and
 * courses from template files (with internal references and asset files
 * placed), and resolving the reader's template language as the server does
 * (`crates/server/src/app/templates.rs`, `language.rs`).
 */
import type { DocNode } from '../../editor/schema.ts';
import {
  createBoard,
  parseRelationValue,
  type Container,
  type PropertyTemplate,
  type PropertyType,
  type PropertyValue,
} from '../../logic/wire.ts';
import type { User } from '../backend.ts';

import { settingsOf } from './account.ts';
import { renameNamed } from './linked.ts';
import { namesFile } from './shapes.ts';
import type { World } from './world.ts';

/** The system workspace that owns built-in templates, matching `SYSTEM_WORKSPACE_ID`. */
const SYSTEM_WORKSPACE_ID = '0';

/** A file named in a built-in template file. */
export interface SeedAsset {
  asset: string;
  name: string;
}

/** Legacy file descriptor kept for synthetic test fixtures. */
export interface LegacySeedFile {
  fileId: string;
  archiveMember: string;
  bytes: number;
}

export type SeedFile = SeedAsset | LegacySeedFile;

export interface SeedCourseSchedule {
  startDate: string;
  targetDate: string;
  studyDays: number;
}

export interface SeedCourseChapter {
  id: string;
  type?: 'chapter';
  title: string;
  fields: { order: string };
}

export interface SeedCourseUnit {
  id: string;
  type?: 'unit';
  parentId: string;
  title: string;
  fields: {
    order: string;
    amount?: number;
    amountUnit?: 'minutes' | 'pages' | 'problems';
    scheduledDate?: string;
  };
  body: DocNode | null;
}

/** One board or course of `./seed/`, in the shape it is written in. */
export interface SeedTemplate {
  key: string;
  language?: string;
  baseDate?: string;
  onboarding: boolean;
  board: {
    id: string;
    kind?: 'board' | 'course';
    title: string;
    description: string;
    icon: string;
    showDescription: boolean;
    cardProperties: {
      id: string;
      name: string;
      type: PropertyType;
      options: PropertyTemplate['options'];
    }[];
  };
  schedule?: SeedCourseSchedule;
  views: { id: string; title: string; fields: Record<string, unknown> }[];
  cards: {
    id: string;
    title: string;
    fields: { icon: string; isTemplate: boolean; properties: Record<string, PropertyValue> };
    body: DocNode | null;
  }[];
  chapters?: SeedCourseChapter[];
  units?: SeedCourseUnit[];
  files: SeedFile[];
}

export type BuiltinLanguage = 'en' | 'ko';

/** The language tag among the two the server writes its built-in templates in. */
export function writtenIn(tag: unknown): BuiltinLanguage | undefined {
  if (typeof tag !== 'string') return undefined;
  const primary = tag.split('-')[0]?.toLowerCase();
  return primary === 'ko' || primary === 'en' ? primary : undefined;
}

/** The language of built-in templates for `userId`: account setting, then browser, then English. */
export function readerLanguage(world: World, userId: string): BuiltinLanguage {
  const setting = settingsOf(world, userId).language;
  const asked = typeof navigator === 'undefined' ? [] : navigator.languages;
  return writtenIn(setting) ?? asked.map(writtenIn).find(Boolean) ?? 'en';
}

/** Whether a built-in template carries a truthy onboarding mark (string `'1'`/`'true'` or boolean `true`). */
function isOnboardingMark(value: unknown): boolean {
  return value === true || (typeof value === 'string' && value !== '' && value !== 'false');
}

/** Workspace templates, then built-in templates in the reader's language, each oldest first. */
export function visibleTemplates(world: World, userId: string, workspaceId: string): Container[] {
  const language = readerLanguage(world, userId);
  const all = [...world.containers.values()]
    .filter(
      (board) =>
        board.isTemplate &&
        board.properties.templateKey !== 'welcome-personal' &&
        (board.isBuiltin === true
          ? board.properties.templateLanguage === undefined ||
            writtenIn(board.properties.templateLanguage) === language
          : board.workspaceId === workspaceId),
    )
    .sort((first, second) => first.createdAt - second.createdAt);
  return [
    ...all.filter((board) => board.isBuiltin !== true),
    ...all.filter((board) => board.isBuiltin === true),
  ];
}

/** The built-in onboarding template for `userId`'s language, if the world holds one. */
export function welcomeTemplate(world: World, userId: string): Container | undefined {
  const language = readerLanguage(world, userId);
  const candidates = [...world.containers.values()]
    .filter(
      (board) =>
        board.isTemplate &&
        board.isBuiltin === true &&
        isOnboardingMark(board.properties.onboarding),
    )
    .sort((first, second) => first.createdAt - second.createdAt);
  return (
    candidates.find((board) => writtenIn(board.properties.templateLanguage) === language) ??
    candidates.find((board) => board.properties.templateLanguage === undefined)
  );
}

const paragraph = (text: string): DocNode => ({
  type: 'doc',
  content: [{ type: 'paragraph', content: [{ type: 'text', text }] }],
});

const COMMENTS = [
  'I can take this one.',
  '다음 주 화요일까지 초안을 올릴게요.',
  'Is this still blocked on the review?',
];

function contentTypeOf(name: string): string {
  if (name.endsWith('.png')) return 'image/png';
  if (name.endsWith('.docx')) {
    return 'application/vnd.openxmlformats-officedocument.wordprocessingml.document';
  }
  return 'application/octet-stream';
}

function renameBodyFiles(node: DocNode, rename: (id: string) => string): DocNode {
  const attrs = namesFile(node)
    ? { attrs: { ...node.attrs, fileId: rename(String(node.attrs?.fileId)) } }
    : {};
  const content = node.content
    ? { content: node.content.map((each) => renameBodyFiles(each, rename)) }
    : {};
  return { ...node, ...attrs, ...content };
}

function remapBody(
  body: DocNode | null,
  blockIdOf: ReadonlyMap<string, string>,
  assetIdOf: ReadonlyMap<string, string>,
): DocNode | null {
  if (!body) return null;
  const withRefs = renameNamed(body, (id) => blockIdOf.get(id) ?? id);
  return assetIdOf.size > 0 ? renameBodyFiles(withRefs, (id) => assetIdOf.get(id) ?? id) : withRefs;
}

/** Registers the files a template names in `world.files` and maps asset names to file ids. */
export function placeSeedFiles(
  world: World,
  containerId: string,
  files: readonly SeedFile[],
): Map<string, string> {
  const assetIdOf = new Map<string, string>();
  for (const file of files) {
    if ('asset' in file) {
      const fileId = world.newId();
      assetIdOf.set(file.asset, fileId);
      world.files.set(fileId, {
        fileId,
        containerId,
        name: file.name,
        size: 1024,
        contentType: contentTypeOf(file.asset),
      });
    } else {
      world.files.set(file.fileId, {
        fileId: file.fileId,
        containerId,
        name: file.archiveMember.split('/').pop() ?? file.fileId,
        size: file.bytes,
        contentType: 'image/png',
      });
    }
  }
  return assetIdOf;
}

/** Converts template card property definitions into wire `PropertyTemplate`s. */
export function boardCardProperties(
  cardProperties: SeedTemplate['board']['cardProperties'],
): PropertyTemplate[] {
  return cardProperties.map((property) => ({
    id: property.id,
    name: property.name,
    type: property.type,
    options: property.options.map((option) => ({ ...option })),
  }));
}

function seedCardComments(
  world: World,
  boardId: string,
  cardId: string,
  people: readonly User[],
  reader: User,
  cardIndex: number,
  templateIndex: number,
): void {
  const comments = cardIndex === 0 ? 2 : cardIndex === 1 ? 1 : 0;
  for (let index = 0; index < comments; index += 1) {
    const commenter = people[(cardIndex + index + 1) % people.length] ?? reader;
    const written = world.now();
    const commentId = world.newId();
    world.blocks.set(commentId, {
      id: commentId,
      containerId: boardId,
      parentId: cardId,
      createdBy: commenter.id,
      modifiedBy: commenter.id,
      type: 'comment',
      title: '',
      fields: {},
      createdAt: written,
      updatedAt: written,
      body: paragraph(COMMENTS[(templateIndex + index) % COMMENTS.length] ?? ''),
    });
  }
}

function seedBoardViews(
  world: World,
  boardId: string,
  views: SeedTemplate['views'],
  cardIdOf: ReadonlyMap<string, string>,
  reader: User,
): void {
  for (const view of views) {
    const made = world.now();
    const fields = structuredClone(view.fields);
    if (Array.isArray(fields.cardOrder)) {
      fields.cardOrder = (fields.cardOrder as string[]).map((id) => cardIdOf.get(id) ?? id);
    }
    if (typeof fields.defaultTemplateId === 'string' && fields.defaultTemplateId !== '') {
      fields.defaultTemplateId = cardIdOf.get(fields.defaultTemplateId) ?? '';
    }
    const id = world.newId();
    world.blocks.set(id, {
      id,
      containerId: boardId,
      parentId: '',
      createdBy: reader.id,
      modifiedBy: reader.id,
      type: 'view',
      title: view.title,
      fields,
      createdAt: made,
      updatedAt: made,
    });
  }
}

/** Seeds cards, views, and (for live boards) person assignments and sample comments. */
export function seedBoardBlocks(
  world: World,
  board: Container,
  template: SeedTemplate,
  assetIdOf: ReadonlyMap<string, string>,
  options: {
    reader: User;
    people: readonly User[];
    live: boolean;
    templateIndex: number;
  },
): void {
  const { reader, people, live, templateIndex } = options;
  const cardIdOf = new Map<string, string>();
  for (const card of template.cards) cardIdOf.set(card.id, world.newId());

  template.cards.forEach((card, cardIndex) => {
    const id = cardIdOf.get(card.id) ?? world.newId();
    const made = world.now();
    const author = live ? (people[cardIndex % people.length] ?? reader) : reader;
    const properties: Record<string, PropertyValue> = structuredClone(card.fields.properties);
    board.cardProperties.forEach((property, propertyIndex) => {
      if (property.type === 'relation') {
        const value = properties[property.id];
        if (value !== undefined) {
          properties[property.id] = parseRelationValue(value).map((link) => ({
            ...link,
            card: cardIdOf.get(link.card) ?? link.card,
          }));
        }
      }
      if (!live) return;
      const person = people[(cardIndex + propertyIndex + 1) % people.length] ?? reader;
      if (property.type === 'person') properties[property.id] = person.id;
      if (property.type === 'multiPerson') properties[property.id] = [person.id, author.id];
    });
    world.blocks.set(id, {
      id,
      containerId: board.id,
      parentId: '',
      createdBy: author.id,
      modifiedBy: author.id,
      type: 'card',
      title: card.title,
      fields: { icon: card.fields.icon, isTemplate: card.fields.isTemplate, properties },
      createdAt: made,
      updatedAt: made,
      body: remapBody(card.body, cardIdOf, assetIdOf),
    });
    if (live && !card.fields.isTemplate) {
      seedCardComments(world, board.id, id, people, reader, cardIndex, templateIndex);
    }
  });

  seedBoardViews(world, board.id, template.views, cardIdOf, reader);
}

/** Seeds one built-in template (board or course) into the system workspace. */
export function seedBuiltinTemplate(world: World, template: SeedTemplate, reader: User): void {
  const id = world.newId();
  const at = world.now();
  const kind = template.board.kind ?? 'board';
  const properties: Record<string, string | string[]> = { templateKey: template.key };
  if (template.language !== undefined) properties.templateLanguage = template.language;
  if (template.baseDate !== undefined) properties.templateBaseDate = template.baseDate;
  if (template.onboarding) properties.onboarding = 'true';

  const container: Container = {
    ...createBoard(),
    id,
    workspaceId: SYSTEM_WORKSPACE_ID,
    kind,
    createdBy: reader.id,
    modifiedBy: reader.id,
    visibility: 'open',
    minimumRole: 'editor',
    isTemplate: true,
    isBuiltin: true,
    title: template.board.title,
    description: template.board.description,
    icon: template.board.icon,
    showDescription: template.board.showDescription,
    cardProperties: boardCardProperties(template.board.cardProperties),
    properties,
    createdAt: at,
    updatedAt: at,
  };
  world.containers.set(id, container);
  const assetIdOf = placeSeedFiles(world, id, template.files);

  if (kind === 'course') {
    if (template.schedule) {
      world.courseSchedules.set(id, { ...template.schedule, updatedAt: at });
    }
    seedCourseTemplateBlocks(world, id, template, reader, assetIdOf);
    return;
  }

  seedBoardBlocks(world, container, template, assetIdOf, {
    reader,
    people: [reader],
    live: false,
    templateIndex: 0,
  });
}

function seedCourseTemplateBlocks(
  world: World,
  courseId: string,
  template: SeedTemplate,
  reader: User,
  assetIdOf: ReadonlyMap<string, string>,
): void {
  const nodeIdOf = new Map<string, string>();
  const chapters = template.chapters ?? [];
  const units = template.units ?? [];
  for (const chapter of chapters) nodeIdOf.set(chapter.id, world.newId());
  for (const unit of units) nodeIdOf.set(unit.id, world.newId());

  for (const chapter of chapters) {
    const id = nodeIdOf.get(chapter.id) ?? world.newId();
    const made = world.now();
    world.blocks.set(id, {
      id,
      containerId: courseId,
      parentId: '',
      createdBy: reader.id,
      modifiedBy: reader.id,
      type: 'chapter',
      title: chapter.title,
      fields: structuredClone(chapter.fields),
      body: null,
      createdAt: made,
      updatedAt: made,
    });
  }
  for (const unit of units) {
    const id = nodeIdOf.get(unit.id) ?? world.newId();
    const made = world.now();
    world.blocks.set(id, {
      id,
      containerId: courseId,
      parentId: unit.parentId === '' ? '' : (nodeIdOf.get(unit.parentId) ?? unit.parentId),
      createdBy: reader.id,
      modifiedBy: reader.id,
      type: 'unit',
      title: unit.title,
      fields: structuredClone(unit.fields),
      body: remapBody(unit.body, nodeIdOf, assetIdOf),
      createdAt: made,
      updatedAt: made,
    });
  }
}
