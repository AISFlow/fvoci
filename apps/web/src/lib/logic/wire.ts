/**
 * The shapes the server stores and sends, and everything that depends on them.
 *
 * Only the `wire*` modules (`wire.ts`, `wire-blocks.ts`, `wire-patches.ts`)
 * know what is inside the untyped `fields` of a block, how dates are written,
 * and how patches are constructed (`tests/logic/wire-boundary.test.ts`).
 */
import type { MemberRole } from './container-members.ts';
export type { ContainerMember, MemberRole } from './container-members.ts';
import {
  dateOfInstant,
  parseCalendarDate,
  type CalendarDate,
  type DatePoint,
  type DateValue,
} from './dates.ts';
export { encodeBase32 } from './base32.ts';
import { type PropertyColor } from './property-colors.ts';
export { DEFAULT_PROPERTY_COLOR, PROPERTY_COLORS, type PropertyColor } from './property-colors.ts';
export * from './relations.ts';
import { createId, type PropertyValue } from './wire-blocks.ts';
export * from './wire-blocks.ts';
export * from './wire-patches.ts';

// ----------------------------------------------------------------- boards

export const CONTAINER_VISIBILITY_OPEN = 'open';
export const CONTAINER_VISIBILITY_PRIVATE = 'private';
export type ContainerVisibility =
  typeof CONTAINER_VISIBILITY_OPEN | typeof CONTAINER_VISIBILITY_PRIVATE;
export type ContainerKind = 'board' | 'wiki' | 'diary' | 'course';

export type PropertyType =
  | 'text'
  | 'number'
  | 'select'
  | 'multiSelect'
  | 'date'
  | 'person'
  | 'multiPerson'
  | 'file'
  | 'checkbox'
  | 'url'
  | 'email'
  | 'phone'
  | 'createdTime'
  | 'createdBy'
  | 'updatedTime'
  | 'updatedBy'
  | 'relation';

export interface PropertyOption {
  id: string;
  label: string;
  color: PropertyColor;
}

export interface PropertyTemplate {
  id: string;
  name: string;
  type: PropertyType;
  options: PropertyOption[];
}

export type DoneRule =
  | { kind: 'option'; propertyId: string; optionIds: string[] }
  | { kind: 'checkbox'; propertyId: string };

export interface Container {
  id: string;
  workspaceId: string;
  channelId?: string;
  createdBy: string;
  modifiedBy: string;
  visibility: ContainerVisibility;
  kind?: ContainerKind;
  minimumRole: MemberRole;
  title: string;
  description: string;
  icon?: string;
  showDescription: boolean;
  isTemplate: boolean;
  isBuiltin?: boolean;
  templateVersion: number;
  properties: Record<string, string | string[]>;
  cardProperties: PropertyTemplate[];
  dueDatePropertyId?: string | null;
  doneWhen?: DoneRule | null;
  createdAt: number;
  updatedAt: number;
  deletedAt?: number;
}

export interface ContainerPatch {
  visibility?: ContainerVisibility;
  minimumRole?: MemberRole;
  title?: string;
  description?: string;
  icon?: string;
  showDescription?: boolean;
  updatedProperties?: Record<string, unknown>;
  deletedProperties?: string[];
  updatedCardProperties?: PropertyTemplate[];
  deletedCardProperties?: string[];
  dueDatePropertyId?: string | null;
  doneWhen?: DoneRule | null;
}

