/**
 * What the memory backend's methods share: the server's limits, the
 * refusal as the server makes it, the shapes its stored blocks are answered
 * in, and an order of lists nobody should lean on.
 */
import { countTasks } from '../../editor/body-text.ts';
import { MAX_BODY_BYTES, type DocNode } from '../../editor/schema.ts';
import {
  createBoardView,
  createCard,
  type Container,
  type ContainerMember,
  type BoardView,
} from '../../logic/wire.ts';
import {
  type Attachment,
  type CardDetail,
  type CardSummary,
  type Change,
  type Comment,
  type WikiPage,
} from '../backend.ts';
import { summaryOfBody } from '../body-summary.ts';

import { withPropertyOrder } from '../../logic/property-order.ts';
import { isAmountUnit } from '../../logic/course-schedule.ts';
import type { CalendarDate } from '../../logic/dates.ts';
import type { CourseUnit } from '../course-types.ts';
import type { DiaryEntry, Mood } from '../diary-types.ts';
import { fail } from './refuse.ts';
import type { StoredBlock, StoredCard } from './world.ts';

/** The server's limits of a body and of a file, as its default configuration has them. */
export const LIMITS = {
  bodyBytes: MAX_BODY_BYTES,
  fileBytes: 100 * 1024 * 1024,
  /** The versions kept of a file, as the server keeps them unless set otherwise. */
  fileVersions: 50,
  /** How long a file's lease lasts unless held on to, as the server has it. */
  fileLeaseSeconds: 90,
  uploadPartBytes: 8 * 1024 * 1024,
  screenStateKeys: 100,
  screenStateValueBytes: 4 * 1024,
} as const;

export const SCREEN_STATE_KEY = /^[A-Za-z0-9_.-]{1,64}$/;

export const copy = <T>(value: T): T => structuredClone(value);

/** The first line of an archive of the memory backend. */
export const MEMORY_ARCHIVE = 'fvoci-memory-1';

/**
 * The board with its properties in `propertyIds`, which names every one of
 * them once, as the server takes it; 400 and nothing changes otherwise.
 */
export function inPropertyOrder(stored: Container, propertyIds: readonly string[]): Container {
  const ids = stored.cardProperties.map((template) => template.id);
  if (new Set(propertyIds).size !== propertyIds.length) {
    fail(400, 'the order names each property of the board once');
  }
  if (propertyIds.some((id) => !ids.includes(id)) || propertyIds.length !== ids.length) {
    fail(400, 'the order names every property of the board');
  }
  return withPropertyOrder(stored, propertyIds);
}

export function summaryOf(card: StoredCard): CardSummary {
  const { body, file: _file, ...block } = card;
  const tasks = body ? countTasks(body) : { total: 0, checked: 0 };
  return copy({
    ...createCard(block),
    hasBody: body !== null && body !== undefined,
    tasksTotal: tasks.total,
    tasksDone: tasks.checked,
  });
}

export function detailOf(card: StoredCard): CardDetail {
  return { ...summaryOf(card), body: copy(card.body ?? null) };
}

export function viewOf(view: StoredBlock): BoardView {
  const { body: _body, file: _file, ...block } = view;
  return copy(createBoardView(block));
}

export function commentOf(block: StoredBlock): Comment {
  const { body, file: _file, ...rest } = block;
  return copy({ ...rest, type: 'comment', body: body ?? null });
}

export function attachmentOf(block: StoredBlock): Attachment {
  const { body: _body, file, ...rest } = block;
  if (!file) throw new Error(`attachment ${block.id} has no file`);
  return copy({ ...rest, type: 'attachment', ...file });
}

/** The place of a page among its siblings, as its `fields` have it. */
export function orderOf(block: StoredBlock): string {
  const order = (block.fields as { order?: unknown }).order;
  return typeof order === 'string' ? order : '';
}