export function createBoard(source?: Container, statusName = 'Status'): Container {
  const now = Date.now();
  const cardProperties: PropertyTemplate[] = source?.cardProperties
    ? source.cardProperties.map((template) => ({
        id: template.id,
        name: template.name,
        type: template.type,
        options: (template.options as PropertyOption[] | undefined)?.map((o) => ({ ...o })) ?? [],
      }))
    : [{ id: createId('block'), name: statusName, type: 'select', options: [] }];

  return {
    id: source?.id || createId('board'),
    workspaceId: source?.workspaceId || '',
    channelId: source?.channelId || '',
    createdBy: source?.createdBy || '',
    modifiedBy: source?.modifiedBy || '',
    visibility: source?.visibility || CONTAINER_VISIBILITY_PRIVATE,
    minimumRole: source?.minimumRole || '',
    title: source?.title || '',
    description: source?.description || '',
    icon: source?.icon || '',
    showDescription: source?.showDescription || false,
    isTemplate: source?.isTemplate || false,
    templateVersion: source?.templateVersion || 0,
    properties: source?.properties ?? {},
    cardProperties,
    ...(source?.dueDatePropertyId === undefined
      ? {}
      : { dueDatePropertyId: source.dueDatePropertyId }),
    ...(source?.doneWhen === undefined ? {} : { doneWhen: source.doneWhen }),
    createdAt: source?.createdAt || now,
    updatedAt: source?.updatedAt || now,
    ...(source?.deletedAt === undefined ? {} : { deletedAt: source.deletedAt }),
  };
}

// ------------------------------------------------------------ date values

export interface StoredDateValue {
  from: string | number;
  to?: string | number;
}

const LAST_INSTANT = Date.UTC(9999, 11, 31, 23, 59, 59, 999);

function readPoint(stored: unknown): DatePoint | undefined {
  if (typeof stored === 'string') {
    const date = parseCalendarDate(stored);
    return date === undefined ? undefined : { date };
  }
  if (typeof stored !== 'number' || !Number.isInteger(stored)) return undefined;
  return stored >= 0 && stored <= LAST_INSTANT ? { instant: stored } : undefined;
}

const writePoint = (point: DatePoint): string | number => point.instant ?? point.date;

const samePoint = (first: DatePoint, second: DatePoint): boolean =>
  first.date === second.date && first.instant === second.instant;

export function parseDateValue(stored: unknown): DateValue {
  if (typeof stored !== 'object' || stored === null || Array.isArray(stored)) return {};
  const { from: storedFrom, to: storedTo } = stored as { from?: unknown; to?: unknown };
  const from = readPoint(storedFrom);
  if (!from) return {};
  if (storedTo === undefined) return { from };
  const to = readPoint(storedTo);
  if (!to) return {};
  if (from.date !== undefined && to.date !== undefined) {
    if (to.date < from.date) return {};
  } else if (from.instant !== undefined && to.instant !== undefined) {
    if (to.instant < from.instant) return {};
  } else {
    return {};
  }
  return samePoint(from, to) ? { from } : { from, to };
}

export function formatDateValue(value: DateValue): StoredDateValue | undefined {
  const { from, to } = value;
  if (!from) return undefined;
  if (!to || samePoint(from, to)) return { from: writePoint(from) };
  const first = writePoint(from);
  const last = writePoint(to);
  if (typeof first !== typeof last) return { from: first };
  return first <= last ? { from: first, to: last } : { from: last, to: first };
}

export function plainPropertyValue(
  value: PropertyValue | undefined,
): string | string[] | undefined {
  if (value === undefined || typeof value === 'string') return value;
  if (Array.isArray(value)) {
    return value.every((item): item is string => typeof item === 'string') ? value : undefined;
  }
  const stored = formatDateValue(parseDateValue(value));
  if (!stored) return undefined;
  return stored.to === undefined ? String(stored.from) : `${stored.from}/${stored.to}`;
}

export const DYNAMIC_FILTER_TODAY = 'today';
export const DYNAMIC_FILTER_ME = 'me';

export function parseFilterDate(
  text: string | undefined,
  timeZone?: string,
  now?: number,
): CalendarDate | undefined {
  if (text === undefined) return undefined;
  if (text === DYNAMIC_FILTER_TODAY && timeZone) {
    return dateOfInstant(now ?? Date.now(), timeZone);
  }
  return parseCalendarDate(text);
}

export function formatFilterDate(date: CalendarDate): string {
  return date;
}