/** An entry of a diary as a change tells it: its day, mood, weather and place are among its `fields`. */
export function entryOf(block: StoredBlock): DiaryEntry {
  const fields = block.fields as {
    date?: CalendarDate;
    mood?: Mood | null;
    weather?: string;
    place?: string;
  };
  return {
    id: block.id,
    diaryId: block.containerId,
    date: fields.date ?? (block.title as CalendarDate),
    mood: fields.mood ?? null,
    weather: fields.weather ?? '',
    place: fields.place ?? '',
    createdAt: block.createdAt,
    updatedAt: block.updatedAt,
  };
}

/** A page as its tree lists it. */
export function pageOf(block: StoredBlock): WikiPage {
  const icon = (block.fields as { icon?: unknown }).icon;
  return {
    id: block.id,
    wikiId: block.containerId,
    parentId: block.parentId,
    title: block.title,
    icon: typeof icon === 'string' ? icon : '',
    order: orderOf(block),
    createdAt: block.createdAt,
    updatedAt: block.updatedAt,
  };
}

/** A unit or chapter of a course as its outline lists it. */
export function unitOf(block: StoredBlock): CourseUnit {
  const kind = block.type === 'chapter' ? 'chapter' : 'unit';
  const fields = block.fields as {
    amount?: unknown;
    amountUnit?: unknown;
    scheduledDate?: unknown;
  };
  return {
    id: block.id,
    courseId: block.containerId,
    kind,
    parentId: block.parentId,
    title: block.title,
    order: orderOf(block),
    amount: kind === 'unit' && typeof fields.amount === 'number' ? fields.amount : null,
    amountUnit: kind === 'unit' && isAmountUnit(fields.amountUnit) ? fields.amountUnit : null,
    scheduledDate:
      kind === 'unit' && typeof fields.scheduledDate === 'string' ? fields.scheduledDate : null,
    createdAt: block.createdAt,
    updatedAt: block.updatedAt,
  };
}

/** What the listeners of a container are told of a block made or changed. */
export function changeOf(containerId: string, block: StoredBlock): Change {
  if (block.type === 'page') return { kind: 'pages', containerId, pages: [pageOf(block)] };
  if (block.type === 'entry') return { kind: 'entries', containerId, entries: [entryOf(block)] };
  if (block.type === 'unit' || block.type === 'chapter') {
    return { kind: 'units', containerId, units: [unitOf(block)] };
  }
  return { kind: 'blocks', containerId, blocks: [wireOf(block)] };
}

export function wireOf(block: StoredBlock): BoardView | CardSummary | Comment | Attachment {
  switch (block.type) {
    case 'view':
      return viewOf(block);
    case 'card': {
      // A change message carries the summary of the body in place of it, as on the server.
      const { body, file: _file, ...rest } = block;
      return copy({ ...createCard(rest), ...summaryOfBody(body) });
    }
    case 'comment':
      return commentOf(block);
    default:
      return attachmentOf(block);
  }
}

/** A list in an order nobody should rely on: by a hash of the identifiers. */
export function scramble<T extends { id: string } | { userId: string }>(items: readonly T[]): T[] {
  const key = (item: T): number => {
    const text = 'id' in item ? item.id : item.userId;
    let hash = 0;
    for (let at = 0; at < text.length; at += 1) hash = (hash * 31 + text.charCodeAt(at)) % 9973;
    return hash;
  };
  return [...items].sort((first, second) => key(first) - key(second));
}

/** Whether a node names a file: an image, or a file block. */
export const namesFile = (node: DocNode): boolean =>
  (node.type === 'image' || node.type === 'file') && typeof node.attrs?.fileId === 'string';

/** The files a body names, by its images and file blocks. */
export function bodyFileIds(node: DocNode, found: string[] = []): string[] {
  if (namesFile(node)) found.push(String(node.attrs?.fileId));
  for (const child of node.content ?? []) bodyFileIds(child, found);
  return found;
}

/** Whether a body has a file block, which a comment's may not. */
export function hasFileBlock(node: DocNode): boolean {
  return node.type === 'file' || (node.content ?? []).some(hasFileBlock);
}

export function memberOf(
  containerId: string,
  userId: string,
  role: 'admin' | 'editor' | 'commenter' | 'viewer',
): ContainerMember {
  return {
    containerId,
    userId,
    role,
    synthetic: false,
  };
}
